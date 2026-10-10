//! Shielding: moving tokens from the market maker's public accounts into its
//! shielded inventory, and the tail of a rebalance (kVault instruction plus
//! shield of its proceeds). A tail shields what a simulation shows the
//! vault paid, never the preview, so a vault price that drifted between
//! quote and execution cannot make the shield exceed the account balance.

use solana_address::Address;
use solana_instruction::Instruction;
use zolana_client::ComputeBudgetConfig;
use zolana_interface::pda;
use zolana_program::instruction::{AssetDeposit, Deposit, DepositAsset, DepositSplAccounts};

use k_lend_rfq_sdk::swap::FULL_BPS;

use crate::{
    config::Settings,
    error::MarketMakerError,
    inventory::balance::{profile::InventoryProfile, reservations::Reservations},
    transactions::{
        budget::MAX_COMPUTE_UNITS,
        send::{SendQueue, SendRequest},
        steps::TailShield,
        Identity,
    },
};

/// Compute units of a consolidate transaction, and of a seed transaction
/// that only shields: the Solana maximum, since one transact proof
/// verification dominates and unused units are not charged.
pub const REBALANCE_COMPUTE_BUDGET: ComputeBudgetConfig =
    ComputeBudgetConfig::new(MAX_COMPUTE_UNITS);

/// Compute units of a transaction carrying a kVault deposit or withdraw on a
/// vault without allocated reserves: the transact that pays the market maker's
/// account (proof verification), the kVault instruction and the shield. On
/// localnet such a rebalance simulates at about 240_000 units without the
/// shield, and a seed's deposit plus shield at about 112_000; the base keeps
/// a wide margin for wider transact shapes.
const VAULT_BASE_COMPUTE_UNITS: u32 = 700_000;

/// Compute units a kVault deposit or withdraw adds per allocated reserve:
/// the program refreshes every reserve itself through klend
/// `RefreshReservesBatch` before pricing. Measured by the `mainnet_vault`
/// test on a snapshot of a mainnet vault with 3 reserves: the deposit alone
/// consumed 129_812 units, of which the klend refresh CPI took 33_125 (about
/// 11_000 per reserve), against about 72_000 for a deposit into a vault
/// without reserves on localnet, so about 19_000 per reserve in total; 50_000
/// keeps a margin over both (`market-market_maker/tests/mainnet_vault.rs`).
/// `VAULT_BASE_COMPUTE_UNITS` plus this times the reserve count must stay
/// under `MAX_COMPUTE_UNITS` for the vaults the market maker serves: with these
/// values `(1_400_000 - 700_000) / 50_000`, up to 14 reserves.
const RESERVE_REFRESH_COMPUTE_UNITS: u32 = 50_000;

/// The compute units a transaction carrying a kVault instruction over
/// `reserves` allocated reserves requests:
/// `VAULT_BASE_COMPUTE_UNITS + RESERVE_REFRESH_COMPUTE_UNITS * reserves`,
/// capped at `MAX_COMPUTE_UNITS`, the most a transaction can request. Past
/// the cap the request may be short; the tail's simulation
/// (`TailShield::resolve`) runs with the same budget, so a vault whose
/// instruction does not fit fails there with `SimulationFailed`, before
/// anything is sent.
pub fn vault_compute_units(reserves: usize) -> u32 {
    u32::try_from(reserves)
        .ok()
        .and_then(|reserves| RESERVE_REFRESH_COMPUTE_UNITS.checked_mul(reserves))
        .and_then(|refresh| VAULT_BASE_COMPUTE_UNITS.checked_add(refresh))
        .map_or(MAX_COMPUTE_UNITS, |units| units.min(MAX_COMPUTE_UNITS))
}

/// Basis points of a simulated kVault payout `delta` the tail does not shield,
/// the shield margin `bps_of(delta, SHIELD_MARGIN_BPS)`. It covers the interest
/// an invested vault accrues between the simulation and the execution, which
/// lowers the shares a deposit mints; a shield of the full simulated amount
/// would then exceed the account balance and fail the transaction. The residual
/// stays in the market maker's account and the next tail sweeps it
/// (`TailShield::before`, read when that operation is scheduled).
pub const SHIELD_MARGIN_BPS: u64 = 1;
/// Basis points of a tail's own payout `delta` up to which it sweeps the
/// balance that was in the market maker's account before the operation, the
/// sweep cap `bps_of(delta, SWEEP_CAP_BPS)`. Ten times the margin a single tail
/// leaves, so the residual of an earlier tail is cleared even after a few
/// skipped sweeps. Anything above the cap is not dust but the operator's public
/// float (for example the collateral a withdraw pays into) and stays in the
/// account.
pub const SWEEP_CAP_BPS: u64 = 10;

/// `max(1, floor(delta / FULL_BPS) * bps)`.
fn bps_of(delta: u64, bps: u64) -> u64 {
    (delta / FULL_BPS).saturating_mul(bps).max(1)
}

