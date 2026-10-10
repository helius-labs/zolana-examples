//! Initial inventory: moving the market maker's public tokens into the shielded
//! pool, optionally through a vault deposit for the share side.

use zolana_interface::pda;

use k_lend_rfq_sdk::pair::Pair;

use super::rebalance::{RebalanceKind, RebalanceRequest};
use crate::{
    api::{Inner, VaultOperation},
    error::MarketMakerError,
    transactions::{
        kvault::{read_vault, token_balance},
        send::{SendOutcome, SendRequest},
        shield::{ShieldPlan, REBALANCE_COMPUTE_BUDGET},
    },
};

impl Inner {
    /// Deposits `deposit` collateral from the market maker's public account
    /// into the vault and shields the minted shares, plus `collateral`
    /// collateral, in one transaction. The shielded share amount is resolved
    /// from a simulation of the deposit (`TailShield::resolve`), sweeping a
    /// residual already in the share account.
    ///
    /// Sent directly, not through the coordinator: it runs before trading.
    /// Errors with `MarketMakerError::AmountZero` when both amounts are zero,
    /// and with the send path's rejection; a send whose outcome is unknown is
    /// treated as sent and confirmed through `settled`.
    pub async fn seed_inventory(
        &self,
        pair: &Pair,
        deposit: u64,
        collateral: u64,
    ) -> Result<VaultOperation, MarketMakerError> {
        if deposit == 0 && collateral == 0 {
            return Err(MarketMakerError::AmountZero);
        }
        let rpc = self.services.rpc.as_ref();
        let before = read_vault(rpc, pair.vault).await?;
        let order = RebalanceRequest {
            pair: *pair,
            kind: RebalanceKind::Shares,
            amount: deposit,
        };
        let shares_before = if deposit == 0 {
            0
        } else {
            token_balance(
                rpc,
                pda::associated_token_address(
                    &self.identity.payer,
                    &order.kind.received_asset(pair),
                ),
            )
            .await?
        };
        let (collateral_utxos, shares_plan) = {
            let settings = self.settings();
            let reservations = &self.services.pending.reservations;
            let collateral_utxos: Vec<_> =
                ShieldPlan::new(&settings, reservations, &pair.token_mint)
                    .amounts(collateral)
                    .into_iter()
                    .map(|amount| (pair.token_mint, amount))
                    .collect();
            let shares_plan =
                ShieldPlan::new(&settings, reservations, &order.kind.received_asset(pair));
            (collateral_utxos, shares_plan)
        };
        let sender = &self.services.sender;
        let request = if deposit == 0 {
            SendRequest {
                instructions: vec![self.identity.shield(collateral_utxos)?],
                compute_units: REBALANCE_COMPUTE_BUDGET.cu_limit,
            }
        } else {
            let tail = order
                .tail(
                    before.clone(),
                    &self.identity,
                    shares_plan,
                    shares_before,
                    collateral_utxos,
                )?
                .shield;
            SendRequest {
                instructions: tail.resolve(sender, &self.identity, None).await?,
                compute_units: tail.compute_units,
            }
        };
        sender.check_size(&request)?;
        let signature = match sender.send(&request).await {
            SendOutcome::Sent(sent) | SendOutcome::OutcomeUnknown { sent, .. } => sent.signature,
            SendOutcome::Rejected(rejection) => return Err(rejection.into()),
            SendOutcome::NotSent(error) => return Err(error),
        };
        self.settled(pair, before, signature, 0).await
    }
}
