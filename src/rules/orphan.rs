//! Incomplete torrents in an arr's category that its queue does not list.
//! The grace period covers a grab the arr has sent but not yet picked up in
//! its queue. v1 only reports them.

use crate::observe::Observations;
use crate::snapshot::Snapshot;

const GRACE: i64 = 15 * 60;

pub(super) fn evaluate(snapshot: &Snapshot, observations: &Observations) -> Vec<String> {
    snapshot
        .untracked
        .iter()
        .filter(|hash| {
            snapshot
                .torrent(hash)
                .is_some_and(|t| t.state.is_downloading())
        })
        .filter(|hash| {
            observations
                .get(hash)
                .is_some_and(|o| snapshot.now - o.first_seen >= GRACE)
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::qbit::State;
    use crate::testkit::torrent;

    fn orphans(state: State, seen_for: i64) -> Vec<String> {
        let t = torrent("a", state, "tv-sonarr");
        let mut observations = Observations::new(120);
        observations.observe(0, std::slice::from_ref(&t));
        let snapshot = Snapshot {
            now: seen_for,
            torrents: vec![t],
            untracked: vec!["a".into()],
            ..Snapshot::default()
        };
        evaluate(&snapshot, &observations)
    }

    #[test]
    fn an_untracked_download_is_reported_after_the_grace_period() {
        assert!(orphans(State::StalledDL, GRACE - 1).is_empty());
        assert_eq!(orphans(State::StalledDL, GRACE), ["a"]);
    }

    #[test]
    fn a_finished_torrent_seeding_after_import_is_not_an_orphan() {
        assert!(orphans(State::StalledUP, 10 * GRACE).is_empty());
    }
}
