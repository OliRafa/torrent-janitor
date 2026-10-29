//! Turns what the rules want into what one cycle does: one removal per
//! torrent, routed through its arr when it has one, a re-search only within
//! the budget, and no more removals than the per-cycle cap.

use std::collections::{BTreeMap, HashSet};

use crate::budget::Budget;
use crate::config::Config;
use crate::rules::{Reason, Removal, Upkeep};
use crate::snapshot::Snapshot;

#[derive(Debug, Clone, PartialEq)]
pub enum Route {
    /// `DELETE /api/v3/queue/{id}`: the arr removes the torrent, and
    /// blocklists and searches again as asked.
    Arr {
        arr: usize,
        queue_id: i64,
        items: Vec<i64>,
    },
    /// Straight to qBittorrent, data included, for categories without an arr.
    Qbit,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Planned {
    pub hash: String,
    pub name: String,
    pub reason: Reason,
    pub detail: String,
    pub route: Route,
    pub blocklist: bool,
    pub research: bool,
    /// The rule wanted a re-search the budget had no room for.
    pub over_budget: bool,
}

/// Why a removal the rules want does not happen this cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Hold {
    /// Over `max_removals_per_cycle`; it comes up again next cycle. Live
    /// only: dry-run plans every removal, so its log shows all of them.
    Cap,
    /// In an arr's category but not in its queue. Removing it behind the
    /// arr's back would blocklist nothing, so it is only reported.
    Untracked,
}

impl Hold {
    pub fn as_str(self) -> &'static str {
        match self {
            Hold::Cap => "cap",
            Hold::Untracked => "untracked",
        }
    }
}

/// A torrent an upkeep action touches, named for the log.
#[derive(Debug, Clone, PartialEq)]
pub struct Named {
    pub hash: String,
    pub name: String,
}

#[derive(Debug, Default, PartialEq)]
pub struct Plan {
    pub removals: Vec<Planned>,
    pub held: Vec<(Reason, Hold)>,
    pub stop: Vec<Named>,
    pub start: Vec<Named>,
    pub recover: Vec<Named>,
    pub orphans: Vec<Named>,
}

impl Plan {
    /// Adds the upkeep, minus torrents this cycle removes: stopping or
    /// starting a torrent on its way out only races the removal.
    pub fn add_upkeep(&mut self, snapshot: &Snapshot, upkeep: Upkeep) {
        let removed: HashSet<&str> = self.removals.iter().map(|r| r.hash.as_str()).collect();
        let named = |hashes: Vec<String>| -> Vec<Named> {
            hashes
                .into_iter()
                .filter(|hash| !removed.contains(hash.as_str()))
                .filter_map(|hash| {
                    let name = snapshot.torrent(&hash)?.name.clone();
                    Some(Named { hash, name })
                })
                .collect()
        };
        self.stop = named(upkeep.stop);
        self.start = named(upkeep.start);
        self.recover = named(upkeep.recover);
        self.orphans = named(upkeep.orphans);
    }
}