/// How to split a shielded amount of one asset into UTXOs: toward its
/// profile, next to the UTXOs already held, in at most `max_utxos` parts.
#[derive(Clone, Debug)]
pub struct ShieldPlan {
    pub profile: InventoryProfile,
    /// Amounts of every tracked UTXO of the asset.
    pub utxos: Vec<u64>,
    pub max_utxos: usize,
}

impl ShieldPlan {
    /// The plan for `asset` from the current settings and inventory.
    pub fn new(config: &Settings, reservations: &Reservations, asset: &Address) -> Self {
        Self {
            profile: config.profile(asset).clone(),
            utxos: reservations
                .utxos(asset)
                .into_iter()
                .map(|utxo| utxo.amount)
                .collect(),
            max_utxos: config.max_shield_utxos,
        }
    }

    /// The UTXO amounts to shield `amount` as; they sum to `amount`.
    pub fn amounts(&self, amount: u64) -> Vec<u64> {
        self.profile.parts(amount, &self.utxos, self.max_utxos)
    }
}

impl Identity {
    /// A zolana `deposit` of `utxos`, one per `(mint, amount)`, from the
    /// market maker's public accounts to new UTXOs of its own shielded address
    /// in its tree, tagged with its viewing key so sync finds them. Errors with
    /// `MarketMakerError::ShieldInstruction` when the zolana builder rejects
    /// it.
    pub fn shield(&self, utxos: Vec<(Address, u64)>) -> Result<Instruction, MarketMakerError> {
        let owner = self.own.owner_hash()?;
        let view_tag = self.own.viewing_pubkey.x();
        Deposit {
            tree: self.tree,
            depositor: self.payer,
            deposits: utxos
                .iter()
                .map(|(mint, amount)| AssetDeposit {
                    asset: DepositAsset::Spl(DepositSplAccounts {
                        mint: *mint,
                        user_token: pda::associated_token_address(&self.payer, mint),
                        token_program: pda::spl_token_program_id(),
                    }),
                    view_tag,
                    owner,
                    amount: *amount,
                    memo: None,
                })
                .collect(),
        }
        .instruction()
        .map_err(|error| MarketMakerError::ShieldInstruction(error.to_string()))
    }
}

impl TailShield {
    /// The kVault instruction followed by one shield instruction with
    /// `amount` of `asset`, split by `plan`, and the `also_shield` UTXOs.
    pub fn instructions(
        &self,
        identity: &Identity,
        amount: u64,
    ) -> Result<Vec<Instruction>, MarketMakerError> {
        self.with_shield(identity, self.plan.amounts(amount))
    }

    /// The widest tail [`TailShield::resolve`] can return: the kVault
    /// instruction and a shield of `plan.max_utxos` parts (at least one,
    /// the most `ShieldPlan::amounts` splits into) plus `also_shield`. A
    /// transaction sized with it fits whatever amount the simulation
    /// resolves; the part amounts do not change the instruction's size, so
    /// placeholders of 1 stand in for them.
    pub fn sizing_instructions(
        &self,
        identity: &Identity,
    ) -> Result<Vec<Instruction>, MarketMakerError> {
        self.with_shield(identity, vec![1; self.plan.max_utxos.max(1)])
    }

    /// The kVault instruction followed by one shield of `parts` of `asset`
    /// and the `also_shield` UTXOs.
    fn with_shield(
        &self,
        identity: &Identity,
        parts: Vec<u64>,
    ) -> Result<Vec<Instruction>, MarketMakerError> {
        let shield = identity.shield(
            parts
                .into_iter()
                .map(|part| (self.asset, part))
                .chain(self.also_shield.iter().copied())
                .collect(),
        )?;
        Ok(vec![self.vault_instruction.clone(), shield])
    }

    /// The amount of `asset` to shield when the simulation leaves `after` in
    /// `asset_account`: `swept + delta - margin` with `delta = after -
    /// before`, `margin = bps_of(delta, SHIELD_MARGIN_BPS)` and `swept =
    /// min(before, bps_of(delta, SWEEP_CAP_BPS))`,
    /// so residuals of earlier tails are swept along but a larger prior
    /// balance stays in the account. Fails with `NothingToShield` if the
    /// simulation pays nothing into the account (`after <= before`) or the
    /// margin takes the whole amount.
    pub fn amount(&self, after: u64) -> Result<u64, MarketMakerError> {
        let nothing = || MarketMakerError::NothingToShield {
            asset: self.asset,
            account: self.asset_account,
        };
        let delta = after
            .checked_sub(self.before)
            .filter(|delta| *delta > 0)
            .ok_or_else(nothing)?;
        let received = delta.checked_sub(bps_of(delta, SHIELD_MARGIN_BPS)).ok_or(
            MarketMakerError::AmountOverflow {
                context: "tail shield margin",
            },
        )?;
        let amount = self
            .before
            .min(bps_of(delta, SWEEP_CAP_BPS))
            .checked_add(received)
            .ok_or(MarketMakerError::AmountOverflow {
                context: "tail shield amount",
            })?;
        if amount == 0 {
            Err(nothing())
        } else {
            Ok(amount)
        }
    }

