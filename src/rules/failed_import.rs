//! Downloads the arr finished but will not import, such as a release that
//! is no upgrade over the file already on disk. Left alone they seed in the
//! queue forever.

use regex::RegexSet;

use super::{Reason, Removal};
use crate::snapshot::Snapshot;

const STUCK: &[&str] = &["importPending", "importBlocked"];

pub(super) fn evaluate(snapshot: &Snapshot, patterns: &RegexSet, research: bool) -> Vec<Removal> {
    snapshot
        .downloads
        .iter()
        .filter(|d| {
            d.tracked_state
                .as_deref()
                .is_some_and(|s| STUCK.contains(&s))
        })
        .filter_map(|download| {
            let message = download.messages.iter().find(|m| patterns.is_match(m))?;
            Some(Removal {
                hash: download.hash.clone(),
                reason: Reason::FailedImport,
                blocklist: true,
                research,
                detail: message.clone(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::RuleSet;
    use crate::testkit::download;

    const NOT_AN_UPGRADE: &str = "Not an upgrade for existing episode file(s). Existing quality: WEBDL-2160p. New Quality HDTV-720p.";

    fn removals(state: &str, message: &str, research: bool) -> Vec<Removal> {
        let mut config = crate::config::Rules::default();
        config.failed_import.research = research;
        let rules = RuleSet::new(&config).unwrap();
        let snapshot = Snapshot {
            downloads: vec![download("a", state, &[message])],
            ..Snapshot::default()
        };
        evaluate(&snapshot, &rules.failed_import, research)
    }

    #[test]
    fn a_release_that_is_no_upgrade_is_removed_and_blocklisted() {
        let found = removals("importPending", NOT_AN_UPGRADE, false);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].reason, Reason::FailedImport);
        assert!(found[0].blocklist);
        assert!(!found[0].research);
        assert_eq!(found[0].detail, NOT_AN_UPGRADE);
    }

    #[test]
    fn re_search_follows_the_config() {
        assert!(removals("importBlocked", NOT_AN_UPGRADE, true)[0].research);
    }

    #[test]
    fn messages_that_match_no_pattern_are_left_for_a_person() {
        assert!(
            removals(
                "importBlocked",
                "Episode file already imported at 2026-01-01",
                false
            )
            .is_empty()
        );
    }

    #[test]
    fn downloads_still_running_or_importing_are_left_alone() {
        for state in ["downloading", "importing", "imported", "failedPending"] {
            assert!(removals(state, NOT_AN_UPGRADE, false).is_empty(), "{state}");
        }
    }
}
