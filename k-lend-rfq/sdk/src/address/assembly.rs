//! Assembling a fill transfer with its order address slot: the witness
//! request that fetches the address's non-inclusion proof and the patch on
//! zolana's assembly (see the parent module).

use anyhow::{anyhow, Result};
use num_bigint::BigUint;
use zolana_client::{
    assemble, input_utxos_from_nullifiers,
    prover::field::{be, right_align_slice},
    AssembledTransfer, NonInclusionProof, PublicInputs, PublicTransfers, SpendProof, TransferInput,
    TransferInputs, TreeSlotFields, STATE_TREE_HEIGHT,
};
use zolana_interface::{
    instruction::{
        instruction_data::transact::{TreeContext, NO_UTXO_ROOT},
        InputUtxo, TransactIxData,
    },
    pda,
    tree_slot::{pack_input_flags, TreeSlot},
    INPUT_TREES, MAX_INPUT_TREES,
};
use zolana_program::PrivateTxHash;
use zolana_transaction::instructions::transact::SppProofInputs;

use super::{OrderAddress, ORDER_ADDRESS_TREE};

impl OrderAddress {
    /// The prover witness of the slot in input slot `tree_index`'s tree:
    /// no state inclusion path (an address slot is not in the state tree), the
    /// nullifier non-inclusion proof of the address, the address as the
    /// published nullifier, the signer's owner identity and the public zero
    /// nullifier secret, which `ProofAuthority::complete_inputs` leaves as is.
    /// Errors when `non_inclusion` is not for the address.
    fn witness(&self, non_inclusion: &NonInclusionProof, tree_index: u8) -> Result<TransferInput> {
        if non_inclusion.leaf != self.address {
            return Err(anyhow!(
                "the non-inclusion proof is not for the address of order {}",
                self.id
            ));
        }
        Ok(TransferInput {
            utxo: self.slot.clone(),
            is_dummy: BigUint::ZERO,
            state_path_elements: vec![BigUint::ZERO; STATE_TREE_HEIGHT],
            state_path_index: BigUint::ZERO,
            nullifier_low_value: be(&non_inclusion.low_element),
            nullifier_next_value: be(&non_inclusion.high_element),
            nullifier_low_path_elements: non_inclusion.path.iter().map(be).collect(),
            nullifier_low_path_index: BigUint::from(non_inclusion.low_element_index),
            tree_slot: BigUint::from(tree_index),
            nullifier: be(&self.address),
            owner_pk_hash: be(&self.owner.owner_proof_input_hash()?),
            nullifier_secret: Some(BigUint::ZERO),
        })
    }
}

/// The nullifiers a transfer fetches non-inclusion proofs for besides its
/// real inputs, and where the order address's proof comes from.
///
/// All proofs of one tree must be against one nullifier root. When the
/// transfer's padding tree is [`ORDER_ADDRESS_TREE`], the address is fetched
/// in the same request as the padding nullifiers, so its proof shares their
/// root; otherwise it is fetched alone from [`ORDER_ADDRESS_TREE`], which then
/// joins the transfer as a second input tree.
pub struct WitnessRequest {
    /// The padding nullifiers, followed by the order address when it shares
    /// their tree.
    pub dummy_nullifiers: Vec<[u8; 32]>,
    source: AddressSource,
}

/// Where a [`WitnessRequest`] gets the order address's proof from.
enum AddressSource {
    /// The transfer carries no order address.
    Absent,
    /// The last of the padding proofs.
    SharedWithPadding,
    /// A request of its own for this address.
    Separate([u8; 32]),
}

