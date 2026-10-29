# torrent-janitor Developer Guide

Guidelines for agents and developers working on this repository. The design and
the delivery phases are in [PLAN.md](PLAN.md). Read it before changing anything.

## 1. Working Principles

### Caveman

Be concise, respont terse like a smark caveman. All technical substance stay. Only fluff die.

Rules:
Drop: filler (just/really/basically), plesantries, hedging.
Fragments OK. Short synonyms. Technical terms exact. Code unchanged.
Patter: [thing] [action] [reason]. [next step].

Auto-Clarity: drop caveman for security warnings, irreversible actions, user confused, or when user explicitly asks to.
Resume after.
Boundaries: code/commits/PRs written normal.

### Planning

- When asked to plan: output only the plan. No code until told to proceed.
- When given a plan: follow it exactly. Flag real problems and wait.
- For non-trivial features: interview about implementation and tradeoffs before writing code.
- Work phase by phase as listed in PLAN.md, at most about 5 files per phase.

### Code Quality

- If architecture is flawed or patterns are inconsistent, propose and implement the
  structural fix rather than working around it.
- Write code that reads like a human wrote it. Default to no comments; comment only
  when the WHY is non-obvious.
- Keep existing comments on refactor. They carry intent you cannot reconstruct.
- Don't build for imaginary scenarios. Simple and correct beats elaborate.

### Edit Safety

- Before every file edit: re-read the file. After editing: read it again.
- On any rename or signature change, search separately for call sites, trait
  implementations, serde attributes and test fakes.

## 2. Build, Lint and Test

Everything runs through one script, which is the only definition of "passing":

```bash
./scripts/ci.sh
```

It checks `cargo fmt`, runs `cargo clippy --all-targets -- -D warnings`, builds
with `--locked`, and runs the tests. `hooks/pre-commit`, `hooks/pre-push` and
GitHub Actions all run the full script; there is intentionally no fast mode.

## 3. Code Style and Conventions

- **Rust**: the version pinned in `rust-toolchain.toml`, edition 2024. Standard
  library first; a dependency needs a reason (PLAN.md §4 lists the current ones).
- **Errors**: `thiserror` enums in library code, so callers can tell "the API
  answered no" from "the API could not be reached". No `unwrap`/`expect` outside
  tests and startup validation.
- **Purity**: rules take a snapshot and return actions. A rule that needs I/O is a
  design bug.
- **Safety first**: anything that removes data must be visible in dry-run, capped
  per cycle, and logged once with its reason.

## 4. Testing Strategy

- **Mandatory coverage**: every behaviour has a test. Tests drive the design.
- **Fakes over mocks**: the qBit and arr clients sit behind traits, and tests
  implement them with small structs. `wiremock` is only for client wire tests.
- **Fixtures come from the real cluster**, captured read-only. The repo is public:
  replace every title, path, hash and hostname with a synthetic one before it is
  committed.
- Name tests for the behaviour, not the function:
  `queued_time_never_counts_as_stalled`.

## 5. Commits

- **Format**: [Conventional Commits](https://www.conventionalcommits.org/) -
  `type(scope): description`.
- Commits are small, self-contained, and leave the tree passing on their own.
- Explain *why* in the body. The diff already shows what changed.
