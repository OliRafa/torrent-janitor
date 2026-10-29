//! Free space, and the downloads it holds back. Below `low` every download
//! stops and is tagged, so the stop is recognisably ours even after
//! a restart. From `high` up, tagged downloads and errored ones start again,
//! oldest first, as many as fit above `low` next to what is still running.
//! A full disk leaves torrents in error, and qBit's `start` clears it.

use super::Upkeep;
use crate::config::{FreeSpaceRule, RecoverRule};
use crate::observe::Observations;
use crate::qbit::{State, Torrent};
use crate::snapshot::Snapshot;

pub const TAG: &str = "janitor:space";

pub(super) fn evaluate(
    snapshot: &Snapshot,
    observations: &Observations,
    space: &FreeSpaceRule,
    recover: &RecoverRule,
) -> Upkeep {
    let mut upkeep = Upkeep::default();
    let Some(free) = snapshot.free_space else {
        return upkeep;
    };
    if space.enabled && free < space.low {
        upkeep.stop = snapshot
            .torrents
            .iter()
            .filter(|t| t.state.is_downloading())
            .map(|t| t.hash.clone())
            .collect();
        return upkeep;
    }
    if free < space.high {
        return upkeep;
    }

    let committed: u64 = snapshot
        .torrents
        .iter()
        .filter(|t| t.state.is_downloading() && !t.has_tag(TAG))
        .map(|t| t.amount_left)
        .sum();
    let mut headroom = free.saturating_sub(space.low).saturating_sub(committed);
    let attempts = |t: &Torrent| observations.get(&t.hash).map_or(0, |o| o.recover_attempts);
    let mut waiting: Vec<&Torrent> = snapshot
        .torrents
        .iter()
        .filter(|t| {
            (space.enabled && t.has_tag(TAG))
                || (recover.enabled
                    && t.state == State::Error
                    && attempts(t) < recover.max_attempts)
        })
        .collect();
    waiting.sort_by(|a, b| (a.added_on, &a.hash).cmp(&(b.added_on, &b.hash)));
    for torrent in waiting {
        if torrent.amount_left > headroom {
            continue;
        }
        headroom -= torrent.amount_left;
        if space.enabled && torrent.has_tag(TAG) {
            upkeep.start.push(torrent.hash.clone());
        } else {
            upkeep.recover.push(torrent.hash.clone());
        }
    }
    upkeep
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::torrent;

    const GIB: u64 = 1 << 30;

    fn t(hash: &str, state: State, added_on: i64, left_gib: u64) -> Torrent {
        let mut t = torrent(hash, state, "tv-sonarr");
        t.added_on = added_on;
        t.amount_left = left_gib * GIB;
        t
    }

    fn tagged(mut torrent: Torrent) -> Torrent {
        torrent.tags = TAG.into();
        torrent
    }

    fn evaluate_with(torrents: Vec<Torrent>, free_gib: u64) -> Upkeep {
        let snapshot = Snapshot {
            torrents,
            free_space: Some(free_gib * GIB),
            ..Snapshot::default()
        };
        evaluate(
            &snapshot,
            &Observations::new(120),
            &FreeSpaceRule::default(),
            &RecoverRule::default(),
        )
    }

    #[test]
    fn below_the_low_mark_every_running_download_stops() {
        let mut done = t("done", State::StalledUP, 0, 0);
        done.progress = 1.0;
        let upkeep = evaluate_with(
            vec![
                t("dl", State::Downloading, 0, 5),
                t("meta", State::MetaDL, 0, 0),
                t("stalled", State::StalledDL, 0, 5),
                t("stopped", State::StoppedDL, 0, 5),
                t("error", State::Error, 0, 5),
                t("deselected", State::StalledUP, 0, 0),
                done,
            ],
            10,
        );
        assert_eq!(upkeep.stop, ["dl", "meta", "stalled"]);
        assert!(upkeep.start.is_empty() && upkeep.recover.is_empty());
    }

    #[test]
    fn between_the_marks_nothing_changes() {
        let upkeep = evaluate_with(
            vec![
                tagged(t("a", State::StoppedDL, 0, 1)),
                t("b", State::Error, 0, 1),
            ],
            40,
        );
        assert_eq!(upkeep, Upkeep::default());
    }

    #[test]
    fn above_the_high_mark_ours_start_oldest_first_as_many_as_fit() {
        let upkeep = evaluate_with(
            vec![
                tagged(t("newest", State::StoppedDL, 3, 10)),
                tagged(t("oldest", State::StoppedDL, 1, 20)),
                tagged(t("middle", State::StoppedDL, 2, 20)),
            ],
            60,
        );
        assert_eq!(upkeep.start, ["oldest", "newest"], "35G headroom");
    }

    #[test]
    fn running_downloads_take_their_share_of_the_headroom_first() {
        let upkeep = evaluate_with(
            vec![
                t("running", State::Downloading, 0, 30),
                tagged(t("waiting", State::StoppedDL, 1, 10)),
            ],
            60,
        );
        assert!(upkeep.start.is_empty());
    }

    #[test]
    fn errored_torrents_are_recovered_within_their_attempts() {
        let errored = t("e", State::Error, 0, 1);
        let snapshot = Snapshot {
            torrents: vec![errored.clone()],
            free_space: Some(60 * GIB),
            ..Snapshot::default()
        };
        let mut observations = Observations::new(120);
        observations.observe(0, &[errored]);
        let rules = (FreeSpaceRule::default(), RecoverRule::default());
        assert_eq!(
            evaluate(&snapshot, &observations, &rules.0, &rules.1).recover,
            ["e"]
        );
        for _ in 0..3 {
            observations.note_recover_attempt("e");
        }
        assert!(
            evaluate(&snapshot, &observations, &rules.0, &rules.1)
                .recover
                .is_empty()
        );
    }

    #[test]
    fn with_the_free_space_rule_off_nothing_stops_and_tags_are_left_alone() {
        let snapshot = |free_gib: u64| Snapshot {
            torrents: vec![
                t("dl", State::Downloading, 0, 1),
                tagged(t("ours", State::StoppedDL, 0, 1)),
                t("e", State::Error, 0, 1),
            ],
            free_space: Some(free_gib * GIB),
            ..Snapshot::default()
        };
        let space = FreeSpaceRule {
            enabled: false,
            ..FreeSpaceRule::default()
        };
        let observations = Observations::new(120);
        let recover = RecoverRule::default();
        assert_eq!(
            evaluate(&snapshot(1), &observations, &space, &recover),
            Upkeep::default()
        );
        let upkeep = evaluate(&snapshot(60), &observations, &space, &recover);
        assert!(upkeep.start.is_empty());
        assert_eq!(upkeep.recover, ["e"]);
    }

    #[test]
    fn unknown_free_space_changes_nothing() {
        let snapshot = Snapshot {
            torrents: vec![t("dl", State::Downloading, 0, 1)],
            ..Snapshot::default()
        };
        let upkeep = evaluate(
            &snapshot,
            &Observations::new(120),
            &FreeSpaceRule::default(),
            &RecoverRule::default(),
        );
        assert_eq!(upkeep, Upkeep::default());
    }
}
