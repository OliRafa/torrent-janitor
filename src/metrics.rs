//! What Prometheus sees. Gauges describe the current cycle; counters count
//! decisions once each, so a dry-run torrent waiting for days counts once.

use prometheus_client::encoding::EncodeLabelSet;
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::registry::Registry;

use crate::executor::{Decision, Executed};
use crate::plan::Plan;
use crate::rules::SPACE_TAG;
use crate::snapshot::Snapshot;

#[derive(Debug, Clone, Hash, PartialEq, Eq, EncodeLabelSet)]
struct CycleLabels {
    outcome: &'static str,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, EncodeLabelSet)]
struct ReasonLabels {
    reason: &'static str,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, EncodeLabelSet)]
struct HeldLabels {
    reason: &'static str,
    hold: &'static str,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, EncodeLabelSet)]
struct RemovalLabels {
    reason: &'static str,
    outcome: &'static str,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, EncodeLabelSet)]
struct UpkeepLabels {
    action: &'static str,
    outcome: &'static str,
}

#[derive(Debug, Clone, Default)]
pub struct Metrics {
    cycles: Family<CycleLabels, Counter>,
    last_success: Gauge,
    managed: Gauge,
    free_space: Gauge,
    wanted: Family<ReasonLabels, Gauge>,
    held: Family<HeldLabels, Gauge>,
    removals: Family<RemovalLabels, Counter>,
    over_budget: Counter,
    upkeep: Family<UpkeepLabels, Counter>,
    space_stopped: Gauge,
    orphans: Gauge,
}

impl Metrics {
    pub fn new(dry_run: bool) -> (Metrics, Registry) {
        let metrics = Metrics::default();
        let mut registry = Registry::with_prefix("torrent_janitor");
        let dry: Gauge = Gauge::default();
        dry.set(i64::from(dry_run));
        registry.register(
            "dry_run",
            "1 while the janitor only logs what it would do",
            dry,
        );
        registry.register(
            "cycles",
            "Cycles run; skipped means a fetch failed and nothing was judged",
            metrics.cycles.clone(),
        );
        registry.register(
            "last_success_timestamp_seconds",
            "When the last complete cycle ran",
            metrics.last_success.clone(),
        );
        registry.register(
            "managed_torrents",
            "Torrents in managed categories",
            metrics.managed.clone(),
        );
        registry.register(
            "free_space_bytes",
            "Free space as qBittorrent reports it",
            metrics.free_space.clone(),
        );
        registry.register(
            "wanted_removals",
            "Torrents the rules want removed this cycle",
            metrics.wanted.clone(),
        );
        registry.register(
            "held_removals",
            "Wanted removals not made this cycle, by why",
            metrics.held.clone(),
        );
        registry.register(
            "removals",
            "Removal decisions, each counted once per torrent and outcome",
            metrics.removals.clone(),
        );
        registry.register(
            "over_budget_removals",
            "Removals that skipped the re-search because the item's budget was spent",
            metrics.over_budget.clone(),
        );
        registry.register(
            "upkeep",
            "Stops for space, starts once space is back and restarts of errored torrents, \
             each counted once per torrent and outcome",
            metrics.upkeep.clone(),
        );
        registry.register(
            "space_stopped_torrents",
            "Torrents stopped for space and not yet started again",
            metrics.space_stopped.clone(),
        );
        registry.register(
            "orphans",
            "Incomplete torrents in an arr category that its queue does not list",
            metrics.orphans.clone(),
        );
        (metrics, registry)
    }

    pub fn cycle(&self, now: i64, ok: bool) {
        let outcome = if ok { "ok" } else { "skipped" };
        self.cycles.get_or_create(&CycleLabels { outcome }).inc();
        if ok {
            self.last_success.set(now);
        }
    }