impl WitnessRequest {
    /// The request for `proof_inputs`, whose padding nullifiers are proved
    /// against tree `padding_tree` (the tree of its first input), with the
    /// address of `order` if set.
    pub fn new(
        proof_inputs: &SppProofInputs,
        padding_tree: u16,
        order: Option<&OrderAddress>,
    ) -> Self {
        let mut dummy_nullifiers = proof_inputs.dummy_nullifiers();
        let source = match order {
            None => AddressSource::Absent,
            Some(order) if padding_tree == ORDER_ADDRESS_TREE && !dummy_nullifiers.is_empty() => {
                dummy_nullifiers.push(order.address);
                AddressSource::SharedWithPadding
            }
            Some(order) => AddressSource::Separate(order.address),
        };
        Self {
            dummy_nullifiers,
            source,
        }
    }

    /// The address to fetch a non-inclusion proof for from
    /// `pda::tree(ORDER_ADDRESS_TREE)` in a request of its own, if any.
    pub fn separate_address(&self) -> Option<[u8; 32]> {
        match self.source {
            AddressSource::Separate(address) => Some(address),
            AddressSource::Absent | AddressSource::SharedWithPadding => None,
        }
    }

    /// Splits the order address's proof off the fetched padding proofs, or
    /// takes it from `separate`, the answer to [`Self::separate_address`].
    /// Errors when the request has no order address or the indexer returned
    /// no proof for it.
    pub fn address_proof(
        &self,
        dummy_proofs: &mut Vec<NonInclusionProof>,
        separate: Option<NonInclusionProof>,
    ) -> Result<NonInclusionProof> {
        let proof = match self.source {
            AddressSource::Absent => return Err(anyhow!("the request has no order address")),
            AddressSource::SharedWithPadding => dummy_proofs.pop(),
            AddressSource::Separate(_) => separate,
        };
        proof.ok_or_else(|| anyhow!("the indexer returned no proof for the order address"))
    }
}

/// The instruction-data values the address slot changes, applied on top of
/// zolana's instruction data by [`Self::apply`].
pub struct AddressPatch {
    inputs: Vec<InputUtxo>,
    private_tx_hash: [u8; 32],
    tree_context: Option<TreeContext>,
}

impl AddressPatch {
    /// Puts the address slot's values into `ix`, the instruction data of the
    /// transfer [`add_order_address`] returned this patch with.
    pub fn apply(self, ix: &mut TransactIxData) {
        ix.inputs = self.inputs;
        ix.private_tx_hash = self.private_tx_hash;
        ix.tree_contexts.extend(self.tree_context);
    }
}

