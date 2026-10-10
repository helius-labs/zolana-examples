//! The UTXO denomination profile of one asset: the set of UTXO amounts the
//! maker aims to hold for a given balance. Change outputs and upkeep
//! consolidations are split toward these targets, so the inventory keeps
//! UTXOs of the sizes fills need without growing without bound.

use crate::error::MakerError;

/// Target UTXO amounts for a balance: one large UTXO of `balance / d` per
/// divisor `d` in `large`, then the rest in up to `small` equal parts, no
/// part below `min_utxo_value`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InventoryProfile {
    /// Divisors of the balance, one large target each; a target is skipped
    /// when it is below `min_utxo_value` or not below what is left.
    pub large: Vec<u64>,
    /// Number of equal parts the rest is split into, reduced so no part is
    /// below `min_utxo_value`, and at least one.
    pub small: usize,
    /// Smallest UTXO the profile creates; a smaller remainder is merged into
    /// the last part.
    pub min_utxo_value: u64,
}

impl InventoryProfile {
    /// `utxos` equal parts, no large targets.
    pub fn equal(utxos: usize, min_utxo_value: u64) -> Self {
        Self {
            large: Vec::new(),
            small: utxos,
            min_utxo_value,
        }
    }

    /// The most targets the profile produces for any balance.
    pub fn count(&self) -> usize {
        self.large.len().saturating_add(self.small.max(1))
    }

    /// The target amounts for `balance`, largest first; they sum to
    /// `balance`. Empty for a zero balance.
    pub fn targets(&self, balance: u64) -> Vec<u64> {
        if balance == 0 {
            return Vec::new();
        }
        let min = self.min_utxo_value.max(1);
        let mut targets = Vec::with_capacity(self.count());
        let mut rest = balance;
        for divisor in &self.large {
            let amount = balance / (*divisor).max(1);
            if amount < min {
                continue;
            }
            // `None` when `amount` is not below what is left.
            if let Some(left) = rest.checked_sub(amount).filter(|left| *left > 0) {
                targets.push(amount);
                rest = left;
            }
        }
        let by_value = usize::try_from(rest / min).unwrap_or(usize::MAX);
        let small = self.small.min(by_value).max(1);
        targets.extend(equal_parts(rest, small));
        targets.sort_by_key(|amount| std::cmp::Reverse(*amount));
        targets
    }

    /// The targets for `balance` that no UTXO in `utxos` is matched to,
    /// largest first (matching as in `match_utxos`).
    pub fn missing(&self, balance: u64, utxos: &[u64]) -> Vec<u64> {
        let targets = self.targets(balance);
        unmatched(&targets, &match_utxos(&targets, utxos))
    }

    /// The number of UTXOs in `utxos` matched to no target for `balance`:
    /// how many a consolidation should merge away.
    pub fn surplus_utxos(&self, balance: u64, utxos: &[u64]) -> usize {
        let targets = self.targets(balance);
        match_utxos(&targets, utxos)
            .utxos
            .iter()
            .filter(|matched| matched.is_none())
            .count()
    }

    /// How to split `value` into at most `max_parts` outputs so that, next
    /// to the existing `others`, the inventory moves toward the profile: the
    /// missing targets largest first while they fit, the rest as the last
    /// part. Empty for a zero `value`.
    ///
    /// If `others` plus `value` overflow `u64` (they cannot for UTXOs of one
    /// mint, whose supply is a `u64`), there is no balance to take targets
    /// from and `value` stays one part.
    pub fn parts(&self, value: u64, others: &[u64], max_parts: usize) -> Vec<u64> {
        let missing = checked_sum(others)
            .and_then(|total| total.checked_add(value))
            .map(|balance| self.missing(balance, others))
            .unwrap_or_default();
        self.fill(value, missing, max_parts)
    }

    /// The UTXO of `utxos` holding the most above its matched target, and
    /// how to split it into its target plus the missing targets, in at most
    /// `max_parts` parts. `Ok(None)` when no UTXO is above its target or
    /// the split yields fewer than two parts; `MakerError::AmountOverflow`
    /// when `utxos` sum above `u64::MAX`.
    pub fn split(
        &self,
        utxos: &[u64],
        max_parts: usize,
    ) -> Result<Option<(usize, Vec<u64>)>, MakerError> {
        let balance = checked_sum(utxos).ok_or(MakerError::AmountOverflow {
            context: "profile split balance",
        })?;
        let targets = self.targets(balance);
        let matching = match_utxos(&targets, utxos);
        let Some((index, amount, own)) = utxos
            .iter()
            .zip(&matching.utxos)
            .enumerate()
            .filter_map(|(index, (amount, target))| {
                let own = target.unwrap_or(0);
                let above = amount.checked_sub(own).filter(|above| *above > 0)?;
                Some((index, *amount, own, above))
            })
            .max_by_key(|(_, _, _, above)| *above)
            .map(|(index, amount, own, _)| (index, amount, own))
        else {
            return Ok(None);
        };
        let wanted = (own > 0)
            .then_some(own)
            .into_iter()
            .chain(unmatched(&targets, &matching))
            .collect();
        let parts = self.fill(amount, wanted, max_parts);
        Ok((parts.len() >= 2).then_some((index, parts)))
    }