    pub fn record(&self, snapshot: &Snapshot, plan: &Plan, executed: &[Executed]) {
        self.managed.set(count(snapshot.torrents.len()));
        if let Some(free) = snapshot.free_space {
            self.free_space.set(i64::try_from(free).unwrap_or(i64::MAX));
        }
        self.wanted.clear();
        self.held.clear();
        let wanted = plan
            .removals
            .iter()
            .map(|p| p.reason)
            .chain(plan.held.iter().map(|(reason, _)| *reason));
        for reason in wanted {
            let reason = reason.as_str();
            self.wanted.get_or_create(&ReasonLabels { reason }).inc();
        }
        for (reason, hold) in &plan.held {
            let labels = HeldLabels {
                reason: reason.as_str(),
                hold: hold.as_str(),
            };
            self.held.get_or_create(&labels).inc();
        }
        let tagged = snapshot.torrents.iter().filter(|t| t.has_tag(SPACE_TAG));
        self.space_stopped.set(count(tagged.count()));
        self.orphans.set(count(plan.orphans.len()));
        for e in executed.iter().filter(|e| e.new) {
            let outcome = e.outcome.as_str();
            match e.decision {
                Decision::Remove(reason) => {
                    let reason = reason.as_str();
                    self.removals
                        .get_or_create(&RemovalLabels { reason, outcome })
                        .inc();
                    if e.over_budget {
                        self.over_budget.inc();
                    }
                }
                decision => {
                    let action = decision.action();
                    self.upkeep
                        .get_or_create(&UpkeepLabels { action, outcome })
                        .inc();
                }
            }
        }
    }
}

fn count(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use prometheus_client::encoding::text::encode;

    use super::*;
    use crate::executor::Outcome;
    use crate::plan::Hold;
    use crate::rules::Reason;

    fn text(registry: &Registry) -> String {
        let mut text = String::new();
        encode(&mut text, registry).unwrap();
        text
    }

    #[test]
    fn a_waiting_decision_counts_once_and_held_ones_show_why() {
        let (metrics, registry) = Metrics::new(true);
        let plan = Plan {
            held: vec![(Reason::Duplicate, Hold::Cap)],
            ..Plan::default()
        };
        let decision = |new| Executed {
            hash: "a".into(),
            decision: Decision::Remove(Reason::Stalled),
            outcome: Outcome::DryRun,
            over_budget: false,
            new,
        };
        metrics.record(&Snapshot::default(), &plan, &[decision(true)]);
        metrics.record(&Snapshot::default(), &plan, &[decision(false)]);
        let text = text(&registry);
        assert!(
            text.contains(
                r#"torrent_janitor_removals_total{reason="stalled",outcome="dry_run"} 1"#
            )
        );
        assert!(text.contains(r#"torrent_janitor_held_removals{reason="duplicate",hold="cap"} 1"#));
        assert!(text.contains("torrent_janitor_dry_run 1"));
    }

    #[test]
    fn last_cycle_gauges_forget_reasons_that_are_gone() {
        let (metrics, registry) = Metrics::new(false);
        let held = Plan {
            held: vec![(Reason::Stalled, Hold::Untracked)],
            ..Plan::default()
        };
        metrics.record(&Snapshot::default(), &held, &[]);
        metrics.record(&Snapshot::default(), &Plan::default(), &[]);
        assert!(!text(&registry).contains("hold=\"untracked\""));
    }

    #[test]
    fn upkeep_counts_apart_from_removals() {
        let (metrics, registry) = Metrics::new(false);
        let stop = Executed {
            hash: "a".into(),
            decision: Decision::Stop,
            outcome: Outcome::Done,
            over_budget: false,
            new: true,
        };
        metrics.record(&Snapshot::default(), &Plan::default(), &[stop]);
        let text = text(&registry);
        assert!(text.contains(r#"torrent_janitor_upkeep_total{action="stop",outcome="done"} 1"#));
        assert!(!text.contains("torrent_janitor_removals_total{"));
    }

    #[test]
    fn a_skipped_cycle_keeps_the_last_success_time() {
        let (metrics, registry) = Metrics::new(false);
        metrics.cycle(1_000, true);
        metrics.cycle(1_060, false);
        let text = text(&registry);
        assert!(text.contains("torrent_janitor_last_success_timestamp_seconds 1000"));
        assert!(text.contains(r#"torrent_janitor_cycles_total{outcome="skipped"} 1"#));
    }
}