/// Assembles `proof_inputs` with zolana's `assemble`, then puts the address
/// slot of `order` with its non-inclusion proof `non_inclusion` into the
/// first padding slot:
///
/// 1. the slot is the first dummy input; the transfer must have one (the
///    caller pads to a shape with [`super::ORDER_ADDRESS_SLOTS`] spare input),
///    and it comes after every real input, as the circuit requires real
///    slots, address slots included, before the dummies;
/// 2. every value zolana computed is read back from its witness and the
///    private transaction hash and public input hash are recomputed from
///    them; both must equal zolana's, so the values the slot changes are
///    recomputed on exactly zolana's basis;
/// 3. the slot's tree is [`ORDER_ADDRESS_TREE`]: the tree's existing index if
///    the transfer spends from it (the address proof must then be against the
///    same nullifier root and root index as its other inputs), else a new
///    input tree with no UTXO root (`NO_UTXO_ROOT`) and the proof's nullifier
///    root;
/// 4. the slot's witness replaces the padding witness, its nullifier the
///    padding nullifier, and the input flags (dummies allowed) are packed with
///    its tree index;
/// 5. the private transaction hash gains the address in its address nullifier
///    chain (`address_nullifiers`, zero in every other slot), and the public
///    input hash is recomputed with the new nullifiers, tree slots, flags and
///    private transaction hash.
///
/// Returns the patched transfer, whose prover inputs, public input hash and
/// input tree ids carry the slot, and the patch its instruction data needs
/// once proven ([`AddressPatch::apply`]).
///
/// Errors with zolana's assembly error, when there is no padding slot, when
/// the transfer would span more than `MAX_INPUT_TREES` trees, when the address
/// proof's root differs from the tree's, and when the read back hashes differ
/// from zolana's.
pub fn add_order_address(
    proof_inputs: SppProofInputs,
    spend_proofs: &[SpendProof],
    dummy_proofs: &[NonInclusionProof],
    order: &OrderAddress,
    non_inclusion: &NonInclusionProof,
) -> Result<(AssembledTransfer, AddressPatch)> {
    // 1. The first padding slot, after every real input.
    let slot = proof_inputs
        .input_utxos
        .iter()
        .position(|input| input.is_dummy())
        .ok_or_else(|| anyhow!("no padding slot for the address of order {}", order.id))?;
    let blinding = proof_inputs.private_tx_blinding()?;
    let public_transfers = proof_inputs.public_transfers()?;
    let mut transfer = assemble(proof_inputs, spend_proofs, dummy_proofs)?;

    // 2. Read zolana's values back and check them against its hashes.
    let mut read = ReadBack::new(&transfer.prover_inputs)?;
    read.check_against(&transfer, &blinding, &public_transfers)?;

    // 3. The address tree's index.
    let (tree_index, tree_context) =
        address_tree(&mut transfer, &mut read, spend_proofs, non_inclusion)?;

    // 4. The slot's witness, nullifier and tree index. `witness` checks that
    //    the proof's leaf is the order address.
    let witness = order.witness(non_inclusion, tree_index)?;
    let address = non_inclusion.leaf;
    let (input, nullifier, index) = transfer
        .prover_inputs
        .inputs
        .get_mut(slot)
        .zip(read.nullifiers.get_mut(slot))
        .zip(read.tree_indexes.get_mut(slot))
        .map(|((input, nullifier), index)| (input, nullifier, index))
        .ok_or_else(|| anyhow!("padding slot {slot} out of range"))?;
    *input = witness;
    *nullifier = address;
    *index = tree_index;
    read.input_flags = pack_input_flags(true, read.tree_indexes.iter().copied())?;

    // 5. The private transaction hash and the public input hash.
    let (private_tx_hash, public_input_hash) =
        read.address_hashes(slot, address, &blinding, &public_transfers)?;
    let prover_inputs = &mut transfer.prover_inputs;
    prover_inputs.tree_slots = TreeSlotFields::encode_all(&read.tree_slots);
    prover_inputs.private_tx_hash = be(&private_tx_hash);
    prover_inputs.input_flags = be(&read.input_flags);
    prover_inputs.public_input_hash = be(&public_input_hash);
    transfer.public_input_hash = public_input_hash;
    let inputs = input_utxos_from_nullifiers(&read.nullifiers, &read.tree_indexes)?;
    Ok((
        transfer,
        AddressPatch {
            inputs,
            private_tx_hash,
            tree_context,
        },
    ))
}

/// Step 3 of [`add_order_address`]: the index of [`ORDER_ADDRESS_TREE`] among
/// `transfer`'s input trees, and the tree context to add when the transfer
/// does not spend from it ([`add_address_tree`]). When it does, the address
/// proof must be against the nullifier root of the tree's other inputs.
fn address_tree(
    transfer: &mut AssembledTransfer,
    read: &mut ReadBack,
    spend_proofs: &[SpendProof],
    non_inclusion: &NonInclusionProof,
) -> Result<(u8, Option<TreeContext>)> {
    let Some(position) = transfer
        .input_tree_ids
        .iter()
        .position(|tree_id| *tree_id == ORDER_ADDRESS_TREE)
    else {
        return add_address_tree(transfer, read, non_inclusion);
    };
    let tree = pda::tree(ORDER_ADDRESS_TREE);
    let tree_root = spend_proofs
        .iter()
        .map(|proof| &proof.nullifier)
        .find(|proof| proof.merkle_context.tree == tree)
        .map(|proof| (proof.root, proof.root_index));
    if tree_root != Some((non_inclusion.root, non_inclusion.root_index)) {
        return Err(anyhow!(
            "the order address proof is against another nullifier root than the tree's inputs"
        ));
    }
    Ok((u8::try_from(position)?, None))
}

