//! Carries out a plan, or in dry-run only says what it would do. Each
//! decision is logged once, when it is first made or its outcome changes,
//! not every cycle it stays true.

use std::collections::HashSet;
use std::error::Error as StdError;
use std::future::Future;

use tracing::{info, warn};

use crate::api::{Arr, Qbit};
use crate::budget::Budget;
use crate::plan::{Named, Plan, Planned, Route};
use crate::rules::{Reason, SPACE_TAG};
use crate::{arr, error_chain, qbit};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Qbit(#[from] qbit::Error),
    #[error(transparent)]
    Arr(#[from] arr::Error),
    #[error("the plan routes through arr #{0}, which is not configured")]
    MissingArr(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Decision {
    Remove(Reason),
    Stop,
    Start,
    Recover,
    Orphan,
}

impl Decision {
    pub fn action(self) -> &'static str {
        match self {
            Decision::Remove(_) => "remove",
            Decision::Stop => "stop",
            Decision::Start => "start",
            Decision::Recover => "recover",
            Decision::Orphan => "orphan",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Outcome {
    DryRun,
    Done,
    Failed,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::DryRun => "dry_run",
            Outcome::Done => "done",
            Outcome::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Executed {
    pub hash: String,
    pub decision: Decision,
    pub outcome: Outcome,
    pub over_budget: bool,
    /// First time this decision and outcome come up for the torrent. Metrics
    /// count only these, so dry-run does not count a torrent every cycle.
    pub new: bool,
}

type Announced = HashSet<(String, Decision, Outcome)>;

#[derive(Debug, Default)]
pub struct Executor {
    dry_run: bool,
    announced: Announced,
}

impl Executor {
    pub fn new(dry_run: bool) -> Self {
        Executor {
            dry_run,
            announced: HashSet::new(),
        }
    }

    pub async fn execute<Q: Qbit, A: Arr>(
        &mut self,
        plan: &Plan,
        now: i64,
        qbit: &Q,
        arrs: &[A],
        budget: &mut Budget,
    ) -> Vec<Executed> {
        let mut current = Announced::new();
        let mut executed = Vec::new();

        for planned in &plan.removals {
            let failure = self.unless_dry_run(remove(planned, qbit, arrs)).await;
            let outcome = self.outcome(failure.is_some());
            if let (Outcome::Done, Route::Arr { arr, items, .. }) = (outcome, &planned.route)
                && planned.research
            {
                budget.spend(now, *arr, items);
            }
            let decision = Decision::Remove(planned.reason);
            let new = self.mark(&mut current, &planned.hash, decision, outcome);
            if new {
                let via = via(&planned.route, arrs);
                log_removal(planned, outcome, &via, failure.as_ref().map(as_dyn));
            }
            executed.push(Executed {
                hash: planned.hash.clone(),
                decision,
                outcome,
                over_budget: planned.over_budget,
                new,
            });
        }

        let stopped = self.unless_dry_run(stop(qbit, &plan.stop)).await;
        self.report(
            &mut current,
            &mut executed,
            &plan.stop,
            Decision::Stop,
            stopped,
        );
        let started = self.unless_dry_run(start(qbit, &plan.start)).await;
        self.report(
            &mut current,
            &mut executed,
            &plan.start,
            Decision::Start,
            started,
        );
        let recovered = self.unless_dry_run(recover(qbit, &plan.recover)).await;
        self.report(
            &mut current,
            &mut executed,
            &plan.recover,
            Decision::Recover,
            recovered,
        );

        for orphan in &plan.orphans {
            if self.mark(&mut current, &orphan.hash, Decision::Orphan, Outcome::Done) {
                info!(
                    hash = %orphan.hash, name = %orphan.name,
                    "incomplete and not in its arr's queue; left alone"
                );
            }
        }

        // A torrent that drops out of the plan and comes back later, say
        // stalled again after some progress, is a new decision.
        self.announced = current;
        executed
    }

    /// Runs `action` unless this is a dry run; the future is never polled
    /// in dry-run, so nothing reaches the network.
    async fn unless_dry_run<E>(&self, action: impl Future<Output = Result<(), E>>) -> Option<E> {
        if self.dry_run {
            None
        } else {
            action.await.err()
        }
    }

    fn outcome(&self, failed: bool) -> Outcome {
        match (self.dry_run, failed) {
            (true, _) => Outcome::DryRun,
            (false, false) => Outcome::Done,
            (false, true) => Outcome::Failed,
        }
    }

    fn mark(
        &self,
        current: &mut Announced,
        hash: &str,
        decision: Decision,
        outcome: Outcome,
    ) -> bool {
        let key = (hash.to_owned(), decision, outcome);
        let new = !self.announced.contains(&key);
        current.insert(key);
        new
    }

    fn report(
        &self,
        current: &mut Announced,
        executed: &mut Vec<Executed>,
        torrents: &[Named],
        decision: Decision,
        failure: Option<qbit::Error>,
    ) {
        let outcome = self.outcome(failure.is_some());
        for torrent in torrents {
            let new = self.mark(current, &torrent.hash, decision, outcome);
            if new {
                log_upkeep(torrent, decision, outcome, failure.as_ref().map(as_dyn));
            }
            executed.push(Executed {
                hash: torrent.hash.clone(),
                decision,
                outcome,
                over_budget: false,
                new,
            });
        }
    }
}

async fn remove<Q: Qbit, A: Arr>(planned: &Planned, qbit: &Q, arrs: &[A]) -> Result<(), Error> {
    match &planned.route {
        Route::Arr { arr, queue_id, .. } => {
            let client = arrs.get(*arr).ok_or(Error::MissingArr(*arr))?;
            client
                .delete_queue_item(*queue_id, planned.blocklist, !planned.research)
                .await?;
        }
        Route::Qbit => qbit.delete(std::slice::from_ref(&planned.hash)).await?,
    }
    Ok(())
}

/// Tags first: if the stop then fails, the next cycle stops them again.
async fn stop<Q: Qbit>(qbit: &Q, torrents: &[Named]) -> Result<(), qbit::Error> {
    if torrents.is_empty() {
        return Ok(());
    }
    let hashes = hashes(torrents);
    qbit.add_tag(&hashes, SPACE_TAG).await?;
    qbit.stop(&hashes).await
}

/// Untags last: if the start fails, the tag keeps them in line for another try.
async fn start<Q: Qbit>(qbit: &Q, torrents: &[Named]) -> Result<(), qbit::Error> {
    if torrents.is_empty() {
        return Ok(());
    }
    let hashes = hashes(torrents);
    qbit.start(&hashes).await?;
    qbit.remove_tag(&hashes, SPACE_TAG).await
}

async fn recover<Q: Qbit>(qbit: &Q, torrents: &[Named]) -> Result<(), qbit::Error> {
    if torrents.is_empty() {
        return Ok(());
    }
    qbit.start(&hashes(torrents)).await
}

fn hashes(torrents: &[Named]) -> Vec<String> {
    torrents.iter().map(|t| t.hash.clone()).collect()
}

fn as_dyn<E: StdError + 'static>(error: &E) -> &(dyn StdError + 'static) {
    error
}

fn via<A: Arr>(route: &Route, arrs: &[A]) -> String {
    match route {
        Route::Arr { arr, .. } => arrs
            .get(*arr)
            .map_or_else(|| format!("arr #{arr}"), |a| format!("{:?}", a.kind())),
        Route::Qbit => "qBittorrent".to_owned(),
    }
}

fn log_removal(p: &Planned, outcome: Outcome, via: &str, failure: Option<&dyn StdError>) {
    let reason = p.reason.as_str();
    if let Some(error) = failure {
        warn!(
            hash = %p.hash, name = %p.name, reason, detail = %p.detail, via,
            error = %error_chain(error),
            "removal failed"
        );
        return;
    }
    let what = match outcome {
        Outcome::DryRun => "would remove",
        _ => "removed",
    };
    info!(
        hash = %p.hash, name = %p.name, reason, detail = %p.detail, via,
        blocklist = p.blocklist, research = p.research, over_budget = p.over_budget,
        "{what}"
    );
}

fn log_upkeep(t: &Named, decision: Decision, outcome: Outcome, failure: Option<&dyn StdError>) {
    let action = decision.action();
    if let Some(error) = failure {
        warn!(
            hash = %t.hash, name = %t.name, action,
            error = %error_chain(error),
            "upkeep failed"
        );
        return;
    }
    let what = match (decision, outcome) {
        (Decision::Stop, Outcome::DryRun) => "would stop: free space below the low mark",
        (Decision::Stop, _) => "stopped: free space below the low mark",
        (Decision::Start, Outcome::DryRun) => "would start: free space is back",
        (Decision::Start, _) => "started: free space is back",
        (_, Outcome::DryRun) => "would restart the errored torrent",
        _ => "restarted the errored torrent",
    };
    info!(hash = %t.hash, name = %t.name, action, "{what}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arr::Kind;
    use crate::qbit::State;
    use crate::testkit::{FakeArr, FakeQbit, record, torrent};

    fn planned(hash: &str, route: Route, research: bool) -> Planned {
        Planned {
            hash: hash.to_owned(),
            name: format!("Example.{hash}"),
            reason: Reason::Stalled,
            detail: "no progress for 3h00m of active time".into(),
            route,
            blocklist: true,
            research,
            over_budget: false,
        }
    }

    fn named(hash: &str) -> Named {
        Named {
            hash: hash.to_owned(),
            name: format!("Example.{hash}"),
        }
    }

    fn via_sonarr(queue_id: i64) -> Route {
        Route::Arr {
            arr: 0,
            queue_id,
            items: vec![101],
        }
    }

    fn sonarr() -> FakeArr {
        let arr = FakeArr::new(Kind::Sonarr);
        arr.records.borrow_mut().push(record(5, "AB", 101));
        arr
    }

    fn removing(planned: Planned) -> Plan {
        Plan {
            removals: vec![planned],
            ..Plan::default()
        }
    }

    #[tokio::test]
    async fn dry_run_changes_nothing_and_announces_once() {
        let (qbit, arrs) = (FakeQbit::default(), [sonarr()]);
        let mut plan = removing(planned("ab", via_sonarr(5), true));
        plan.stop = vec![named("cd")];
        let mut executor = Executor::new(true);
        let mut budget = Budget::new(2);
        let first = executor.execute(&plan, 0, &qbit, &arrs, &mut budget).await;
        let second = executor.execute(&plan, 60, &qbit, &arrs, &mut budget).await;
        assert!(first.iter().all(|e| e.new && e.outcome == Outcome::DryRun));
        assert!(second.iter().all(|e| !e.new));
        assert!(arrs[0].deletes.borrow().is_empty());
        assert!(qbit.stopped.borrow().is_empty());
        assert!(budget.allows(60, 0, &[101]), "dry-run spends no budget");
    }

    #[tokio::test]
    async fn a_live_removal_goes_through_the_arr_and_spends_the_budget() {
        let (qbit, arrs) = (FakeQbit::default(), [sonarr()]);
        let plan = removing(planned("ab", via_sonarr(5), true));
        let mut budget = Budget::new(1);
        let done = Executor::new(false)
            .execute(&plan, 0, &qbit, &arrs, &mut budget)
            .await;
        assert_eq!(done[0].outcome, Outcome::Done);
        assert_eq!(*arrs[0].deletes.borrow(), [(5, true, false)]);
        assert!(!budget.allows(60, 0, &[101]));
    }

    #[tokio::test]
    async fn a_removal_without_re_search_tells_the_arr_to_skip_it() {
        let (qbit, arrs) = (FakeQbit::default(), [sonarr()]);
        let plan = removing(planned("ab", via_sonarr(5), false));
        let mut budget = Budget::new(1);
        Executor::new(false)
            .execute(&plan, 0, &qbit, &arrs, &mut budget)
            .await;
        assert_eq!(*arrs[0].deletes.borrow(), [(5, true, true)]);
        assert!(budget.allows(60, 0, &[101]));
    }

    #[tokio::test]
    async fn a_qbit_only_torrent_is_deleted_in_qbit() {
        let qbit = FakeQbit::default();
        let arrs: [FakeArr; 0] = [];
        let plan = removing(planned("cd", Route::Qbit, false));
        Executor::new(false)
            .execute(&plan, 0, &qbit, &arrs, &mut Budget::new(1))
            .await;
        assert_eq!(*qbit.deleted.borrow(), ["cd"]);
    }

    #[tokio::test]
    async fn a_failed_removal_spends_nothing_and_is_retried() {
        let (qbit, arrs) = (FakeQbit::default(), [sonarr()]);
        arrs[0].down.set(true);
        let plan = removing(planned("ab", via_sonarr(5), true));
        let mut executor = Executor::new(false);
        let mut budget = Budget::new(1);
        let failed = executor.execute(&plan, 0, &qbit, &arrs, &mut budget).await;
        assert_eq!(failed[0].outcome, Outcome::Failed);
        assert!(budget.allows(0, 0, &[101]));

        arrs[0].down.set(false);
        let retried = executor.execute(&plan, 60, &qbit, &arrs, &mut budget).await;
        assert_eq!(retried[0].outcome, Outcome::Done);
        assert!(retried[0].new, "a changed outcome is a new decision");
    }

    #[tokio::test]
    async fn a_decision_that_lapses_is_announced_again() {
        let (qbit, arrs) = (FakeQbit::default(), [sonarr()]);
        let plan = removing(planned("ab", via_sonarr(5), true));
        let mut executor = Executor::new(true);
        let mut budget = Budget::new(2);
        executor.execute(&plan, 0, &qbit, &arrs, &mut budget).await;
        executor
            .execute(&Plan::default(), 60, &qbit, &arrs, &mut budget)
            .await;
        let again = executor
            .execute(&plan, 120, &qbit, &arrs, &mut budget)
            .await;
        assert!(again[0].new);
    }

    #[tokio::test]
    async fn stopping_for_space_tags_and_starting_again_untags() {
        let qbit = FakeQbit::default();
        *qbit.torrents.borrow_mut() = vec![torrent("a", State::Downloading, "tv-sonarr")];
        let arrs: [FakeArr; 0] = [];
        let mut executor = Executor::new(false);
        let mut budget = Budget::new(1);
        let stop = Plan {
            stop: vec![named("a")],
            ..Plan::default()
        };
        executor.execute(&stop, 0, &qbit, &arrs, &mut budget).await;
        assert_eq!(qbit.torrents.borrow()[0].state, State::StoppedDL);
        assert!(qbit.torrents.borrow()[0].has_tag(SPACE_TAG));

        let start = Plan {
            start: vec![named("a")],
            ..Plan::default()
        };
        executor
            .execute(&start, 60, &qbit, &arrs, &mut budget)
            .await;
        assert_eq!(qbit.torrents.borrow()[0].state, State::Downloading);
        assert!(!qbit.torrents.borrow()[0].has_tag(SPACE_TAG));
    }

    #[tokio::test]
    async fn an_errored_torrent_is_restarted_without_a_tag() {
        let qbit = FakeQbit::default();
        *qbit.torrents.borrow_mut() = vec![torrent("e", State::Error, "tv-sonarr")];
        let arrs: [FakeArr; 0] = [];
        let plan = Plan {
            recover: vec![named("e")],
            ..Plan::default()
        };
        let done = Executor::new(false)
            .execute(&plan, 0, &qbit, &arrs, &mut Budget::new(1))
            .await;
        assert_eq!(*qbit.started.borrow(), ["e"]);
        assert_eq!(done[0].decision, Decision::Recover);
        assert!(qbit.torrents.borrow()[0].tags.is_empty());
    }
}
