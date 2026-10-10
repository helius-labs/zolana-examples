//! Settling a fill: the user's signature is checked against the parked swap
//! message, the maker co-signs, and the transaction goes to the send path.
//! A message is only co-signed while its fill step awaits a signature and its
//! deadline has not passed.

use std::time::Instant;

use solana_address::Address;
use solana_message::VersionedMessage;
use solana_signature::Signature;
use solana_signer::Signer;
use solana_transaction::versioned::VersionedTransaction;
use tokio::sync::oneshot;

use k_lend_rfq_sdk::swap::Fill;

use crate::{
    api::Inner,
    error::MakerError,
    transactions::{
        confirm::Retry,
        coordinator::{Coordinator, Event},
        steps::StepState,
    },
};

impl Inner {
    /// Submits the user's signature over `fill.message` and waits for the
    /// swap's landing signature or the error of `Coordinator::on_settle`.
    /// Errors with `MakerError::ShuttingDown` or
    /// `MakerError::CoordinatorStopped` when the coordinator is gone.
    pub async fn settle(
        &self,
        fill: &Fill,
        user_signature: Signature,
    ) -> Result<Signature, MakerError> {
        let (reply, receipt) = oneshot::channel();
        self.runtime
            .events
            .send(Event::Settle {
                message: fill.message.clone(),
                user_signature,
                reply,
            })
            .map_err(|_| MakerError::ShuttingDown)?;
        receipt.await.map_err(|_| MakerError::CoordinatorStopped)?
    }
}

impl Coordinator {
    /// Co-signs and sends the fill whose message is `message`.
    ///
    /// Replies, in order of the checks: `MakerError::UnknownFill` when no
    /// fill step holds `message`; `MakerError::AlreadySettling` when the
    /// step is not `AwaitingSignature` (a second settle of the same fill);
    /// `MakerError::ReservationExpired` when the fill's deadline has passed,
    /// which also discards the step; then any `co_sign` error, which leaves
    /// the step awaiting a valid signature. On success the reply is parked on
    /// the step and answered once the send path resolves.
    pub async fn on_settle(
        &mut self,
        message: &VersionedMessage,
        user_signature: Signature,
        reply: oneshot::Sender<Result<Signature, MakerError>>,
    ) {
        let Some(id) = self.steps.find_fill(message) else {
            let _ = reply.send(Err(MakerError::UnknownFill));
            return;
        };
        let Some(step) = self.steps.get_mut(id) else {
            let _ = reply.send(Err(MakerError::UnknownFill));
            return;
        };
        if step.state != StepState::AwaitingSignature {
            let _ = reply.send(Err(MakerError::AlreadySettling { step: id }));
            return;
        }
        let Some(fill) = step.fill.as_mut() else {
            let _ = reply.send(Err(MakerError::UnknownFill));
            return;
        };
        if fill
            .expires_at
            .is_none_or(|deadline| Instant::now() >= deadline)
        {
            let _ = reply.send(Err(MakerError::ReservationExpired { step: id }));
            self.discard(id, MakerError::ReservationExpired { step: id }, Retry::Fail)
                .await;
            self.try_schedule().await;
            return;
        }
        let Some(message) = fill.message.clone() else {
            let _ = reply.send(Err(MakerError::UnknownFill));
            return;
        };
        match co_sign(self.identity.signer.as_ref(), message, user_signature) {
            Ok(transaction) => {
                fill.transaction = Some(transaction);
                fill.settle = Some(reply);
                self.spawn_send(id);
            }
            Err(error) => {
                let _ = reply.send(Err(error));
            }
        }
    }
}