/// Adds [`ORDER_ADDRESS_TREE`] as the next input tree of `transfer`, with no
/// UTXO root and the nullifier root of `non_inclusion`. Errors when the
/// transfer already has `MAX_INPUT_TREES` input trees.
fn add_address_tree(
    transfer: &mut AssembledTransfer,
    read: &mut ReadBack,
    non_inclusion: &NonInclusionProof,
) -> Result<(u8, Option<TreeContext>)> {
    let position = transfer.input_tree_ids.len();
    if position >= MAX_INPUT_TREES {
        return Err(anyhow!(
            "the order address tree would be input tree {} of at most {MAX_INPUT_TREES}",
            position + 1
        ));
    }
    let tree_slot = read
        .tree_slots
        .get_mut(position)
        .ok_or_else(|| anyhow!("tree slot {position} out of range"))?;
    *tree_slot = TreeSlot::new(ORDER_ADDRESS_TREE, [0; 32], non_inclusion.root);
    transfer.input_tree_ids.push(ORDER_ADDRESS_TREE);
    let context = TreeContext {
        utxo_tree_root_index: NO_UTXO_ROOT,
        nullifier_tree_root_index: non_inclusion.root_index,
    };
    Ok((u8::try_from(position)?, Some(context)))
}

/// The values zolana's assembly put into a transfer witness, as 32-byte
/// field elements, for recomputing the hashes the address slot changes.
struct ReadBack {
    /// Per input slot: the published nullifier (0 for compact padding).
    nullifiers: Vec<[u8; 32]>,
    /// Per input slot: the UTXO hash of a real input, 0 for padding.
    input_hashes: Vec<[u8; 32]>,
    /// Per input slot: the index of its tree among the input trees.
    tree_indexes: Vec<u8>,
    /// Per output slot: the output hash.
    output_hashes: Vec<[u8; 32]>,
    /// Per output slot: the output hash of a real output, 0 for padding.
    private_output_hashes: Vec<[u8; 32]>,
    output_owner_pk_hashes: Vec<[u8; 32]>,
    signer_pk_hashes: Vec<[u8; 32]>,
    tree_slots: [TreeSlot; INPUT_TREES],
    output_tree_id: u16,
    external_data_hash: [u8; 32],
    input_flags: [u8; 32],
    cached_inputs: [[u8; 32]; 2],
}

impl ReadBack {
    fn new(inputs: &TransferInputs) -> Result<Self> {
        let is_padding = |is_dummy: &BigUint| *is_dummy != BigUint::ZERO;
        let mut tree_slots = [TreeSlot::ZERO; INPUT_TREES];
        for (slot, fields) in tree_slots.iter_mut().zip(inputs.tree_slots.iter()) {
            *slot = TreeSlot {
                id: field(&fields.id)?,
                utxo_root: field(&fields.utxo_root)?,
                nullifier_root: field(&fields.nullifier_root)?,
            };
        }
        let output_hashes = inputs
            .outputs
            .iter()
            .map(|output| field(&output.hash))
            .collect::<Result<Vec<_>>>()?;
        let private_output_hashes = inputs
            .outputs
            .iter()
            .zip(output_hashes.iter())
            .map(|(output, hash)| {
                if is_padding(&output.is_dummy) {
                    [0; 32]
                } else {
                    *hash
                }
            })
            .collect();
        Ok(Self {
            nullifiers: inputs
                .inputs
                .iter()
                .map(|input| field(&input.nullifier))
                .collect::<Result<_>>()?,
            input_hashes: inputs
                .inputs
                .iter()
                .map(|input| {
                    if is_padding(&input.is_dummy) {
                        Ok([0; 32])
                    } else {
                        Ok(input.utxo.hash()?)
                    }
                })
                .collect::<Result<_>>()?,
            tree_indexes: inputs
                .inputs
                .iter()
                .map(|input| Ok(u8::try_from(&input.tree_slot)?))
                .collect::<Result<_>>()?,
            output_hashes,
            private_output_hashes,
            output_owner_pk_hashes: inputs
                .published_output_owner_pk_hashes
                .iter()
                .map(field)
                .collect::<Result<_>>()?,
            signer_pk_hashes: inputs
                .signer_pk_hashes
                .iter()
                .map(field)
                .collect::<Result<_>>()?,
            tree_slots,
            output_tree_id: u16::try_from(&inputs.output_tree_id)?,
            external_data_hash: field(&inputs.external_data_hash)?,
            input_flags: field(&inputs.input_flags)?,
            cached_inputs: [
                field(&inputs.cache.tree_id)?,
                field(&inputs.cache.read_hash_chain)?,
            ],
        })
    }

