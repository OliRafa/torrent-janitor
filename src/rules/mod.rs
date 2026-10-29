//! The rules that pick torrents to remove. Each one is pure: it reads the
//! cycle's snapshot and observations and says what it would remove and why.
//! Whether that happens, and how, is the planner's call.

mod disc;
mod duplicate;
mod failed_import;
mod fake;
mod orphan;
mod space;
mod stalled;

use std::collections::HashSet;

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use regex::RegexSet;

use crate::config;
use crate::observe::Observations;
use crate::snapshot::Snapshot;

pub use space::TAG as SPACE_TAG;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid pattern in rules.{rule}")]
    Glob {
        rule: &'static str,
        #[source]
        source: globset::Error,
    },
    #[error("invalid pattern in rules.failed_import")]
    Regex(#[from] regex::Error),
}

/// Ordered by precedence: when several rules pick one torrent, the first
/// reason names the removal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Reason {
    Fake,
    Disc,
    FailedImport,
    Metadata,
    Stalled,
    Duplicate,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::Fake => "fake",
            Reason::Disc => "disc",
            Reason::FailedImport => "failed_import",
            Reason::Metadata => "metadata",
            Reason::Stalled => "stalled",
            Reason::Duplicate => "duplicate",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Removal {
    pub hash: String,
    pub reason: Reason,
    /// Ask the arr to blocklist the release. qBit-only categories cannot.
    pub blocklist: bool,
    /// Ask the arr to search again, if the budget allows.
    pub research: bool,
    pub detail: String,
}

/// What the rules want besides removals.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Upkeep {
    /// Below the low mark: stop and tag with `SPACE_TAG`.
    pub stop: Vec<String>,
    /// Stopped for space and fitting again: start and untag.
    pub start: Vec<String>,
    /// In error: start, which counts an attempt.
    pub recover: Vec<String>,
    /// Report only.
    pub orphans: Vec<String>,
}

#[derive(Debug)]
pub struct RuleSet {
    config: config::Rules,
    media: GlobSet,
    executables: GlobSet,
    disc: GlobSet,
    failed_import: RegexSet,
}

impl RuleSet {
    pub fn new(config: &config::Rules) -> Result<RuleSet, Error> {
        Ok(RuleSet {
            media: globs("fake.media", &config.fake.media)?,
            executables: globs("fake.executables", &config.fake.executables)?,
            disc: globs("disc.patterns", &config.disc.patterns)?,
            failed_import: RegexSet::new(&config.failed_import.patterns)?,
            config: config.clone(),
        })
    }

    pub fn removals(&self, snapshot: &Snapshot, observations: &Observations) -> Vec<Removal> {
        let rules = &self.config;
        let mut removals = Vec::new();
        if rules.fake.enabled {
            removals.extend(fake::evaluate(snapshot, &self.media, &self.executables));
        }
        if rules.disc.enabled {
            removals.extend(disc::evaluate(snapshot, &self.disc));
        }
        if rules.stalled.enabled {
            removals.extend(stalled::evaluate(snapshot, observations, &rules.stalled));
        }
        if rules.failed_import.enabled {
            removals.extend(failed_import::evaluate(
                snapshot,
                &self.failed_import,
                rules.failed_import.research,
            ));
        }
        if rules.duplicate.enabled {
            let taken: HashSet<&str> = removals.iter().map(|r| r.hash.as_str()).collect();
            let duplicates = duplicate::evaluate(snapshot, rules.duplicate.keep, &taken);
            removals.extend(duplicates);
        }
        removals
    }

    pub fn upkeep(&self, snapshot: &Snapshot, observations: &Observations) -> Upkeep {
        let rules = &self.config;
        let mut upkeep = space::evaluate(snapshot, observations, &rules.free_space, &rules.recover);
        if rules.orphan.enabled {
            upkeep.orphans = orphan::evaluate(snapshot, observations);
        }
        upkeep
    }
}

/// qBittorrent's wildcard semantics: case-insensitive, matched against one
/// path component at a time.
fn globs(rule: &'static str, patterns: &[String]) -> Result<GlobSet, Error> {
    let invalid = |source| Error::Glob { rule, source };
    let mut set = GlobSetBuilder::new();
    for pattern in patterns {
        let glob = GlobBuilder::new(pattern)
            .case_insensitive(true)
            .literal_separator(true)
            .build()
            .map_err(invalid)?;
        set.add(glob);
    }
    set.build().map_err(invalid)
}

/// `3h05m`, `31m`, `45s`: durations as a person reads them in a log line.
fn span(secs: u64) -> String {
    match (secs / 3600, secs % 3600 / 60) {
        (0, 0) => format!("{secs}s"),
        (0, m) => format!("{m}m"),
        (h, m) => format!("{h}h{m:02}m"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bad_pattern_fails_at_startup() {
        let mut rules = config::Rules::default();
        rules.fake.executables.push("[".into());
        assert!(matches!(
            RuleSet::new(&rules),
            Err(Error::Glob {
                rule: "fake.executables",
                ..
            })
        ));
        let mut rules = config::Rules::default();
        rules.failed_import.patterns.push("(".into());
        assert!(matches!(RuleSet::new(&rules), Err(Error::Regex(_))));
    }

    #[test]
    fn a_disabled_rule_removes_nothing() {
        let mut rules = config::Rules::default();
        rules.fake.enabled = false;
        rules.disc.enabled = false;
        rules.stalled.enabled = false;
        rules.failed_import.enabled = false;
        rules.duplicate.enabled = false;
        let snapshot = crate::testkit::troubled_snapshot();
        let observations = Observations::new(120);
        assert!(
            RuleSet::new(&rules)
                .unwrap()
                .removals(&snapshot, &observations)
                .is_empty()
        );
        assert!(
            !RuleSet::new(&config::Rules::default())
                .unwrap()
                .removals(&snapshot, &observations)
                .is_empty()
        );
    }

    #[test]
    fn spans_read_like_a_clock() {
        assert_eq!(span(45), "45s");
        assert_eq!(span(31 * 60 + 5), "31m");
        assert_eq!(span(3 * 3600 + 5 * 60), "3h05m");
    }
}