pub fn plan(config: &Config, snapshot: &Snapshot, budget: &Budget, removals: Vec<Removal>) -> Plan {
    let mut strongest: BTreeMap<String, Removal> = BTreeMap::new();
    for removal in removals {
        match strongest.get(&removal.hash) {
            Some(kept) if kept.reason <= removal.reason => {}
            _ => {
                strongest.insert(removal.hash.clone(), removal);
            }
        }
    }
    let mut ordered: Vec<Removal> = strongest.into_values().collect();
    ordered.sort_by(|a, b| (a.reason, &a.hash).cmp(&(b.reason, &b.hash)));

    let mut plan = Plan::default();
    // One re-search covers an item however many of its downloads go.
    let mut researched: HashSet<(usize, i64)> = HashSet::new();
    for removal in ordered {
        let Some(torrent) = snapshot.torrent(&removal.hash) else {
            continue;
        };
        let route = match snapshot.download(&removal.hash) {
            Some(d) => Route::Arr {
                arr: d.arr,
                queue_id: d.queue_id,
                items: d.items.clone(),
            },
            None if config.owner(&torrent.category).is_none() => Route::Qbit,
            None => {
                plan.held.push((removal.reason, Hold::Untracked));
                continue;
            }
        };
        if !config.dry_run && plan.removals.len() >= config.max_removals_per_cycle {
            plan.held.push((removal.reason, Hold::Cap));
            continue;
        }
        let (blocklist, research, over_budget) = match &route {
            Route::Arr { arr, items, .. } if removal.research => {
                let fresh: Vec<i64> = items
                    .iter()
                    .copied()
                    .filter(|item| !researched.contains(&(*arr, *item)))
                    .collect();
                let allowed = !fresh.is_empty() && budget.allows(snapshot.now, *arr, &fresh);
                if allowed {
                    researched.extend(fresh.iter().map(|item| (*arr, *item)));
                }
                (removal.blocklist, allowed, !allowed && !fresh.is_empty())
            }
            Route::Arr { .. } => (removal.blocklist, false, false),
            Route::Qbit => (false, false, false),
        };
        plan.removals.push(Planned {
            hash: removal.hash,
            name: torrent.name.clone(),
            reason: removal.reason,
            detail: removal.detail,
            route,
            blocklist,
            research,
            over_budget,
        });
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::qbit::State;
    use crate::testkit::{config, download, torrent};

    fn removal(hash: &str, reason: Reason) -> Removal {
        Removal {
            hash: hash.to_owned(),
            reason,
            blocklist: reason != Reason::Duplicate,
            research: reason != Reason::Duplicate,
            detail: reason.as_str().to_owned(),
        }
    }

    fn snapshot(hashes: &[&str]) -> Snapshot {
        Snapshot {
            torrents: hashes
                .iter()
                .map(|h| torrent(h, State::StalledDL, "tv-sonarr"))
                .collect(),
            downloads: hashes
                .iter()
                .map(|h| download(h, "downloading", &[]))
                .collect(),
            ..Snapshot::default()
        }
    }

    #[test]
    fn a_torrent_two_rules_pick_is_removed_once_for_the_stronger_reason() {
        let plan = plan(
            &config(),
            &snapshot(&["a"]),
            &Budget::new(2),
            vec![removal("a", Reason::Duplicate), removal("a", Reason::Fake)],
        );
        assert_eq!(plan.removals.len(), 1);
        assert_eq!(plan.removals[0].reason, Reason::Fake);
        assert!(plan.removals[0].blocklist);
    }

    #[test]
    fn an_arr_download_goes_through_its_queue() {
        let plan = plan(
            &config(),
            &snapshot(&["a"]),
            &Budget::new(2),
            vec![removal("a", Reason::Stalled)],
        );
        assert_eq!(
            plan.removals[0].route,
            Route::Arr {
                arr: 0,
                queue_id: 1,
                items: vec![101]
            }
        );
        assert!(plan.removals[0].research);
    }

    #[test]
    fn a_qbit_only_torrent_is_deleted_directly_without_blocklist() {
        let snapshot = Snapshot {
            torrents: vec![torrent("a", State::StalledDL, "comics")],
            ..Snapshot::default()
        };
        let plan = plan(
            &config(),
            &snapshot,
            &Budget::new(2),
            vec![removal("a", Reason::Fake)],
        );
        assert_eq!(plan.removals[0].route, Route::Qbit);
        assert!(!plan.removals[0].blocklist && !plan.removals[0].research);
    }

    #[test]
    fn a_torrent_its_arr_does_not_track_is_held() {
        let snapshot = Snapshot {
            torrents: vec![torrent("a", State::StalledDL, "tv-sonarr")],
            ..Snapshot::default()
        };
        let plan = plan(
            &config(),
            &snapshot,
            &Budget::new(2),
            vec![removal("a", Reason::Stalled)],
        );
        assert!(plan.removals.is_empty());
        assert_eq!(plan.held, [(Reason::Stalled, Hold::Untracked)]);
    }

    #[test]
    fn removals_past_the_cap_wait_strongest_first() {
        let mut config = config();
        config.dry_run = false;
        config.max_removals_per_cycle = 2;
        let plan = plan(
            &config,
            &snapshot(&["a", "b", "c"]),
            &Budget::new(2),
            vec![
                removal("a", Reason::Duplicate),
                removal("b", Reason::Stalled),
                removal("c", Reason::Fake),
            ],
        );
        let reasons: Vec<Reason> = plan.removals.iter().map(|r| r.reason).collect();
        assert_eq!(reasons, [Reason::Fake, Reason::Stalled]);
        assert_eq!(plan.held, [(Reason::Duplicate, Hold::Cap)]);
    }

    #[test]
    fn dry_run_plans_past_the_cap() {
        let mut config = config();
        config.max_removals_per_cycle = 1;
        let plan = plan(
            &config,
            &snapshot(&["a", "b"]),
            &Budget::new(2),
            vec![removal("a", Reason::Stalled), removal("b", Reason::Stalled)],
        );
        assert_eq!(plan.removals.len(), 2);
        assert!(plan.held.is_empty());
    }

    #[test]
    fn a_spent_budget_still_blocklists_but_skips_the_re_search() {
        let mut budget = Budget::new(1);
        budget.spend(0, 0, &[101]);
        let plan = plan(
            &config(),
            &snapshot(&["a"]),
            &budget,
            vec![removal("a", Reason::Stalled)],
        );
        let planned = &plan.removals[0];
        assert!(planned.blocklist);
        assert!(!planned.research);
        assert!(planned.over_budget);
    }

    #[test]
    fn two_removals_of_one_episode_search_for_it_once() {
        let plan = plan(
            &config(),
            &snapshot(&["a", "b"]),
            &Budget::new(2),
            vec![removal("a", Reason::Stalled), removal("b", Reason::Stalled)],
        );
        assert!(plan.removals[0].research);
        assert!(!plan.removals[1].research);
        assert!(!plan.removals[1].over_budget);
    }

    #[test]
    fn upkeep_skips_torrents_this_cycle_removes() {
        let snapshot = snapshot(&["a", "b"]);
        let mut plan = plan(
            &config(),
            &snapshot,
            &Budget::new(2),
            vec![removal("a", Reason::Stalled)],
        );
        plan.add_upkeep(
            &snapshot,
            Upkeep {
                stop: vec!["a".into(), "b".into()],
                ..Upkeep::default()
            },
        );
        let stopped: Vec<&str> = plan.stop.iter().map(|n| n.hash.as_str()).collect();
        assert_eq!(stopped, ["b"]);
        assert_eq!(plan.stop[0].name, "Example.b");
    }

    #[test]
    fn a_duplicate_never_asks_for_a_re_search() {
        let plan = plan(
            &config(),
            &snapshot(&["a"]),
            &Budget::new(2),
            vec![removal("a", Reason::Duplicate)],
        );
        assert!(!plan.removals[0].research && !plan.removals[0].over_budget);
    }
}