/// Signs the fill message as the market maker and assembles it with the
/// user's signature into a transaction ready to send.
///
/// Checks, in order:
/// 1. the message names at least `num_required_signatures` static accounts,
///    else `MakerError::MalformedMessage`;
/// 2. the maker's payer key is one of the required signers, else
///    `MakerError::MakerNotSigner`;
/// 3. at most one other required signer exists, else
///    `MakerError::UnexpectedSigner` naming the second;
/// 4. that signer, the user, exists, else `MakerError::MissingUserSigner`;
/// 5. `user_signature` verifies for the user's key over the serialized
///    message, else `MakerError::InvalidUserSignature`.
///
/// Signing itself can fail with `MakerError::Signer`.
///
/// The user signature is verified here rather than left to the RPC: a
/// preflight rejection would abort the fill, while on a local rejection the
/// fill stays reserved and the user's real signature can still settle it.
fn co_sign(
    maker_signer: &dyn Signer,
    message: VersionedMessage,
    user_signature: Signature,
) -> Result<VersionedTransaction, MakerError> {
    let payer = maker_signer.pubkey();
    let required = usize::from(message.header().num_required_signatures);
    let accounts = message.static_account_keys();
    let signers = accounts
        .get(..required)
        .ok_or(MakerError::MalformedMessage {
            required,
            accounts: accounts.len(),
        })?
        .to_vec();
    let (maker, users): (Vec<Address>, Vec<Address>) =
        signers.iter().partition(|signer| **signer == payer);
    if maker.is_empty() {
        return Err(MakerError::MakerNotSigner);
    }
    if let Some(extra) = users.get(1) {
        return Err(MakerError::UnexpectedSigner { signer: *extra });
    }
    let serialized = message.serialize();
    let user = users.first().ok_or(MakerError::MissingUserSigner)?;
    if !user_signature.verify(user.as_ref(), &serialized) {
        return Err(MakerError::InvalidUserSignature { signer: *user });
    }
    let maker_signature = maker_signer
        .try_sign_message(&serialized)
        .map_err(|source| MakerError::Signer {
            pubkey: payer,
            source,
        })?;
    let signatures = signers
        .iter()
        .map(|signer| {
            if *signer == payer {
                maker_signature
            } else {
                user_signature
            }
        })
        .collect();
    Ok(VersionedTransaction {
        signatures,
        message,
    })
}

#[cfg(test)]
mod tests {
    //! Tested invariants:
    //! 1. `co_sign` refuses a message whose required signers do not include
    //!    the maker's payer key, with `MakerNotSigner`.
    //! 2. `co_sign` refuses a message whose only required signer is the maker,
    //!    with `MissingUserSigner`.
    //! 3. `co_sign` refuses a message with a third required signer, with
    //!    `UnexpectedSigner` naming it.
    //! 4. `co_sign` refuses a header requiring more signers than the message
    //!    has static accounts, with `MalformedMessage`.
    //! 5. `co_sign` refuses a user signature that does not verify, with
    //!    `InvalidUserSignature`, and otherwise returns the transaction with
    //!    each signature at its signer's position.

    use solana_instruction::{AccountMeta, Instruction};
    use solana_message::legacy::Message;
    use zolana_keypair::ShieldedKeypair;

    use super::*;

    const PROGRAM: Address = Address::new_from_array([9; 32]);

    /// Invariant 1: a message signed only by the user is not the maker's to
    /// co-sign.
    #[test]
    fn co_sign_rejects_message_without_maker() {
        let [maker, user] = signers();
        let message = message(&user.pubkey(), &[]);
        let signature = sign(&user, &message.serialize());
        let outcome = co_sign(&maker, message, signature);
        assert!(
            matches!(outcome, Err(MakerError::MakerNotSigner)),
            "got {outcome:?}, want MakerNotSigner"
        );
    }

    /// Invariant 2: a message the maker alone signs has no user signer.
    #[test]
    fn co_sign_rejects_message_without_user() {
        let [maker, _] = signers();
        let message = message(&maker.pubkey(), &[]);
        let outcome = co_sign(&maker, message, Signature::default());
        assert!(
            matches!(outcome, Err(MakerError::MissingUserSigner)),
            "got {outcome:?}, want MissingUserSigner"
        );
    }