    /// Step 2 of [`add_order_address`]: the private transaction hash and the
    /// public input hash recomputed from these values equal `transfer`'s.
    fn check_against(
        &self,
        transfer: &AssembledTransfer,
        blinding: &[u8; 32],
        public_transfers: &PublicTransfers,
    ) -> Result<()> {
        let private_tx =
            PrivateTxHash::new(&self.input_hashes, &self.private_output_hashes, blinding).hash()?;
        if be(&private_tx) != transfer.prover_inputs.private_tx_hash
            || self.public_input_hash(public_transfers, &private_tx)? != transfer.public_input_hash
        {
            return Err(anyhow!(
                "the transfer's read-back hashes differ from zolana's assembly"
            ));
        }
        Ok(())
    }

    /// Step 5 of [`add_order_address`]: the private transaction hash with
    /// `address` in input slot `slot` of its address nullifier chain, and the
    /// public input hash over it.
    fn address_hashes(
        &self,
        slot: usize,
        address: [u8; 32],
        blinding: &[u8; 32],
        public_transfers: &PublicTransfers,
    ) -> Result<([u8; 32], [u8; 32])> {
        let mut address_nullifiers = vec![[0; 32]; self.nullifiers.len()];
        *address_nullifiers
            .get_mut(slot)
            .ok_or_else(|| anyhow!("padding slot {slot} out of range"))? = address;
        let private_tx_hash = PrivateTxHash {
            input_hashes: &self.input_hashes,
            output_hashes: &self.private_output_hashes,
            address_nullifiers: Some(&address_nullifiers),
            blinding,
        }
        .hash()?;
        let public_input_hash = self.public_input_hash(public_transfers, &private_tx_hash)?;
        Ok((private_tx_hash, public_input_hash))
    }

    /// The public input hash of the confidential transfer rail over these
    /// values and `private_tx_hash` (zolana's `TransferProver::build`).
    fn public_input_hash(
        &self,
        public_transfers: &PublicTransfers,
        private_tx_hash: &[u8; 32],
    ) -> Result<[u8; 32]> {
        Ok(PublicInputs {
            nullifiers: &self.nullifiers,
            output_hashes: &self.output_hashes,
            tree_slots: &self.tree_slots,
            output_tree_id: self.output_tree_id,
            private_tx: private_tx_hash,
            external_data_hash: &self.external_data_hash,
            public_transfers,
            ring_program_id: &[0; 32],
            input_flags: &self.input_flags,
            signer_pk_hashes: &self.signer_pk_hashes,
            output_owner_pk_hashes: Some(&self.output_owner_pk_hashes),
            cached_inputs: self.cached_inputs,
        }
        .hash()?)
    }
}

/// A witness field element as 32 big-endian bytes.
fn field(value: &BigUint) -> Result<[u8; 32]> {
    Ok(right_align_slice(&value.to_bytes_be())?)
}
