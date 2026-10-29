//! What the janitor has seen of each torrent across cycles. This is the
//! stall clock: it counts only time observed stalled, never time spent
//! queued, and it lives in memory. A restart starts every clock again,
//! which delays a removal and can never hasten one.

use std::collections::HashMap;

use crate::qbit::Torrent;

#[derive(Debug, Clone, PartialEq)]
pub struct Observation {
    pub first_seen: i64,
    pub last_seen: i64,
    pub downloaded: u64,
    /// Seconds observed stalled since the last byte arrived.
    pub stalled_for: u64,
    pub recover_attempts: u32,
    was_stalled: bool,
}

#[derive(Debug, Default)]
pub struct Observations {
    torrents: HashMap<String, Observation>,
    max_gap: u64,
}

impl Observations {
    /// `max_gap` caps how much time one observation can add, so cycles
    /// skipped after a failed fetch are not counted as stalled time nobody saw.
    pub fn new(max_gap: u64) -> Self {
        Observations {
            torrents: HashMap::new(),
            max_gap,
        }
    }

    pub fn observe(&mut self, now: i64, torrents: &[Torrent]) {
        let mut seen = HashMap::with_capacity(torrents.len());
        for torrent in torrents {
            let stalled = torrent.state.is_stalled();
            let observation = match self.torrents.remove(&torrent.hash) {
                None => Observation {
                    first_seen: now,
                    last_seen: now,
                    downloaded: torrent.downloaded,
                    stalled_for: 0,
                    recover_attempts: 0,
                    was_stalled: stalled,
                },
                Some(mut o) => {
                    let elapsed = u64::try_from(now - o.last_seen)
                        .unwrap_or(0)
                        .min(self.max_gap);
                    if torrent.downloaded > o.downloaded {
                        o.stalled_for = 0;
                        o.recover_attempts = 0;
                    } else if stalled && o.was_stalled {
                        o.stalled_for += elapsed;
                    }
                    o.downloaded = torrent.downloaded;
                    o.last_seen = now;
                    o.was_stalled = stalled;
                    o
                }
            };
            seen.insert(torrent.hash.clone(), observation);
        }
        self.torrents = seen;
    }

    pub fn get(&self, hash: &str) -> Option<&Observation> {
        self.torrents.get(hash)
    }

    pub fn note_recover_attempt(&mut self, hash: &str) {
        if let Some(o) = self.torrents.get_mut(hash) {
            o.recover_attempts += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::qbit::State;
    use crate::testkit::torrent;

    const TICK: i64 = 60;

    fn run(observations: &mut Observations, start: i64, ticks: i64, t: &Torrent) -> i64 {
        let mut now = start;
        for _ in 0..ticks {
            now += TICK;
            observations.observe(now, std::slice::from_ref(t));
        }
        now
    }

    #[test]
    fn queued_time_never_counts_as_stalled() {
        let mut observations = Observations::new(120);
        let queued = torrent("a", State::QueuedDL, "tv");
        let now = run(&mut observations, 0, 600, &queued);
        let fetching = torrent("a", State::MetaDL, "tv");
        run(&mut observations, now, 5, &fetching);
        assert_eq!(observations.get("a").unwrap().stalled_for, 4 * 60);
    }

    #[test]
    fn stalled_time_accumulates_until_a_byte_arrives() {
        let mut observations = Observations::new(120);
        let mut stalled = torrent("a", State::StalledDL, "tv");
        let now = run(&mut observations, 0, 31, &stalled);
        assert_eq!(observations.get("a").unwrap().stalled_for, 30 * 60);

        stalled.downloaded += 1;
        run(&mut observations, now, 1, &stalled);
        assert_eq!(observations.get("a").unwrap().stalled_for, 0);
    }

    #[test]
    fn a_restart_only_delays_the_clock() {
        let mut before = Observations::new(120);
        let stalled = torrent("a", State::StalledDL, "tv");
        run(&mut before, 0, 100, &stalled);
        let mut after = Observations::new(120);
        run(&mut after, 6_000, 1, &stalled);
        assert_eq!(after.get("a").unwrap().stalled_for, 0);
    }

    #[test]
    fn unobserved_gaps_count_at_most_the_cap() {
        let mut observations = Observations::new(120);
        let stalled = torrent("a", State::StalledDL, "tv");
        observations.observe(0, std::slice::from_ref(&stalled));
        observations.observe(3_600, std::slice::from_ref(&stalled));
        assert_eq!(observations.get("a").unwrap().stalled_for, 120);
    }

    #[test]
    fn torrents_that_vanish_are_forgotten() {
        let mut observations = Observations::new(120);
        observations.observe(0, &[torrent("a", State::StalledDL, "tv")]);
        observations.observe(60, &[]);
        assert!(observations.get("a").is_none());
    }

    #[test]
    fn progress_clears_recover_attempts() {
        let mut observations = Observations::new(120);
        let mut errored = torrent("a", State::Error, "tv");
        observations.observe(0, std::slice::from_ref(&errored));
        observations.note_recover_attempt("a");
        assert_eq!(observations.get("a").unwrap().recover_attempts, 1);
        errored.downloaded += 10;
        observations.observe(60, std::slice::from_ref(&errored));
        assert_eq!(observations.get("a").unwrap().recover_attempts, 0);
    }
}