    /// Resolves the tail to the instructions that are sent:
    ///
    /// 1. simulate `head` (the transact that pays the market maker's account,
    ///    if any), the kVault instruction and the `also_shield` shield, with
    ///    the tail's compute units, so the simulation fails where the
    ///    transaction would (`MarketMakerError::SimulationFailed`);
    /// 2. read the simulated balance of `asset_account`; the shield amount
    ///    is derived from it by [`TailShield::amount`]
    ///    (`MarketMakerError::NothingToShield` when the vault paid nothing);
    /// 3. return the kVault instruction and one shield of that amount split
    ///    by `plan`, plus `also_shield`.
    ///
    /// The simulation sees the vault state at resolve time, so the shield
    /// follows the vault's executed price rather than the preview.
    pub async fn resolve(
        &self,
        sender: &SendQueue,
        identity: &Identity,
        head: Option<&Instruction>,
    ) -> Result<Vec<Instruction>, MarketMakerError> {
        let mut instructions: Vec<Instruction> = head
            .into_iter()
            .cloned()
            .chain(std::iter::once(self.vault_instruction.clone()))
            .collect();
        if !self.also_shield.is_empty() {
            instructions.push(identity.shield(self.also_shield.clone())?);
        }
        let request = SendRequest {
            instructions,
            compute_units: self.compute_units,
        };
        let after = sender
            .simulate_token_balance(&request, self.asset_account)
            .await?;
        let amount = self.amount(after)?;
        tracing::debug!(
            asset = %self.asset,
            before = self.before,
            after,
            amount,
            "resolved tail shield"
        );
        self.instructions(identity, amount)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tail whose share account held `before` ahead of the vault
    /// operation.
    fn tail(before: u64) -> TailShield {
        TailShield {
            vault_instruction: Instruction {
                program_id: Address::default(),
                accounts: Vec::new(),
                data: Vec::new(),
            },
            asset: Address::new_unique(),
            asset_account: Address::new_unique(),
            before,
            plan: ShieldPlan {
                profile: InventoryProfile::equal(1, 0),
                utxos: Vec::new(),
                max_utxos: 1,
            },
            also_shield: Vec::new(),
            compute_units: REBALANCE_COMPUTE_BUDGET.cu_limit,
        }
    }

    /// The vault compute budget is 700_000 plus 50_000 per reserve, capped
    /// at `MAX_COMPUTE_UNITS`.
    #[test]
    fn vault_compute_units_grow_with_reserves_up_to_the_ceiling() {
        for (label, reserves, want) in [
            ("no reserves", 0, 700_000),
            ("4 reserves", 4, 900_000),
            ("14 reserves reach the ceiling", 14, MAX_COMPUTE_UNITS),
            ("25 reserves stay at the ceiling", 25, MAX_COMPUTE_UNITS),
            (
                "usize::MAX reserves do not overflow",
                usize::MAX,
                MAX_COMPUTE_UNITS,
            ),
        ] {
            let got = vault_compute_units(reserves);
            assert_eq!(got, want, "{label}: got {got}, want {want}");
        }
    }

    /// A tail shields its payout minus a margin of `max(1, payout / 10_000)`
    /// plus the residual a previous tail left, up to the sweep cap.
    #[test]
    fn amount_keeps_one_bps_margin_and_sweeps_residual() {
        for (label, before, after, want) in [
            ("payout 200M, no residual", 0, 200_000_000, 199_980_000),
            (
                "payout 100M, residual 20_000 swept",
                20_000,
                100_020_000,
                100_010_000,
            ),
            ("payout below 10_000 keeps a margin of 1", 0, 9_999, 9_998),
            (
                "payout 1: the cap is 1, so 1 of the 5 is swept and the margin is 1",
                5,
                6,
                1,
            ),
        ] {
            let got = tail(before).amount(after).ok();
            assert_eq!(got, Some(want), "{label}: got {got:?}, want {want}");
        }
    }

    /// The sweep of an earlier residual is capped at `max(1, payout / 1_000)`.
    #[test]
    fn amount_caps_the_sweep_at_ten_bps_of_the_payout() {
        // Payout 100M: cap 100_000, margin 10_000.
        for (label, before, after, want) in [
            (
                "residual below the cap is swept fully",
                50_000,
                100_050_000,
                100_040_000,
            ),
            (
                "residual above the cap is swept up to the cap",
                1_000_000_000,
                1_100_000_000,
                100_090_000,
            ),
        ] {
            let got = tail(before).amount(after).ok();
            assert_eq!(got, Some(want), "{label}: got {got:?}, want {want}");
        }
    }

    /// A share account that did not grow fails with `NothingToShield`.
    #[test]
    fn amount_without_payout_is_nothing_to_shield() {
        for (label, before, after) in [
            ("empty account unchanged", 0, 0),
            ("residual unchanged", 7, 7),
            ("balance dropped", 7, 3),
            ("payout of 1 is all margin", 0, 1),
        ] {
            let refused = tail(before).amount(after);
            assert!(
                matches!(refused, Err(MarketMakerError::NothingToShield { .. })),
                "{label}: got {refused:?}, want NothingToShield"
            );
        }
    }
}
