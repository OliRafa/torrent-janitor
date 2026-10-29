# torrent-janitor: implementation plan

A long-running Rust service that keeps qBittorrent download queues healthy for
a home-lab Sonarr/Radarr stack on Kubernetes. It removes dead, duplicate,
fake and unimportable downloads *through the arr* (so the arr blocklists the
release and searches again, within a budget). It guards free space with qBit 5's
stop/start, and recovers torrents that errored on a full disk. It replaces
Torrentarr's torrent handling.

## 1. Why build it

Every requirement below traces back to something that broke in production
(Oct 2026):

| Incident | Requirement |
|---|---|
| Torrentarr timed a metaDL stall from `added_on`, so a torrent that sat in qBit's queue was "stalled" the instant it started. One episode was grabbed 66 times in a single day (2026-08-10). | The stall clock counts only *observed active time without byte progress*. Queued, stopped and checking time never counts. |
| Free-space pause called `/torrents/pause`, which is a 404 on qBit 5. The library filled to zero. | Check the qBit WebAPI version at startup (≥ 2.11), use only `stop`/`start`, refuse to run otherwise. |
| `ArrErrorCodesToBlocklist` needed an exact match, but Sonarr appends quality detail, so it never fired. | Match failed imports with configurable regexes. |
| After 6.19, `RemoveTorrent=-1` silently refused every removal while the log still said "Deleting…" (~35k lines/day). | Log each decision once, with its reason. Every skip gets a reason label in metrics. No log line per torrent per cycle. |
| Sonarr grabbed several releases per episode and never cancels the superseded one; no existing tool handles this. | Duplicate rule. |
| A lone 1 GB `.exe`, named like the episode, posed as a release. | Executable/fake rule (qBit's `ExcludedFileNames` is the first line of defence). |
| Torrentarr's config was rewritten at runtime, and Cleanuparr keeps its config in a UI database. | Declarative TOML from a ConfigMap; secrets from env. |

Off-the-shelf options were evaluated on 2026-10-29. Cleanuparr is good but
UI-configured and has no duplicate handling; Decluttarr loses strikes on restart
and needs a maintainer; qBitrr has the same `added_on` bug.

## 2. Principles

- **Rules are pure.** `(Snapshot, Observations, Config) -> Vec<Action>`. All I/O
  lives in the clients and the executor, so every rule is unit-tested on
  fixtures.
- **Remove through the arr** whenever the arr owns the torrent (`DELETE
  /api/v3/queue/{id}`), so blocklisting and re-search stay the arr's job. Remove
  directly in qBit only for instances without an arr.
- **Safe by default.**
  - `dry_run = true` until switched off.
  - A per-cycle cap on removals.
  - Never touch a category that isn't configured.
  - Never act on a partial snapshot: if any fetch fails, skip the cycle.
- **Losing state only delays.** Observations live in memory. A restart resets
  the stall clocks, so removals happen later, never sooner.
- **One process, constant loop** (a Deployment, not a CronJob). It ticks every
  `interval`, shuts down cleanly on SIGTERM, and serves `/healthz` and
  `/metrics`.

## 3. Rules (v1)

| Rule | Fires when | Action | Blocklist | Re-search |
|---|---|---|---|---|
| `fake` | Metadata is known, and no file has a media extension, while at least one file matches the executable list. Media counts even when deselected: genuine releases end up with every file deselected, and Blu-ray discs carry BD-J `.jar` menus (both seen in the first dry-run). | Remove via the arr | yes | budgeted |
| `disc` | Metadata is known and a path component matches the disc list (`BDMV`, `VIDEO_TS`, `*.iso`, `*.img`), selected or not: a whole disc instead of video files. Release titles often hide it (TRaSH's BR-DISK format skips titles tagged H.265), so this reads the files. | Remove via the arr | yes | budgeted |
| `stalled` | State is `metaDL` or `stalledDL` (not queued, stopped, checking or forced-off) with no increase in `downloaded` for `stall_after` of observed active time. Separate thresholds for metaDL and stalledDL. | Remove via the arr | yes | budgeted |
| `duplicate` | One episode or movie has more than `keep` (default 2) distinct `downloadId`s in the arr queue | Keep the best `keep` (below), remove the rest | no | no (`skipRedownload`) |
| `failed_import` | `trackedDownloadState` is `importPending`/`importBlocked` and a status message matches a configured regex | Remove via the arr | yes | configurable |
| `free_space` | Free space drops below `low`. It comes from `file`, btrfs-local-csi's per-volume JSON, when set. qBit's `server_state.free_space_on_disk` ignores btrfs quotas unless an LD_PRELOAD shim fences it, and a static qbittorrent-nox build (linuxserver's since 5.2.0) cannot load one | `stop` every running download and tag it `janitor:space`, except magnets still fetching metadata: they have nothing to write, and stopped they could never time out. From `high` up, `start` tagged ones again oldest first, as many as fit above `low` next to what is still running (hysteresis) | none | none |
| `recover` | Torrent in `error` while free space is above `high` | `start`, at most N attempts per hash, sharing the headroom with `free_space` | none | none |
| `orphan` | qBit torrent in an arr category, incomplete, absent from the arr queue for 15 minutes | Report only (metric and log) in v1 | none | none |

**Duplicate ranking:**
1. Completed or importing wins.
2. Otherwise prefer a download that is progressing, ranked by quality (the arr
   reports it) and then custom-format score. Progressing means bytes arrived in
   the last 15 minutes, not qBit's state of the moment: a live download that
   stalls for a minute would otherwise lose its rank to a dead one, and the
   first dry-run showed decisions flipping that way. An unfinished download
   the janitor hasn't watched for 15 minutes, after a restart too, isn't
   ranked yet. Nor is a magnet still fetching metadata: it is `stalled`'s,
   which blocklists a dead one. Ranked, it lost at 15 minutes and went
   unblocklisted before `stalled` could judge it at 30 (about 90 of the first 151
   live duplicate removals).
3. Otherwise take the highest progress.

The top `keep` downloads stay, not just the best one: two run side by side, so a
slow first grab doesn't block a faster second one. Nothing extra is needed once
the race ends:
- An imported download leaves the arr queue and stops counting.
- If the loser finishes later, the arr either imports it as an upgrade or
  rejects it ("Not an upgrade…"), and `failed_import` removes it.

A download covering several episodes (a season pack) is removed only if *every*
episode it covers already has `keep` better downloads. A pack is never dropped
for one episode.

**Re-search budget:** each episode or movie gets at most `max_researches_per_day`
re-searches (default 2). Beyond that, removal still blocklists but sends
`skipRedownload=true`, and the budget exhaustion shows up in a metric. This is
the guard against another 66-grab loop. An item is searched for only when its
last download in the arr queue goes, cap-held ones counting as still there:
the first dry-run had 22 of 23 re-searches aimed at episodes with 3 to 16
other downloads queued, and each search can grab one more.

**Actions are merged per download:** season packs appear as one queue record per
episode, so actions are grouped by `downloadId` and send one DELETE each.
Deleting one record removes the whole download (verified on 2026-10-29).

**Names:** a magnet added without a display name is called by its hash in qBit
and in the arr queue alike, 40 of 297 torrents in the first dry-run. The log
names it by the release title from the arr's grab history instead
(`/api/v3/history?downloadId=<HASH>&eventType=1`; the id must be upper-case),
looked up once per torrent. A failed lookup only leaves the hash.

## 4. Architecture

```
src/main.rs        config path from env, wiring, signals, the tick loop
src/config.rs      TOML schema and validation at startup
src/qbit/          WebAPI client: version, torrents/info, files (cached per hash),
                   sync/maindata (server_state), delete, stop, start, tags
src/arr/           Sonarr/Radarr v3 client: paged queue, DELETE queue/{id}
src/snapshot.rs    one consistent view: torrents ⋈ queue records by hash
src/observe.rs     per-hash memory: last state, last downloaded bytes,
                   accumulated active-no-progress time, first seen
src/rules/         fake, disc, stalled, duplicate, failed_import, free_space, recover, orphan
src/plan.rs        one removal per download; routing, cap and budget; upkeep
src/budget.rs      re-search budget per media item
src/api.rs         the client traits, so the cycle and executor run on fakes
src/executor.rs    dry-run vs live; every decision logged once
src/janitor.rs     one cycle: fetch, snapshot, rules, plan, execute, metrics
src/http.rs        axum: /healthz, /metrics
src/metrics.rs     prometheus-client registry
```

**One cycle:**
1. Fetch qBit torrents and server_state, plus files for hashes not yet cached.
   File lists don't change after metadata arrives.
2. Fetch every page of each arr queue.
3. Build the snapshot and update the observations.
4. Evaluate the rules, then plan, then execute.
5. Record metrics.

**Crates, each with a reason:**
- `tokio`: the runtime.
- `reqwest` (rustls, json): HTTP without OpenSSL, which allows a static musl build.
- `serde`, `serde_json`, `toml`, `humantime-serde`: config and API types.
- `regex`: failed-import matching.
- `globset`: executable and media patterns, using the same wildcard semantics as
  qBit.
- `tracing` and `tracing-subscriber` (JSON): logs.
- `axum`: health and metrics.
- `prometheus-client`: metrics.
- `thiserror`: client errors.
- Dev only: `wiremock`, used only for client wire tests. Rules and the executor
  use hand-written fakes, not a mocking framework.

**Config sketch:**

```toml
interval = "60s"
dry_run = true
max_removals_per_cycle = 10

[qbit]
url = "http://qbittorrent:8080"          # WebUI auth is bypassed for the cluster subnet

[[arr]]
kind = "sonarr"
url = "http://sonarr:8989/sonarr"        # UrlBase included; redirects are not followed
api_key_env = "SONARR_API_KEY"
category = "tv-sonarr"

[[arr]]
kind = "radarr"
url = "http://radarr:7878/radarr"
api_key_env = "RADARR_API_KEY"
category = "movies-radarr"

[rules.stalled]                          # active time without byte progress
metadata_after = "30m"
stalled_after = "3h"

[rules.duplicate]
keep = 2                                 # concurrent downloads allowed per episode/movie

[rules.failed_import]
patterns = ["^Not an upgrade for existing", "^Not a Custom Format upgrade", "^No files found are eligible"]

[rules.free_space]
low = "25G"
high = "50G"

[rules.budget]
max_researches_per_day = 2
```

## 5. Testing

- **Fixtures captured from the live cluster**, read-only. This repo is public, so
  before committing, every title, path and hash is replaced with a synthetic one.
  Only the shapes and states stay. Coverage:
  - qBit `torrents/info` with metaDL torrents whose name is only the hash
  - errored torrents
  - a season-pack Sonarr queue
  - "not an upgrade" warnings
  - an executable-only fake torrent
- **Rules:** table tests over fixture snapshots, one behaviour per test, named for
  the behaviour.
- **Observation clock:** simulated ticks, including "queued for 10h, then metaDL
  for 5 min" (must not fire) and "restart loses state" (fires later, never
  sooner).
- **Executor:** fakes for the qBit and arr traits; assert one DELETE per
  download, the caps, and that dry-run makes no calls.
- **e2e:** the real binary against stub upstreams, asserting the HTTP calls it
  makes.
- **`scripts/ci.sh`** is the single definition of passing: `cargo fmt --check`,
  `cargo clippy --all-targets -- -D warnings`, `cargo test`,
  `cargo build --locked`. The git hooks and GitHub Actions run the same script.

## 6. Phases

Each phase is at most about 5 files, ends with CI green, and is reviewed before
the next one starts.

0. **Skeleton (done):** PLAN.md, CLAUDE.md, README.md, .gitignore. Repo public.
1. **Toolchain (done):**
   - Cargo.toml and `rust-toolchain.toml` (pinned stable)
   - `src/main.rs` stub
   - `scripts/ci.sh`, hooks, the CI workflow
2. **qBit client (done):** version gate, info, files, maindata, delete, stop/start, tags.
   Fixtures captured. Wire tests.
3. **arr client (done):** paged queue, queue delete, with season-pack fixtures.
4. **Config, snapshot, observations (done),** including the clock tests above.
5. **Rules (done):** `fake`, `stalled`, `failed_import`.
6. **Rules (done):** `duplicate` and the budget.
7. **Plan, executor, metrics, health, tick loop (done).** Dry-run end to end.
8. **Rules (done):** `free_space`, `recover`, `orphan`.
9. **Packaging (done):**
   - Dockerfile (musl static → distroless)
   - CD to `ghcr.io/olirafa/torrent-janitor` with `main` and `sha-…` tags
     (Renovate pins `tag@digest`)
10. **Deploy in home-servers-setup (dry-run, deployed 2026-10-29; compare from 2026-10-31):**
    - Deployment (Recreate, 1 replica), ConfigMap
    - env from the existing `sonarr-api-key`/`radarr-api-key` secrets
    - Prometheus scrape job in `additionalScrapeConfigs`
    - Flux kustomization
    - Run dry-run for at least 48h and compare the intended actions against what
      we'd do by hand.
11. **Go live in localflix.** Turn off Torrentarr's torrent handling; it keeps
    running for missing-item search only. Drain the backlog under the per-cycle
    cap.
12. **Bookstore instance (dry-run, deployed 2026-10-29)**, qBit-only: `fake`, `free_space`, `recover`.
13. **Rule (done):** `disc`. The first dry-run surfaced a Blu-ray disc that a
    release title had disguised; whole discs are unwanted.

## 7. Decisions (2026-10-29)

1. **Public repo.** The cluster has no `imagePullSecrets`, so the image has to be
   public anyway.
2. **Torrentarr stays for missing-item search only.** The arrs do RSS only, and
   that search is the one thing it does that nothing else here does. Turning its
   torrent handling off without losing search still has to be verified in
   phase 11.
3. **Stall thresholds:** metaDL 30m and stalledDL 3h of active time, both
   configurable.
4. **Duplicates:** the ranking above, keeping `keep = 2` downloads side by side.
5. **State:** in memory.
6. **Bookstore:** qBit-only rules (`fake`, `free_space`, `recover`). No stalled
   removal until there's a Kapowarr integration, because removal there can't
   blocklist.
7. **Error recovery:** settled from the qBittorrent source in phase 8.
   `TorrentImpl::start()` calls `clear_error()` and unsets `upload_mode`, the
   two things qBit's Error state is made of (`hasError()`), so `torrents/start`
   resumes a torrent a full disk errored. Watch the first live recoveries
   anyway.
