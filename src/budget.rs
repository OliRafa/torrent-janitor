//! How often one episode or movie may be searched again after a removal.
//! Without a cap, a release that keeps failing turns into a grab loop (one
//! episode was grabbed 66 times). Beyond the cap a removal still blocklists
//! but skips the re-search. It lives in memory, so a restart refills it.

use std::collections::HashMap;

const DAY: i64 = 24 * 60 * 60;

#[derive(Debug, Default)]
pub struct Budget {
    per_day: usize,
    spent: HashMap<(usize, i64), Vec<i64>>,
}

impl Budget {
    pub fn new(per_day: u32) -> Self {
        Budget {
            per_day: per_day as usize,
            spent: HashMap::new(),
        }
    }

    /// Whether every item still has a re-search left in the last 24 hours.
    /// Checking spends nothing, so dry-run can ask every cycle.
    pub fn allows(&self, now: i64, arr: usize, items: &[i64]) -> bool {
        items.iter().all(|item| {
            let used = self
                .spent
                .get(&(arr, *item))
                .map_or(0, |times| times.iter().filter(|&&t| now - t < DAY).count());
            used < self.per_day
        })
    }

    pub fn spend(&mut self, now: i64, arr: usize, items: &[i64]) {
        self.spent.retain(|_, times| {
            times.retain(|&t| now - t < DAY);
            !times.is_empty()
        });
        for item in items {
            self.spent.entry((arr, *item)).or_default().push(now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_third_re_search_in_a_day_is_refused() {
        let mut budget = Budget::new(2);
        assert!(budget.allows(0, 0, &[101]));
        budget.spend(0, 0, &[101]);
        budget.spend(3_600, 0, &[101]);
        assert!(!budget.allows(7_200, 0, &[101]));
    }

    #[test]
    fn the_budget_refills_after_a_day() {
        let mut budget = Budget::new(1);
        budget.spend(0, 0, &[101]);
        assert!(!budget.allows(DAY - 1, 0, &[101]));
        assert!(budget.allows(DAY, 0, &[101]));
    }

    #[test]
    fn a_pack_is_refused_when_any_episode_is_spent() {
        let mut budget = Budget::new(1);
        budget.spend(0, 0, &[102]);
        assert!(!budget.allows(60, 0, &[101, 102, 103]));
        assert!(budget.allows(60, 0, &[101, 103]));
    }

    #[test]
    fn asking_does_not_spend() {
        let budget = Budget::new(1);
        for minute in 0..100 {
            assert!(budget.allows(minute * 60, 0, &[101]));
        }
    }

    #[test]
    fn each_arr_has_its_own_items() {
        let mut budget = Budget::new(1);
        budget.spend(0, 0, &[101]);
        assert!(budget.allows(60, 1, &[101]));
    }

    #[test]
    fn a_zero_budget_never_re_searches() {
        assert!(!Budget::new(0).allows(0, 0, &[101]));
    }
}
