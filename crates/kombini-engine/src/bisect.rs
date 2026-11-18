//! "Find the broken mod" by halving the enabled set.

use crate::{ChangeReport, Engine, NoProgress, Result};
use kombini_core::{AppError, ModId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const BISECT_KEY: &str = "bisect_state";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bisect {
    /// Enabled mods (in order) before bisecting; restored at the end.
    pub original_enabled: Vec<ModId>,
    pub suspects: Vec<ModId>,
    /// Mods enabled for the current test round.
    pub testing: Vec<ModId>,
    pub round: u32,
    /// The problem was last *observed* with exactly `suspects` (vs inferred by elimination).
    #[serde(default)]
    pub confirmed: bool,
    pub result: Option<BisectResult>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BisectResult {
    Found {
        mod_id: ModId,
    },
    /// The problem needs more than one mod together (or is not mod related).
    Inconclusive,
}

impl Bisect {
    pub fn start(enabled: Vec<ModId>) -> Option<Self> {
        if enabled.is_empty() {
            return None;
        }
        let mut b = Self {
            original_enabled: enabled.clone(),
            suspects: enabled,
            testing: Vec::new(),
            round: 0,
            confirmed: true,
            result: None,
        };
        b.next_round();
        Some(b)
    }

    /// Like [`Bisect::start`], but the first round tests exactly the
    /// `likely` mods (outdated or with known issues). If the problem shows
    /// up with them, the search continues inside that small set; if not,
    /// they are cleared at once. Falls back to halving when `likely` is
    /// empty or covers every enabled mod.
    pub fn start_prioritized(enabled: Vec<ModId>, likely: &BTreeSet<ModId>) -> Option<Self> {
        let (mut first, rest): (Vec<ModId>, Vec<ModId>) =
            enabled.iter().cloned().partition(|id| likely.contains(id));
        if first.is_empty() || rest.is_empty() {
            return Self::start(enabled);
        }
        let n = first.len();
        first.extend(rest);
        Some(Self {
            original_enabled: enabled,
            testing: first[..n].to_vec(),
            suspects: first,
            round: 1,
            confirmed: true,
            result: None,
        })
    }

    fn next_round(&mut self) {
        if self.suspects.len() == 1 && self.confirmed {
            self.result = Some(BisectResult::Found {
                mod_id: self.suspects[0].clone(),
            });
            self.testing = self.suspects.clone();
            return;
        }
        if self.suspects.is_empty() {
            self.result = Some(BisectResult::Inconclusive);
            self.testing.clear();
            return;
        }
        // A lone suspect reached by elimination gets one confirmation round.
        let half = if self.suspects.len() == 1 {
            1
        } else {
            self.suspects.len().div_ceil(2)
        };
        self.testing = self.suspects[..half].to_vec();
        self.round += 1;
    }

    /// Records whether the problem happened with `testing` enabled.
    pub fn report(&mut self, problem: bool) {
        if self.result.is_some() {
            return;
        }
        self.confirmed = problem;
        if problem {
            self.suspects = self.testing.clone();
        } else {
            let tested: BTreeSet<&ModId> = self.testing.iter().collect();
            self.suspects.retain(|m| !tested.contains(m));
        }
        self.next_round();
    }

    /// Upper bound of remaining rounds.
    pub fn rounds_left(&self) -> u32 {
        if self.result.is_some() {
            0
        } else {
            usize::BITS - self.suspects.len().saturating_sub(1).leading_zeros() + 1
        }
    }
}

impl Engine {
    pub fn bisect_state(&self) -> Option<Bisect> {
        self.db.lock().get_setting(BISECT_KEY).ok().flatten()
    }

    fn save_bisect(&self, b: Option<&Bisect>) -> Result<()> {
        let db = self.db.lock();
        match b {
            Some(b) => db.set_setting(BISECT_KEY, b)?,
            None => db.set_setting(BISECT_KEY, &serde_json::Value::Null)?,
        }
        Ok(())
    }

    fn apply_enabled_set(&self, ids: &[ModId], summary: String) -> Result<ChangeReport> {
        let ids: BTreeSet<ModId> = ids.iter().cloned().collect();
        self.mutate("bisect", &NoProgress, move |state| {
            let existing: BTreeSet<ModId> = state.mods.keys().cloned().collect();
            state.profile_mut().enabled = ids.intersection(&existing).cloned().collect();
            Ok(summary)
        })
    }

    pub fn bisect_start(&self) -> Result<Bisect> {
        self.bisect_start_with(&BTreeSet::new())
    }

    /// Starts a search that tests the `likely` mods first.
    pub fn bisect_start_with(&self, likely: &BTreeSet<ModId>) -> Result<Bisect> {
        if self.bisect_state().is_some() {
            return Err(AppError::invalid("A search is already running"));
        }
        let b = Bisect::start_prioritized(self.load_order(), likely)
            .ok_or_else(|| AppError::invalid("Enable some mods first"))?;
        self.apply_enabled_set(
            &b.testing,
            format!("Testing {} of {} mods", b.testing.len(), b.suspects.len()),
        )?;
        self.save_bisect(Some(&b))?;
        Ok(b)
    }

    pub fn bisect_report(&self, problem: bool) -> Result<Bisect> {
        let mut b = self
            .bisect_state()
            .ok_or_else(|| AppError::invalid("No search is running"))?;
        b.report(problem);
        let summary = match &b.result {
            Some(BisectResult::Found { mod_id }) => format!("Found the problem mod: {mod_id}"),
            Some(BisectResult::Inconclusive) => "No single mod causes the problem".to_owned(),
            None => format!("Testing {} mods", b.testing.len()),
        };
        self.apply_enabled_set(&b.testing, summary)?;
        self.save_bisect(Some(&b))?;
        Ok(b)
    }

    /// Ends the search and restores the original enabled mods.
    pub fn bisect_finish(&self) -> Result<ChangeReport> {
        let b = self
            .bisect_state()
            .ok_or_else(|| AppError::invalid("No search is running"))?;
        let report = self.apply_enabled_set(&b.original_enabled, "Restored your mods".into())?;
        self.save_bisect(None)?;
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: usize) -> Vec<ModId> {
        (0..n).map(|i| ModId::new(format!("m{i}"))).collect()
    }

    /// Simulates a user whose problem is caused by `culprit`.
    fn run(n: usize, culprit: Option<usize>) -> (BisectResult, u32) {
        let mut b = Bisect::start(ids(n)).unwrap();
        let mut rounds = 0;
        while b.result.is_none() {
            let problem = culprit.is_some_and(|c| b.testing.contains(&ModId::new(format!("m{c}"))));
            b.report(problem);
            rounds += 1;
            assert!(rounds < 64);
        }
        (b.result.unwrap(), rounds)
    }

    #[test]
    fn finds_every_culprit_in_log_rounds() {
        for n in [1, 2, 3, 7, 16, 33] {
            for c in 0..n {
                let (result, rounds) = run(n, Some(c));
                assert_eq!(
                    result,
                    BisectResult::Found {
                        mod_id: ModId::new(format!("m{c}"))
                    }
                );
                assert!(
                    rounds <= (n as f64).log2().ceil() as u32 + 1,
                    "n={n} rounds={rounds}"
                );
            }
        }
    }

    #[test]
    fn likely_mods_are_tested_first() {
        let all = ids(10);
        let likely: BTreeSet<ModId> = [all[7].clone(), all[8].clone()].into();
        // Culprit among the likely ones: found in 2 more rounds.
        let mut b = Bisect::start_prioritized(all.clone(), &likely).unwrap();
        assert_eq!(b.testing, vec![all[7].clone(), all[8].clone()]);
        b.report(true);
        assert_eq!(b.testing, vec![all[7].clone()]);
        b.report(false);
        // all[8] by elimination gets a confirmation round.
        assert_eq!(b.testing, vec![all[8].clone()]);
        b.report(true);
        assert_eq!(
            b.result,
            Some(BisectResult::Found {
                mod_id: all[8].clone()
            })
        );
        // Culprit elsewhere: the likely ones are cleared in one round.
        let mut b = Bisect::start_prioritized(all.clone(), &likely).unwrap();
        b.report(false);
        assert_eq!(b.suspects.len(), 8);
        assert!(!b.suspects.contains(&all[7]));
        // Nothing likely: plain halving.
        let b = Bisect::start_prioritized(all.clone(), &BTreeSet::new()).unwrap();
        assert_eq!(b.testing.len(), 5);
    }

    #[test]
    fn inconclusive_without_culprit() {
        assert_eq!(run(5, None).0, BisectResult::Inconclusive);
    }
}
