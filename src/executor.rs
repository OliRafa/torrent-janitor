//! Carries out a plan, or in dry-run only says what it would do. Each
//! decision is logged once, when it is first made or its outcome changes,
//! not every cycle it stays true.

use std::collections::HashMap;

use tracing::{info, warn};

use crate::api::{Arr, Qbit};
use crate::budget::Budget;
use crate::plan::{Plan, Planned, Route};
use crate::rules::Reason;
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
pub enum Outcome {
    WouldRemove,
    Removed,
    Failed,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::WouldRemove => "would_remove",
            Outcome::Removed => "removed",
            Outcome::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Executed {
    pub reason: Reason,
    pub outcome: Outcome,
    pub over_budget: bool,
    /// First time this decision is made for the torrent. Metrics count only
    /// these, so dry-run does not count a torrent again every cycle.
    pub new: bool,
}

#[derive(Debug, Default)]
pub struct Executor {
    dry_run: bool,
    announced: HashMap<String, (Reason, Outcome)>,
}

impl Executor {
    pub fn new(dry_run: bool) -> Self {
        Executor {
            dry_run,
            announced: HashMap::new(),
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
        let mut executed = Vec::with_capacity(plan.removals.len());
        for planned in &plan.removals {
            let failure = if self.dry_run {
                None
            } else {
                remove(planned, qbit, arrs).await.err()
            };
            let outcome = match (self.dry_run, &failure) {
                (true, _) => Outcome::WouldRemove,
                (false, None) => Outcome::Removed,
                (false, Some(_)) => Outcome::Failed,
            };
            if let (Outcome::Removed, Route::Arr { arr, items, .. }) = (outcome, &planned.route)
                && planned.research
            {
                budget.spend(now, *arr, items);
            }
            let decision = (planned.reason, outcome);
            let new = self.announced.insert(planned.hash.clone(), decision) != Some(decision);
            if new {
                log(
                    planned,
                    outcome,
                    &via(&planned.route, arrs),
                    failure.as_ref(),
                );
            }
            executed.push(Executed {
                reason: planned.reason,
                outcome,
                over_budget: planned.over_budget,
                new,
            });
        }
        // A torrent that drops out of the plan and comes back later, say
        // stalled again after some progress, is a new decision.
        self.announced
            .retain(|hash, _| plan.removals.iter().any(|p| &p.hash == hash));
        executed
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

fn via<A: Arr>(route: &Route, arrs: &[A]) -> String {
    match route {
        Route::Arr { arr, .. } => arrs
            .get(*arr)
            .map_or_else(|| format!("arr #{arr}"), |a| format!("{:?}", a.kind())),
        Route::Qbit => "qBittorrent".to_owned(),
    }
}

fn log(p: &Planned, outcome: Outcome, via: &str, failure: Option<&Error>) {
    let reason = p.reason.as_str();
    match (outcome, failure) {
        (Outcome::Failed, Some(error)) => warn!(
            hash = %p.hash, name = %p.name, reason, detail = %p.detail, via,
            error = %error_chain(error),
            "removal failed"
        ),
        (Outcome::Removed, _) => info!(
            hash = %p.hash, name = %p.name, reason, detail = %p.detail, via,
            blocklist = p.blocklist, research = p.research, over_budget = p.over_budget,
            "removed"
        ),
        _ => info!(
            hash = %p.hash, name = %p.name, reason, detail = %p.detail, via,
            blocklist = p.blocklist, research = p.research, over_budget = p.over_budget,
            "would remove"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arr::Kind;
    use crate::testkit::{FakeArr, FakeQbit, record};

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

    #[tokio::test]
    async fn dry_run_changes_nothing_and_announces_once() {
        let (qbit, arrs) = (FakeQbit::default(), [sonarr()]);
        let plan = Plan {
            removals: vec![planned("ab", via_sonarr(5), true)],
            ..Plan::default()
        };
        let mut executor = Executor::new(true);
        let mut budget = Budget::new(2);
        let first = executor.execute(&plan, 0, &qbit, &arrs, &mut budget).await;
        let second = executor.execute(&plan, 60, &qbit, &arrs, &mut budget).await;
        assert_eq!(first[0].outcome, Outcome::WouldRemove);
        assert!(first[0].new);
        assert!(!second[0].new);
        assert!(arrs[0].deletes.borrow().is_empty());
        assert!(budget.allows(60, 0, &[101]), "dry-run spends no budget");
    }

    #[tokio::test]
    async fn a_live_removal_goes_through_the_arr_and_spends_the_budget() {
        let (qbit, arrs) = (FakeQbit::default(), [sonarr()]);
        let plan = Plan {
            removals: vec![planned("ab", via_sonarr(5), true)],
            ..Plan::default()
        };
        let mut budget = Budget::new(1);
        let done = Executor::new(false)
            .execute(&plan, 0, &qbit, &arrs, &mut budget)
            .await;
        assert_eq!(done[0].outcome, Outcome::Removed);
        assert_eq!(*arrs[0].deletes.borrow(), [(5, true, false)]);
        assert!(!budget.allows(60, 0, &[101]));
    }

    #[tokio::test]
    async fn a_removal_without_re_search_tells_the_arr_to_skip_it() {
        let (qbit, arrs) = (FakeQbit::default(), [sonarr()]);
        let plan = Plan {
            removals: vec![planned("ab", via_sonarr(5), false)],
            ..Plan::default()
        };
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
        let plan = Plan {
            removals: vec![planned("cd", Route::Qbit, false)],
            ..Plan::default()
        };
        Executor::new(false)
            .execute(&plan, 0, &qbit, &arrs, &mut Budget::new(1))
            .await;
        assert_eq!(*qbit.deleted.borrow(), ["cd"]);
    }

    #[tokio::test]
    async fn a_failed_removal_spends_nothing_and_is_retried() {
        let (qbit, arrs) = (FakeQbit::default(), [sonarr()]);
        arrs[0].down.set(true);
        let plan = Plan {
            removals: vec![planned("ab", via_sonarr(5), true)],
            ..Plan::default()
        };
        let mut executor = Executor::new(false);
        let mut budget = Budget::new(1);
        let failed = executor.execute(&plan, 0, &qbit, &arrs, &mut budget).await;
        assert_eq!(failed[0].outcome, Outcome::Failed);
        assert!(budget.allows(0, 0, &[101]));

        arrs[0].down.set(false);
        let retried = executor.execute(&plan, 60, &qbit, &arrs, &mut budget).await;
        assert_eq!(retried[0].outcome, Outcome::Removed);
        assert!(retried[0].new, "a changed outcome is a new decision");
    }

    #[tokio::test]
    async fn a_decision_that_lapses_is_announced_again() {
        let (qbit, arrs) = (FakeQbit::default(), [sonarr()]);
        let plan = Plan {
            removals: vec![planned("ab", via_sonarr(5), true)],
            ..Plan::default()
        };
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
}