    /// Greedily takes `wanted` amounts out of `value`, leaving room for the
    /// remainder as the last of at most `max_parts` parts; a remainder below
    /// `min_utxo_value` is merged into the previous part instead.
    fn fill(&self, value: u64, wanted: Vec<u64>, max_parts: usize) -> Vec<u64> {
        if value == 0 {
            return Vec::new();
        }
        let mut parts = Vec::new();
        let mut rest = value;
        for target in wanted {
            if parts.len().saturating_add(1) >= max_parts.max(1) {
                break;
            }
            if let Some(left) = rest.checked_sub(target) {
                parts.push(target);
                rest = left;
            }
        }
        // The parts so far sum to `value - rest`, so adding `rest` to one of
        // them cannot overflow.
        let mut merged = false;
        if let Some(last) = parts.last_mut().filter(|_| rest < self.min_utxo_value) {
            if let Some(sum) = last.checked_add(rest) {
                *last = sum;
                merged = true;
            }
        }
        if !merged && rest > 0 {
            parts.push(rest);
        }
        parts
    }
}

/// The result of `match_utxos`, indexed like its inputs.
struct Matching {
    /// Whether each target got a UTXO.
    targets: Vec<bool>,
    /// The target each UTXO was matched to.
    utxos: Vec<Option<u64>>,
}

fn unmatched(targets: &[u64], matching: &Matching) -> Vec<u64> {
    let mut missing: Vec<u64> = targets
        .iter()
        .zip(&matching.targets)
        .filter(|(_, matched)| !**matched)
        .map(|(target, _)| *target)
        .collect();
    missing.sort_by_key(|amount| std::cmp::Reverse(*amount));
    missing
}

/// One-to-one greedy matching of UTXOs to targets, closest pairs first by
/// `distance`.
fn match_utxos(targets: &[u64], utxos: &[u64]) -> Matching {
    let mut pairs: Vec<(f64, usize, usize)> = targets
        .iter()
        .enumerate()
        .flat_map(|(target_index, target)| {
            utxos
                .iter()
                .enumerate()
                .map(move |(utxo_index, utxo)| (distance(*target, *utxo), target_index, utxo_index))
        })
        .collect();
    pairs.sort_by(|left, right| left.0.total_cmp(&right.0));
    let mut matching = Matching {
        targets: vec![false; targets.len()],
        utxos: vec![None; utxos.len()],
    };
    for (_, target_index, utxo_index) in pairs {
        let free = matching.targets.get(target_index) == Some(&false)
            && matching.utxos.get(utxo_index) == Some(&None);
        if !free {
            continue;
        }
        if let (Some(target_matched), Some(utxo), Some(target)) = (
            matching.targets.get_mut(target_index),
            matching.utxos.get_mut(utxo_index),
            targets.get(target_index),
        ) {
            *target_matched = true;
            *utxo = Some(*target);
        }
    }
    matching
}

/// Distance on a log scale, so a UTXO twice its target is as far off as one
/// half of it, at any denomination.
fn distance(target: u64, utxo: u64) -> f64 {
    let target = (target.max(1) as f64).ln();
    let utxo = (utxo.max(1) as f64).ln();
    (target - utxo).abs()
}

/// `amount` split into `parts` near-equal parts (the last takes the
/// rounding remainder), never more parts than `amount` so none is zero.
pub fn equal_parts(amount: u64, parts: usize) -> Vec<u64> {
    let count = u64::try_from(parts.max(1)).unwrap_or(1).min(amount.max(1));
    // `count >= 1`, and `part * count + rest == amount`, so `part + rest`
    // fits; the fallbacks keep `amount` whole and are not reached.
    let (Some(part), Some(rest)) = (amount.checked_div(count), amount.checked_rem(count)) else {
        return vec![amount];
    };
    let Some(last) = part.checked_add(rest) else {
        return vec![amount];
    };
    (1..count)
        .map(|_| part)
        .chain(std::iter::once(last))
        .collect()
}

/// The sum of `amounts`, `None` above `u64::MAX`.
fn checked_sum(amounts: &[u64]) -> Option<u64> {
    amounts
        .iter()
        .try_fold(0u64, |total, amount| total.checked_add(*amount))
}
