//! Torrents that are started and want data but have made no progress for
//! too long. Only observed stalled time counts; see `observe`.

use super::{Reason, Removal, span};
use crate::config::StalledRule;
use crate::observe::Observations;
use crate::qbit::State;
use crate::snapshot::Snapshot;

pub(super) fn evaluate(
    snapshot: &Snapshot,
    observations: &Observations,
    rule: &StalledRule,
) -> Vec<Removal> {
    snapshot
        .torrents
        .iter()
        .filter_map(|torrent| {
            let (reason, limit, missing) = match torrent.state {
                s if s.is_metadata() => (Reason::Metadata, rule.metadata_after, "metadata"),
                State::StalledDL => (Reason::Stalled, rule.stalled_after, "progress"),
                _ => return None,
            };
            let stalled_for = observations.get(&torrent.hash)?.stalled_for;
            (stalled_for >= limit.as_secs()).then(|| Removal {
                hash: torrent.hash.clone(),
                reason,
                blocklist: true,
                research: true,
                detail: format!("no {missing} for {} of active time", span(stalled_for)),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::qbit::Torrent;
    use crate::testkit::torrent;

    const TICK: i64 = 60;

    /// Watches one torrent for `minutes` of one-minute cycles.
    fn watch(t: &Torrent, minutes: i64) -> Vec<Removal> {
        let mut observations = Observations::new(120);
        for tick in 0..=minutes {
            observations.observe(tick * TICK, std::slice::from_ref(t));
        }
        let snapshot = Snapshot {
            torrents: vec![t.clone()],
            ..Snapshot::default()
        };
        evaluate(&snapshot, &observations, &StalledRule::default())
    }

    #[test]
    fn missing_metadata_fires_after_thirty_minutes() {
        let t = torrent("a", State::MetaDL, "tv-sonarr");
        assert!(watch(&t, 29).is_empty());
        let found = watch(&t, 30);
        assert_eq!(found[0].reason, Reason::Metadata);
        assert_eq!(found[0].detail, "no metadata for 30m of active time");
    }

    #[test]
    fn a_stalled_download_fires_after_three_hours() {
        let t = torrent("a", State::StalledDL, "tv-sonarr");
        assert!(watch(&t, 179).is_empty());
        let found = watch(&t, 180);
        assert_eq!(found[0].reason, Reason::Stalled);
        assert!(found[0].blocklist && found[0].research);
    }

    #[test]
    fn queued_stopped_and_checking_torrents_never_fire() {
        for state in [
            State::QueuedDL,
            State::StoppedDL,
            State::CheckingDL,
            State::Downloading,
        ] {
            assert!(
                watch(&torrent("a", state, "tv-sonarr"), 600).is_empty(),
                "{state:?}"
            );
        }
    }

    #[test]
    fn a_torrent_first_seen_this_cycle_does_not_fire() {
        let snapshot = Snapshot {
            torrents: vec![torrent("a", State::StalledDL, "tv-sonarr")],
            ..Snapshot::default()
        };
        let rule = StalledRule::default();
        assert!(evaluate(&snapshot, &Observations::new(120), &rule).is_empty());
    }
}
