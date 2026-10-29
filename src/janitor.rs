//! One cycle, start to end: fetch, judge, act, record. A failed fetch skips
//! the whole cycle, since rules judging half a picture remove the wrong
//! things.

use std::collections::{HashMap, HashSet};

use serde::Deserialize;
use tracing::{info, warn};

use crate::api::{Arr, Qbit};
use crate::budget::Budget;
use crate::config::Config;
use crate::executor::{Decision, Executor, Outcome};
use crate::metrics::Metrics;
use crate::observe::Observations;
use crate::qbit::{File, Torrent};
use crate::rules::RuleSet;
use crate::snapshot::{ArrView, Snapshot};
use crate::{arr, error_chain, plan, qbit};

/// Quality profiles and the series/movie lists change rarely, and the lists
/// are large, so they are refetched on this period rather than every cycle.
const PROFILE_TTL: i64 = 15 * 60;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Qbit(#[from] qbit::Error),
    #[error(transparent)]
    Arr(#[from] arr::Error),
    #[error("cannot read free space from {path}")]
    FreeSpace {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

pub struct Janitor<Q, A> {
    config: Config,
    rules: RuleSet,
    qbit: Q,
    arrs: Vec<A>,
    views: Vec<ArrView>,
    profiles_fetched: Vec<Option<i64>>,
    /// File lists do not change once metadata is in, so each is fetched once.
    files: HashMap<String, Vec<File>>,
    observations: Observations,
    budget: Budget,
    executor: Executor,
    metrics: Metrics,
    failing: Option<String>,
}

impl<Q: Qbit, A: Arr> Janitor<Q, A> {
    pub fn new(config: Config, rules: RuleSet, qbit: Q, arrs: Vec<A>, metrics: Metrics) -> Self {
        let interval = config.interval.as_secs();
        Janitor {
            rules,
            qbit,
            views: vec![ArrView::default(); arrs.len()],
            profiles_fetched: vec![None; arrs.len()],
            arrs,
            files: HashMap::new(),
            observations: Observations::new(2 * interval),
            budget: Budget::new(config.rules.budget.max_researches_per_day),
            executor: Executor::new(config.dry_run),
            metrics,
            failing: None,
            config,
        }
    }

    pub async fn tick(&mut self, now: i64) {
        let result = self.cycle(now).await;
        self.metrics.cycle(now, result.is_ok());
        match result {
            Ok(()) if self.failing.take().is_some() => info!("fetches recovered"),
            Ok(()) => {}
            Err(error) => {
                let error = error_chain(&error);
                if self.failing.as_ref() != Some(&error) {
                    warn!(%error, "cycle skipped: a fetch failed");
                }
                self.failing = Some(error);
            }
        }
    }

    async fn cycle(&mut self, now: i64) -> Result<(), Error> {
        let main = self.qbit.main_data().await?;
        let free_space = match &self.config.rules.free_space.file {
            Some(path) => Some(read_free_space(path)?),
            None => main.free_space,
        };
        let managed: Vec<Torrent> = main
            .torrents
            .into_iter()
            .filter(|t| self.config.manages(&t.category))
            .collect();
        self.fetch_files(&managed).await?;
        for i in 0..self.arrs.len() {
            self.fetch_queue(i, now).await?;
        }

        let snapshot = Snapshot::build(
            &self.config,
            now,
            managed,
            free_space,
            self.files.clone(),
            &self.views,
        );
        self.observations.observe(now, &snapshot.torrents);
        let removals = self.rules.removals(&snapshot, &self.observations);
        let upkeep = self.rules.upkeep(&snapshot, &self.observations);
        let mut plan = plan::plan(&self.config, &snapshot, &self.budget, removals);
        plan.add_upkeep(&snapshot, upkeep);
        let executed = self
            .executor
            .execute(&plan, now, &self.qbit, &self.arrs, &mut self.budget)
            .await;
        for e in &executed {
            if (e.decision, e.outcome) == (Decision::Recover, Outcome::Done) {
                self.observations.note_recover_attempt(&e.hash);
            }
        }
        self.metrics.record(&snapshot, &plan, &executed);
        Ok(())
    }

    async fn fetch_files(&mut self, torrents: &[Torrent]) -> Result<(), qbit::Error> {
        let present: HashSet<&str> = torrents.iter().map(|t| t.hash.as_str()).collect();
        self.files.retain(|hash, _| present.contains(hash.as_str()));
        for torrent in torrents {
            if torrent.state.is_metadata() || self.files.contains_key(&torrent.hash) {
                continue;
            }
            let files = self.qbit.files(&torrent.hash).await?;
            if !files.is_empty() {
                self.files.insert(torrent.hash.clone(), files);
            }
        }
        Ok(())
    }

    async fn fetch_queue(&mut self, i: usize, now: i64) -> Result<(), arr::Error> {
        let arr = &self.arrs[i];
        let view = &mut self.views[i];
        view.records = arr.queue().await?;
        if self.profiles_fetched[i].is_none_or(|at| now - at >= PROFILE_TTL) {
            view.profiles = arr.quality_profiles().await?;
            view.owners = arr.profiles_by_owner().await?;
            self.profiles_fetched[i] = Some(now);
        }
        Ok(())
    }
}

fn read_free_space(path: &str) -> Result<u64, Error> {
    #[derive(Deserialize)]
    struct Volume {
        avail_bytes: u64,
    }
    let read = || -> std::io::Result<u64> {
        let text = std::fs::read_to_string(path)?;
        let volume: Volume = serde_json::from_str(&text).map_err(std::io::Error::other)?;
        Ok(volume.avail_bytes)
    };
    read().map_err(|source| Error::FreeSpace {
        path: path.to_owned(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arr::Kind;
    use crate::qbit::State;
    use crate::testkit::{FakeArr, FakeQbit, config, file, record, torrent};

    fn janitor(dry_run: bool, torrents: Vec<Torrent>) -> Janitor<FakeQbit, FakeArr> {
        let mut config = config();
        config.dry_run = dry_run;
        let rules = RuleSet::new(&config.rules).unwrap();
        let qbit = FakeQbit::default();
        *qbit.torrents.borrow_mut() = torrents;
        let arrs = vec![FakeArr::new(Kind::Sonarr), FakeArr::new(Kind::Radarr)];
        Janitor::new(config, rules, qbit, arrs, Metrics::default())
    }

    fn sonarr(j: &Janitor<FakeQbit, FakeArr>) -> &FakeArr {
        &j.arrs[0]
    }

    fn with_fake(j: &Janitor<FakeQbit, FakeArr>, hash: &str) {
        j.qbit
            .files
            .borrow_mut()
            .insert(hash.to_owned(), vec![file("Example.S01E01.exe", 0)]);
    }

    #[tokio::test]
    async fn a_live_cycle_removes_a_fake_through_its_arr() {
        let mut j = janitor(false, vec![torrent("ab", State::StalledDL, "tv-sonarr")]);
        with_fake(&j, "ab");
        sonarr(&j).records.borrow_mut().push(record(5, "AB", 101));
        j.tick(0).await;
        assert_eq!(*sonarr(&j).deletes.borrow(), [(5, true, false)]);
    }

    #[tokio::test]
    async fn a_dry_run_cycle_removes_nothing() {
        let mut j = janitor(true, vec![torrent("ab", State::StalledDL, "tv-sonarr")]);
        with_fake(&j, "ab");
        sonarr(&j).records.borrow_mut().push(record(5, "AB", 101));
        j.tick(0).await;
        assert!(sonarr(&j).deletes.borrow().is_empty());
    }

    #[tokio::test]
    async fn a_failed_fetch_skips_the_whole_cycle() {
        let mut j = janitor(false, vec![torrent("cd", State::StalledDL, "comics")]);
        with_fake(&j, "cd");
        j.arrs[1].down.set(true);
        assert!(j.cycle(0).await.is_err());
        assert!(j.qbit.deleted.borrow().is_empty());

        j.arrs[1].down.set(false);
        j.tick(60).await;
        assert_eq!(*j.qbit.deleted.borrow(), ["cd"]);
    }

    #[tokio::test]
    async fn file_lists_are_fetched_once_metadata_is_in() {
        let mut j = janitor(true, vec![torrent("ab", State::MetaDL, "tv-sonarr")]);
        j.tick(0).await;
        assert_eq!(j.qbit.file_fetches.get(), 0);

        j.qbit.torrents.borrow_mut()[0].state = State::StalledDL;
        j.qbit
            .files
            .borrow_mut()
            .insert("ab".into(), vec![file("Example.S01E01.mkv", 1)]);
        j.tick(60).await;
        j.tick(120).await;
        assert_eq!(j.qbit.file_fetches.get(), 1);
    }

    #[tokio::test]
    async fn unmanaged_torrents_are_never_looked_at() {
        let mut j = janitor(false, vec![torrent("ef", State::StalledDL, "games")]);
        with_fake(&j, "ef");
        j.tick(0).await;
        assert_eq!(j.qbit.file_fetches.get(), 0);
        assert!(j.qbit.deleted.borrow().is_empty());
    }

    #[tokio::test]
    async fn profiles_are_refetched_only_after_their_ttl() {
        let mut j = janitor(true, vec![]);
        j.tick(0).await;
        j.tick(60).await;
        assert_eq!(sonarr(&j).profile_fetches.get(), 1);
        j.tick(PROFILE_TTL).await;
        assert_eq!(sonarr(&j).profile_fetches.get(), 2);
    }

    #[tokio::test]
    async fn a_full_disk_stops_downloads_until_space_is_back() {
        let mut j = janitor(false, vec![torrent("ab", State::Downloading, "tv-sonarr")]);
        sonarr(&j).records.borrow_mut().push(record(5, "AB", 101));
        j.qbit.free_space.set(Some(1 << 30));
        j.tick(0).await;
        assert_eq!(*j.qbit.stopped.borrow(), ["ab"]);

        j.qbit.free_space.set(Some(40 << 30));
        j.tick(60).await;
        assert!(
            j.qbit.started.borrow().is_empty(),
            "inside the hysteresis band"
        );

        j.qbit.free_space.set(Some(60 << 30));
        j.tick(120).await;
        assert_eq!(*j.qbit.started.borrow(), ["ab"]);
        assert!(j.qbit.torrents.borrow()[0].tags.is_empty());
    }

    #[tokio::test]
    async fn free_space_comes_from_the_volume_file_when_one_is_set() {
        let path = std::env::temp_dir().join(format!("tj-volume-{}.json", std::process::id()));
        std::fs::write(
            &path,
            r#"{"quota_bytes":3285649981440,"avail_bytes":204800}"#,
        )
        .unwrap();
        let mut j = janitor(false, vec![torrent("ab", State::Downloading, "tv-sonarr")]);
        sonarr(&j).records.borrow_mut().push(record(5, "AB", 101));
        j.config.rules.free_space.file = Some(path.to_string_lossy().into_owned());
        j.qbit.free_space.set(Some(3 << 40));
        j.tick(0).await;
        std::fs::remove_file(&path).unwrap();
        assert_eq!(*j.qbit.stopped.borrow(), ["ab"], "qBit's 3 TiB is the pool");

        assert!(matches!(j.cycle(60).await, Err(Error::FreeSpace { .. })));
    }

    #[tokio::test]
    async fn an_errored_torrent_gets_a_limited_number_of_restarts() {
        let mut j = janitor(false, vec![torrent("ab", State::Error, "tv-sonarr")]);
        sonarr(&j).records.borrow_mut().push(record(5, "AB", 101));
        j.qbit.free_space.set(Some(60 << 30));
        for minute in 0..10 {
            j.qbit.torrents.borrow_mut()[0].state = State::Error;
            j.tick(minute * 60).await;
        }
        assert_eq!(j.qbit.started.borrow().len(), 3);
    }

    #[tokio::test]
    async fn a_stalled_download_goes_after_three_hours_of_cycles() {
        let mut j = janitor(false, vec![torrent("ab", State::StalledDL, "tv-sonarr")]);
        j.qbit
            .files
            .borrow_mut()
            .insert("ab".into(), vec![file("Example.S01E01.mkv", 1)]);
        sonarr(&j).records.borrow_mut().push(record(5, "AB", 101));
        for minute in 0..180 {
            j.tick(minute * 60).await;
        }
        assert!(sonarr(&j).deletes.borrow().is_empty());
        j.tick(180 * 60).await;
        assert_eq!(*sonarr(&j).deletes.borrow(), [(5, true, false)]);
    }
}
