//! Input selection over the maker's tracked UTXOs. Only UTXOs with a known
//! leaf index are ever selected, since an input without one cannot be
//! proven. `select` and `width` run the same `pick`, so the width a quote
//! promises is the width the fill selects on the same inventory.

use std::cmp::Reverse;

use zolana_client::SPP_SUPPORTED_SHAPES;
use zolana_transaction::WalletUtxo;

use super::reservations::TrackedUtxo;

/// A tracked UTXO chosen as an input, with the leaf index it is proven at.
#[derive(Clone)]
pub struct SelectedInput {
    pub utxo: TrackedUtxo,
    pub leaf_index: u64,
}

impl SelectedInput {
    /// The wallet UTXO with `leaf_index` filled in, as the prover takes it.
    pub fn wallet(&self) -> WalletUtxo {
        let mut wallet = self.utxo.wallet.clone();
        wallet.leaf_index = self.leaf_index;
        wallet
    }
}

/// Selected inputs and their summed amount. `total` is always the exact sum
/// of the inputs' amounts: an input that would overflow it is not added.
#[derive(Clone, Default)]
pub struct Selection {
    pub inputs: Vec<SelectedInput>,
    pub total: u64,
}

impl Selection {
    /// A selection of `utxo` alone; `None` if its leaf index is unknown.
    pub fn single(utxo: &TrackedUtxo) -> Option<Self> {
        let mut selection = Self::default();
        selection.push(utxo).then_some(selection)
    }

    /// Adds `utxo` if its leaf index is known and its amount keeps `total`
    /// within `u64`; returns whether it did. The amounts of one mint never
    /// exceed its `u64` supply, so the overflow refusal does not occur in
    /// practice; it keeps `total` exact instead of saturating.
    fn push(&mut self, utxo: &TrackedUtxo) -> bool {
        let Some(leaf_index) = utxo.leaf_index else {
            return false;
        };
        let Some(total) = self.total.checked_add(utxo.amount()) else {
            return false;
        };
        self.total = total;
        self.inputs.push(SelectedInput {
            utxo: utxo.clone(),
            leaf_index,
        });
        true
    }

    /// The commitments of the selected inputs, in selection order.
    pub fn hashes(&self) -> Vec<[u8; 32]> {
        self.inputs
            .iter()
            .map(|input| input.utxo.utxo_hash())
            .collect()
    }
}

/// The most outputs any supported proof shape with at least `inputs` inputs
/// has; 0 when no shape is that wide. Ignores transaction size.
pub fn max_outputs_for(inputs: usize) -> usize {
    SPP_SUPPORTED_SHAPES
        .into_iter()
        .filter(|shape| shape.n_inputs() >= inputs)
        .map(|shape| shape.n_outputs())
        .max()
        .unwrap_or(0)
}

/// At most `max_inputs` provable UTXOs from `available` covering `amount`,
/// chosen by `pick`; `None` when no such selection exists.
pub fn select(available: &[TrackedUtxo], amount: u64, max_inputs: usize) -> Option<Selection> {
    let candidates: Vec<&TrackedUtxo> = available
        .iter()
        .filter(|utxo| utxo.leaf_index.is_some())
        .collect();
    let picked = pick(candidates, |utxo| utxo.amount(), amount, max_inputs)?;
    let mut selection = Selection::default();
    for utxo in picked {
        if !selection.push(utxo) {
            return None;
        }
    }
    Some(selection)
}

/// How many of `utxos` (amounts) `pick` would take to cover `amount`, or
/// `None`. The caller passes only provable UTXOs, as `select` would see them.
pub fn width(utxos: Vec<u64>, amount: u64, max_inputs: usize) -> Option<usize> {
    pick(utxos, |utxo| *utxo, amount, max_inputs).map(|picked| picked.len())
}

/// Greedy cover of `amount` with at most `max_inputs` candidates: at each
/// step it takes the smallest candidate that covers the remainder alone, or
/// the largest one if none does. This keeps large UTXOs for large fills and
/// the input count low. `None` when `max_inputs` are taken without covering,
/// or when the picked amounts overflow `u64` (so `select` and `width` agree
/// with the exact `Selection::total`).
fn pick<T>(
    mut candidates: Vec<T>,
    amount_of: impl Fn(&T) -> u64,
    amount: u64,
    max_inputs: usize,
) -> Option<Vec<T>> {
    candidates.sort_by_key(|candidate| Reverse(amount_of(candidate)));
    let mut picked = Vec::new();
    let mut total = 0u64;
    while picked.len() < max_inputs && !candidates.is_empty() {
        let remaining = amount.saturating_sub(total);
        let covering = candidates
            .iter()
            .rposition(|candidate| amount_of(candidate) >= remaining)
            .unwrap_or(0);
        let candidate = candidates.remove(covering);
        total = total.checked_add(amount_of(&candidate))?;
        picked.push(candidate);
        if total >= amount {
            return Some(picked);
        }
    }
    None
}

/// The `max_inputs` largest provable UTXOs of `available`.
pub fn select_all(available: &[TrackedUtxo], max_inputs: usize) -> Selection {
    let mut candidates: Vec<&TrackedUtxo> = available.iter().collect();
    candidates.sort_by_key(|utxo| Reverse(utxo.amount()));
    let mut selection = Selection::default();
    for utxo in candidates {
        if selection.inputs.len() >= max_inputs {
            break;
        }
        selection.push(utxo);
    }
    selection
}

/// The `count` smallest provable UTXOs of `available`.
pub fn select_smallest(available: &[TrackedUtxo], count: usize) -> Selection {
    let mut candidates: Vec<&TrackedUtxo> = available.iter().collect();
    candidates.sort_by_key(|utxo| utxo.amount());
    let mut selection = Selection::default();
    for utxo in candidates {
        if selection.inputs.len() >= count {
            break;
        }
        selection.push(utxo);
    }
    selection
}
