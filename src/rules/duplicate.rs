//! The arrs grab another release for an episode or movie while the first is
//! still running, and never cancel the loser. Keep the best `keep` downloads
//! per item and remove the rest without blocklisting: those releases are
//! fine, just surplus.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashSet};

use super::{Reason, Removal};
use crate::observe::{Observation, Observations};
use crate::qbit::Torrent;
use crate::snapshot::{Download, Snapshot};

/// How recently bytes must have arrived for an unfinished download to count
/// as running. A live download pauses now and then; a dead one gets nothing.
const PROGRESS_WINDOW: i64 = 15 * 60;

type Candidate<'a> = (&'a Download, &'a Torrent, Tier);

/// `taken` holds downloads another rule already removes this cycle; they
/// must not count as better than the ones that stay. An unfinished download
/// watched for less than `PROGRESS_WINDOW` sits out, after a restart too:
/// until then a paused one and a dead one look the same. So does a magnet
/// still fetching metadata: it is the stalled rule's, which blocklists a dead
/// one instead of leaving the arr free to grab it again.
pub(super) fn evaluate(
    snapshot: &Snapshot,
    observations: &Observations,
    keep: usize,
    taken: &HashSet<&str>,
) -> Vec<Removal> {
    let candidates: Vec<Candidate> = snapshot
        .downloads
        .iter()
        .filter(|d| !d.items.is_empty() && !taken.contains(d.hash.as_str()))
        .filter_map(|d| {
            let torrent = snapshot.torrent(&d.hash)?;
            let tier = tier(torrent, observations.get(&d.hash), snapshot.now)?;
            Some((d, torrent, tier))
        })
        .collect();

    let mut ranked: BTreeMap<(usize, i64), Vec<usize>> = BTreeMap::new();
    for (i, (download, _, _)) in candidates.iter().enumerate() {
        for item in &download.items {
            ranked.entry((download.arr, *item)).or_default().push(i);
        }
    }
    let mut kept = vec![false; candidates.len()];
    for indices in ranked.values_mut() {
        indices.sort_by(|&a, &b| compare(candidates[b], candidates[a]));
        indices.truncate(keep);
        for &i in indices.iter() {
            kept[i] = true;
        }
    }

    // A download goes only when it is outranked on every item it covers, so
    // a season pack is never dropped for one episode.
    candidates
        .iter()
        .zip(&kept)
        .filter(|(_, kept)| !**kept)
        .map(|((download, _, _), _)| {
            let winners: BTreeSet<&str> = download
                .items
                .iter()
                .filter_map(|item| ranked.get(&(download.arr, *item)))
                .flatten()
                .map(|&i| candidates[i].0.title.as_str())
                .collect();
            Removal {
                hash: download.hash.clone(),
                reason: Reason::Duplicate,
                blocklist: false,
                research: false,
                detail: format!(
                    "superseded by {}",
                    winners.into_iter().collect::<Vec<_>>().join(", ")
                ),
            }
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Tier {
    Stuck,
    Progressing,
    Complete,
}

/// Running means bytes arrived within the window, whatever qBit's state
/// says this minute. `None` while it is too early to tell.
fn tier(torrent: &Torrent, observation: Option<&Observation>, now: i64) -> Option<Tier> {
    if torrent.is_complete() {
        return Some(Tier::Complete);
    }
    if torrent.state.is_metadata() {
        return None;
    }
    let observation = observation.filter(|o| now - o.first_seen >= PROGRESS_WINDOW)?;
    let running = observation
        .progressed_at
        .is_some_and(|at| now - at < PROGRESS_WINDOW);
    Some(if running {
        Tier::Progressing
    } else {
        Tier::Stuck
    })
}

/// `Greater` when `a` is the better download. Finished beats running beats
/// stuck. Running downloads are ranked by quality, stuck ones by how close
/// they are to done. A tie goes to the earlier grab.
fn compare((a, ta, tier_a): Candidate, (b, tb, tier_b): Candidate) -> Ordering {
    let quality =
        (a.quality_rank, a.custom_format_score).cmp(&(b.quality_rank, b.custom_format_score));
    let progress = ta.progress.total_cmp(&tb.progress);
    let within = if tier_a == Tier::Stuck {
        progress.then(quality)
    } else {
        quality.then(progress)
    };
    tier_a
        .cmp(&tier_b)
        .then(within)
        .then_with(|| tb.added_on.cmp(&ta.added_on))
        .then_with(|| b.hash.cmp(&a.hash))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::qbit::State;
    use crate::testkit::{download, torrent};

    struct Grab {
        hash: &'static str,
        state: State,
        progress: f64,
        quality: usize,
        items: &'static [i64],
    }

    fn grab(
        hash: &'static str,
        state: State,
        progress: f64,
        quality: usize,
        items: &'static [i64],
    ) -> Grab {
        Grab {
            hash,
            state,
            progress,
            quality,
            items,
        }
    }

    fn snapshot(grabs: &[Grab]) -> Snapshot {
        let mut snapshot = Snapshot::default();
        for (i, g) in grabs.iter().enumerate() {
            let mut t = torrent(g.hash, g.state, "tv-sonarr");
            t.progress = g.progress;
            t.added_on = i as i64;
            snapshot.torrents.push(t);
            let mut d = download(g.hash, "downloading", &[]);
            d.items = g.items.to_vec();
            d.quality_rank = Some(g.quality);
            snapshot.downloads.push(d);
        }
        snapshot
    }

    /// Watches `snapshot` for one window in which only `alive` torrents get bytes.
    fn watch(snapshot: &mut Snapshot, alive: impl Fn(&Torrent) -> bool) -> Observations {
        let mut observations = Observations::new(120);
        observations.observe(0, &snapshot.torrents);
        for t in &mut snapshot.torrents {
            if alive(t) {
                t.downloaded += 1;
            }
        }
        snapshot.now = PROGRESS_WINDOW;
        observations.observe(snapshot.now, &snapshot.torrents);
        observations
    }

    /// Judges after a window in which whatever qBit calls downloading got bytes.
    fn judge(mut snapshot: Snapshot, keep: usize, taken: &HashSet<&str>) -> Vec<Removal> {
        let observations = watch(&mut snapshot, |t| t.state == State::Downloading);
        evaluate(&snapshot, &observations, keep, taken)
    }

    fn removed(grabs: &[Grab], keep: usize) -> Vec<String> {
        let mut hashes: Vec<String> = judge(snapshot(grabs), keep, &HashSet::new())
            .into_iter()
            .map(|r| r.hash)
            .collect();
        hashes.sort();
        hashes
    }

    #[test]
    fn two_downloads_of_one_episode_both_run() {
        let grabs = [
            grab("a", State::StalledDL, 0.1, 5, &[101]),
            grab("b", State::Downloading, 0.5, 9, &[101]),
        ];
        assert!(removed(&grabs, 2).is_empty());
    }

    #[test]
    fn a_third_download_of_one_episode_goes() {
        let grabs = [
            grab("a", State::StalledDL, 0.1, 9, &[101]),
            grab("b", State::Downloading, 0.5, 5, &[101]),
            grab("c", State::Downloading, 0.2, 7, &[101]),
        ];
        assert_eq!(removed(&grabs, 2), ["a"]);
    }

    #[test]
    fn a_finished_download_beats_a_better_one_still_running() {
        let grabs = [
            grab("a", State::StalledUP, 1.0, 1, &[101]),
            grab("b", State::Downloading, 0.9, 9, &[101]),
        ];
        assert_eq!(removed(&grabs, 1), ["b"]);
    }

    #[test]
    fn running_downloads_rank_by_quality_then_custom_format_score() {
        let grabs = [
            grab("a", State::Downloading, 0.9, 5, &[101]),
            grab("b", State::Downloading, 0.1, 9, &[101]),
        ];
        assert_eq!(removed(&grabs, 1), ["a"]);

        let mut snapshot = snapshot(&[
            grab("a", State::Downloading, 0.9, 5, &[101]),
            grab("b", State::Downloading, 0.1, 5, &[101]),
        ]);
        snapshot.downloads[1].custom_format_score = 100;
        let removals = judge(snapshot, 1, &HashSet::new());
        assert_eq!(removals[0].hash, "a");
    }

    #[test]
    fn stuck_downloads_rank_by_progress() {
        let grabs = [
            grab("a", State::StalledDL, 0.2, 9, &[101]),
            grab("b", State::Error, 0.0, 9, &[101]),
            grab("c", State::StalledDL, 0.7, 1, &[101]),
        ];
        assert_eq!(removed(&grabs, 2), ["b"]);
    }

    #[test]
    fn a_magnet_fetching_metadata_is_left_to_the_stalled_rule() {
        let grabs = [
            grab("magnet", State::MetaDL, 0.0, 9, &[101]),
            grab("a", State::StalledDL, 0.0, 1, &[101]),
            grab("b", State::StalledDL, 0.0, 2, &[101]),
        ];
        assert_eq!(removed(&grabs, 1), ["a"], "neither removed nor ranked");
    }

    #[test]
    fn a_running_download_that_pauses_keeps_its_rank() {
        let mut snapshot = snapshot(&[
            grab("paused", State::StalledDL, 0.1, 9, &[101]),
            grab("dead", State::StalledDL, 0.6, 1, &[101]),
            grab("running", State::Downloading, 0.3, 5, &[101]),
        ]);
        let observations = watch(&mut snapshot, |t| t.hash != "dead");
        let removals = evaluate(&snapshot, &observations, 2, &HashSet::new());
        assert_eq!(removals.len(), 1);
        assert_eq!(removals[0].hash, "dead");
    }

    #[test]
    fn nothing_unfinished_is_judged_before_a_whole_window_is_watched() {
        let mut snapshot = snapshot(&[
            grab("a", State::StalledDL, 0.1, 9, &[101]),
            grab("b", State::Downloading, 0.5, 5, &[101]),
            grab("c", State::Downloading, 0.2, 7, &[101]),
        ]);
        let mut observations = Observations::new(120);
        observations.observe(0, &snapshot.torrents);
        snapshot.now = PROGRESS_WINDOW - 60;
        observations.observe(snapshot.now, &snapshot.torrents);
        assert!(evaluate(&snapshot, &observations, 2, &HashSet::new()).is_empty());
    }

    #[test]
    fn a_season_pack_stays_while_any_episode_needs_it() {
        let grabs = [
            grab("pack", State::StalledDL, 0.0, 5, &[101, 102]),
            grab("e1a", State::Downloading, 0.5, 5, &[101]),
            grab("e1b", State::Downloading, 0.5, 5, &[101]),
        ];
        assert!(removed(&grabs, 2).is_empty());
    }

    #[test]
    fn a_season_pack_goes_once_every_episode_has_better_downloads() {
        let grabs = [
            grab("pack", State::StalledDL, 0.0, 5, &[101, 102]),
            grab("e1a", State::Downloading, 0.5, 5, &[101]),
            grab("e1b", State::Downloading, 0.5, 5, &[101]),
            grab("e2a", State::Downloading, 0.5, 5, &[102]),
            grab("e2b", State::Downloading, 0.5, 5, &[102]),
        ];
        let removals = judge(snapshot(&grabs), 2, &HashSet::new());
        assert_eq!(removals.len(), 1);
        assert_eq!(removals[0].hash, "pack");
        assert!(!removals[0].blocklist && !removals[0].research);
        assert_eq!(
            removals[0].detail,
            "superseded by Example.e1a, Example.e1b, Example.e2a, Example.e2b"
        );
    }

    #[test]
    fn the_same_item_id_in_two_arrs_is_two_items() {
        let mut snapshot = snapshot(&[
            grab("a", State::Downloading, 0.5, 5, &[101]),
            grab("b", State::Downloading, 0.5, 5, &[101]),
        ]);
        snapshot.downloads[1].arr = 1;
        assert!(judge(snapshot, 1, &HashSet::new()).is_empty());
    }

    #[test]
    fn downloads_taken_by_another_rule_do_not_outrank_anyone() {
        let grabs = [
            grab("junk", State::StalledUP, 1.0, 9, &[101]),
            grab("a", State::Downloading, 0.5, 5, &[101]),
            grab("b", State::Downloading, 0.4, 5, &[101]),
        ];
        let taken = HashSet::from(["junk"]);
        assert!(judge(snapshot(&grabs), 2, &taken).is_empty());
    }

    #[test]
    fn a_download_without_items_is_never_judged() {
        let grabs = [
            grab("a", State::Downloading, 0.5, 5, &[101]),
            grab("b", State::StalledDL, 0.0, 5, &[]),
        ];
        assert!(removed(&grabs, 1).is_empty());
    }
}