    /// Invariant 3: a second non-maker signer is named in the error.
    #[test]
    fn co_sign_rejects_third_signer() {
        let [maker, user] = signers();
        let stranger = Address::new_from_array([5; 32]);
        let message = message(&maker.pubkey(), &[user.pubkey(), stranger]);
        let signature = sign(&user, &message.serialize());
        let want = second_user_signer(&message, &maker.pubkey());
        let outcome = co_sign(&maker, message, signature);
        assert!(
            matches!(outcome, Err(MakerError::UnexpectedSigner { signer }) if signer == want),
            "got {outcome:?}, want UnexpectedSigner of {want}"
        );
    }

    /// Invariant 4: a header requiring one signer more than the static
    /// accounts hold fails before any key is read.
    #[test]
    fn co_sign_rejects_malformed_header() {
        let [maker, user] = signers();
        let mut legacy = Message::new(&[instruction(&[user.pubkey()])], Some(&maker.pubkey()));
        let accounts = legacy.account_keys.len();
        legacy.header.num_required_signatures = u8::try_from(accounts + 1).expect("few accounts");
        let outcome = co_sign(
            &maker,
            VersionedMessage::Legacy(legacy),
            Signature::default(),
        );
        assert!(
            matches!(
                outcome,
                Err(MakerError::MalformedMessage { required, accounts: got })
                    if required == accounts + 1 && got == accounts
            ),
            "got {outcome:?}, want MalformedMessage {{ required: {}, accounts: {accounts} }}",
            accounts + 1
        );
    }

    /// Invariant 5: a signature by another key is refused for the user; the
    /// user's own signature is placed at the user's index and the maker's at
    /// the payer's.
    #[test]
    fn co_sign_verifies_user_signature_and_orders_signatures() {
        let [maker, user] = signers();
        let message = message(&maker.pubkey(), &[user.pubkey()]);
        let serialized = message.serialize();
        let forged = sign(&maker, &serialized);
        let outcome = co_sign(&maker, message.clone(), forged);
        let user_key = user.pubkey();
        assert!(
            matches!(outcome, Err(MakerError::InvalidUserSignature { signer }) if signer == user_key),
            "got {outcome:?}, want InvalidUserSignature of {user_key}"
        );

        let user_signature = sign(&user, &serialized);
        let transaction = co_sign(&maker, message, user_signature).expect("valid co-sign");
        assert_eq!(
            transaction.signatures,
            vec![sign(&maker, &serialized), user_signature],
            "signatures in signer order"
        );
    }

    // Test fixtures and shared helpers.

    /// A maker and a user ed25519 signer.
    fn signers() -> [ShieldedKeypair; 2] {
        [
            ShieldedKeypair::new_ed25519().expect("maker keypair"),
            ShieldedKeypair::new_ed25519().expect("user keypair"),
        ]
    }

    /// An instruction requiring the signatures of `signers`.
    fn instruction(signers: &[Address]) -> Instruction {
        Instruction {
            program_id: PROGRAM,
            accounts: signers
                .iter()
                .map(|signer| AccountMeta::new_readonly(*signer, true))
                .collect(),
            data: Vec::new(),
        }
    }

    /// A legacy message paid by `payer` whose one instruction also requires
    /// `signers`.
    fn message(payer: &Address, signers: &[Address]) -> VersionedMessage {
        VersionedMessage::Legacy(Message::new(&[instruction(signers)], Some(payer)))
    }

    /// The second required signer of `message` other than `payer`, the one
    /// `co_sign` reports.
    fn second_user_signer(message: &VersionedMessage, payer: &Address) -> Address {
        let required = usize::from(message.header().num_required_signatures);
        *message
            .static_account_keys()
            .iter()
            .take(required)
            .filter(|key| *key != payer)
            .nth(1)
            .expect("two non-payer signers")
    }

    /// `signer`'s ed25519 signature over `message`.
    fn sign(signer: &ShieldedKeypair, message: &[u8]) -> Signature {
        signer.try_sign_message(message).expect("ed25519 signs")
    }
}
