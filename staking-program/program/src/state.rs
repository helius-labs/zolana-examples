use pinocchio::{error::ProgramError, AccountView, Address};
use wincode::{SchemaRead, SchemaWrite};
use zolana_hasher::{Hasher, Poseidon};
use zolana_interface::instruction::instruction_data::transact::{
    InputUtxo, OwnerTag, TransactOutput,
};

use crate::{error::StakingError, pda, M, N};

pub const CONFIG_DISCRIMINATOR: u8 = 1;
pub const EPOCH_DISCRIMINATOR: u8 = 2;
pub const BATCH_DISCRIMINATOR: u8 = 3;
pub const CONFIG_SIZE: usize = 302;
pub const EPOCH_HEADER_SIZE: usize = 82;
pub const CHAIN_SIZE: usize = 36;
pub const EPOCH_SIZE: usize = EPOCH_HEADER_SIZE + M * CHAIN_SIZE;
pub const BATCH_HEADER_SIZE: usize = 249;
pub const MAX_CIPHERTEXT_BYTES: usize = 384;
pub const ENTRY_SIZE: usize = 2 + 33 + 1 + 32 + 33 + 1 + 2 + MAX_CIPHERTEXT_BYTES;
pub const BATCH_SIZE: usize = BATCH_HEADER_SIZE + N * ENTRY_SIZE;
pub type BatchEntry = (InputUtxo, Option<TransactOutput>);

#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct ShieldedAddress {
    pub signing_pubkey: Address,
    pub nullifier_pubkey: [u8; 32],
    pub viewing_pubkey: [u8; 33],
}

/// SPP owner encoding; intent recipients use Solana signing keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct RecipientAddress {
    pub signing_pubkey: [u8; 34],
    pub nullifier_pubkey: [u8; 32],
    pub viewing_pubkey: [u8; 33],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct IntentUtxoData {
    pub epoch: Address,
    pub recipient: RecipientAddress,
}

impl IntentUtxoData {
    pub fn hash(&self) -> Result<[u8; 32], ProgramError> {
        use zolana_hasher::primitives::hash_bytes;
        Poseidon::hashv(&[
            &hash_bytes(b"stake_pool_intent").map_err(|_| StakingError::HashingFailed)?,
            &hash_bytes(self.epoch.as_array()).map_err(|_| StakingError::HashingFailed)?,
            &hash_bytes(&self.recipient.signing_pubkey).map_err(|_| StakingError::HashingFailed)?,
            &self.recipient.nullifier_pubkey,
            &hash_bytes(&self.recipient.viewing_pubkey).map_err(|_| StakingError::HashingFailed)?,
        ])
        .map_err(|_| StakingError::HashingFailed.into())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct Config {
    pub discriminator: u8,
    pub authority: Address,
    pub stake_pool_program_id: Address,
    pub stake_pool: Address,
    pub pool_mint: Address,
    pub token_program_id: Address,
    pub escrow_tree: Address,
    pub crank: ShieldedAddress,
    pub intent_fee_lamports: u64,
    pub settlement_window_seconds: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct EpochHeader {
    pub discriminator: u8,
    pub config: Address,
    pub epoch_id: u64,
    pub page_index: u64,
    pub submission_end_slot: u64,
    pub settlement_deadline: i64,
    pub locked: bool,
    pub intent_count: u32,
    pub resolved_intent_count: u32,
    pub fee_balance_lamports: u64,
}

impl EpochHeader {
    pub fn resolve(&mut self, count: u32) -> Result<(), ProgramError> {
        let resolved = self
            .resolved_intent_count
            .checked_add(count)
            .ok_or(StakingError::Overflow)?;
        if resolved > self.intent_count {
            return Err(StakingError::UnresolvedIntents.into());
        }
        self.resolved_intent_count = resolved;
        Ok(())
    }

    pub fn check_settlement(&self, now: i64) -> Result<(), ProgramError> {
        if !self.locked {
            return Err(StakingError::EpochUnlocked.into());
        }
        if now >= self.settlement_deadline {
            return Err(StakingError::SettlementExpired.into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct HashChain {
    pub utxo_hash_chain: [u8; 32],
    pub intent_count: u16,
    pub executed: bool,
    pub batch_exists: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct BatchHeader {
    pub discriminator: u8,
    pub epoch: Address,
    pub hash_chain_index: u16,
    pub utxo_hash_chain: [u8; 32],
    pub intent_count: u16,
    pub crank: ShieldedAddress,
    pub rent_payer: Address,
    pub tx_viewing_pk: [u8; 33],
    pub salt: [u8; 16],
    pub filled_count: u16,
}

fn decode<'a, T: SchemaRead<'a, wincode::config::DefaultConfig, Dst = T>>(
    data: &'a [u8],
) -> Result<T, ProgramError> {
    wincode::deserialize_exact(data).map_err(|_| StakingError::InvalidAccount.into())
}

fn encode<T: SchemaWrite<wincode::config::DefaultConfig, Src = T>>(
    value: &T,
    data: &mut [u8],
) -> Result<(), ProgramError> {
    let encoded = wincode::serialize(value).map_err(|_| StakingError::InvalidAccount)?;
    if data.len() != encoded.len() {
        return Err(StakingError::InvalidAccount.into());
    }
    data.copy_from_slice(&encoded);
    Ok(())
}

pub fn config_from_bytes(data: &[u8]) -> Result<Config, ProgramError> {
    let config: Config = decode(data)?;
    if config.discriminator != CONFIG_DISCRIMINATOR || data.len() != CONFIG_SIZE {
        return Err(StakingError::InvalidConfig.into());
    }
    Ok(config)
}

pub fn load_config(account: &AccountView) -> Result<Config, ProgramError> {
    if !account.owned_by(&crate::ID) {
        return Err(StakingError::InvalidAccount.into());
    }
    let config = config_from_bytes(&account.try_borrow()?)?;
    if *account.address() != pda::config(&config.stake_pool).0 {
        return Err(StakingError::InvalidSeeds.into());
    }
    Ok(config)
}

pub fn write_config(data: &mut [u8], config: &Config) -> Result<(), ProgramError> {
    encode(config, data)
}

pub fn epoch_from_bytes(data: &[u8]) -> Result<EpochHeader, ProgramError> {
    if data.len() != EPOCH_SIZE {
        return Err(StakingError::InvalidEpoch.into());
    }
    let header: EpochHeader = decode(
        data.get(..EPOCH_HEADER_SIZE)
            .ok_or(StakingError::InvalidEpoch)?,
    )?;
    if header.discriminator != EPOCH_DISCRIMINATOR {
        return Err(StakingError::InvalidEpoch.into());
    }
    Ok(header)
}

pub fn load_epoch(account: &AccountView) -> Result<EpochHeader, ProgramError> {
    if !account.owned_by(&crate::ID) {
        return Err(StakingError::InvalidAccount.into());
    }
    let header = epoch_from_bytes(&account.try_borrow()?)?;
    if *account.address() != pda::epoch(&header.config, header.epoch_id, header.page_index).0 {
        return Err(StakingError::InvalidSeeds.into());
    }
    Ok(header)
}

pub fn write_epoch(data: &mut [u8], header: &EpochHeader) -> Result<(), ProgramError> {
    encode(
        header,
        data.get_mut(..EPOCH_HEADER_SIZE)
            .ok_or(StakingError::InvalidEpoch)?,
    )
}

fn chain_range(index: u16) -> Result<core::ops::Range<usize>, ProgramError> {
    if usize::from(index) >= M {
        return Err(StakingError::InvalidChain.into());
    }
    let start = EPOCH_HEADER_SIZE + usize::from(index) * CHAIN_SIZE;
    Ok(start..start + CHAIN_SIZE)
}

pub fn read_chain(data: &[u8], index: u16) -> Result<HashChain, ProgramError> {
    decode(
        data.get(chain_range(index)?)
            .ok_or(StakingError::InvalidChain)?,
    )
}

pub fn write_chain(data: &mut [u8], index: u16, chain: &HashChain) -> Result<(), ProgramError> {
    encode(
        chain,
        data.get_mut(chain_range(index)?)
            .ok_or(StakingError::InvalidChain)?,
    )
}

pub fn append_intent(data: &mut [u8], utxo_hash: [u8; 32], fee: u64) -> Result<u16, ProgramError> {
    let mut header = epoch_from_bytes(data)?;
    if header.locked {
        return Err(StakingError::EpochLocked.into());
    }
    let mut selected = None;
    for index in 0..M {
        let index = u16::try_from(index).map_err(|_| StakingError::Overflow)?;
        let chain = read_chain(data, index)?;
        if usize::from(chain.intent_count) < N && !chain.executed && !chain.batch_exists {
            selected = Some((index, chain));
            break;
        }
    }
    let (index, mut chain) = selected.ok_or(StakingError::EpochFull)?;
    chain.utxo_hash_chain = Poseidon::hashv(&[&chain.utxo_hash_chain, &utxo_hash])
        .map_err(|_| StakingError::HashingFailed)?;
    chain.intent_count = chain
        .intent_count
        .checked_add(1)
        .ok_or(StakingError::Overflow)?;
    header.intent_count = header
        .intent_count
        .checked_add(1)
        .ok_or(StakingError::Overflow)?;
    header.fee_balance_lamports = header
        .fee_balance_lamports
        .checked_add(fee)
        .ok_or(StakingError::Overflow)?;
    write_chain(data, index, &chain)?;
    write_epoch(data, &header)?;
    Ok(index)
}

pub fn batch_from_bytes(data: &[u8]) -> Result<BatchHeader, ProgramError> {
    if data.len() != BATCH_SIZE {
        return Err(StakingError::InvalidAccount.into());
    }
    let header: BatchHeader = decode(
        data.get(..BATCH_HEADER_SIZE)
            .ok_or(StakingError::InvalidAccount)?,
    )?;
    if header.discriminator != BATCH_DISCRIMINATOR || usize::from(header.filled_count) > N {
        return Err(StakingError::InvalidAccount.into());
    }
    Ok(header)
}

pub fn load_batch(account: &AccountView) -> Result<BatchHeader, ProgramError> {
    if !account.owned_by(&crate::ID) {
        return Err(StakingError::InvalidAccount.into());
    }
    let header = batch_from_bytes(&account.try_borrow()?)?;
    if *account.address() != pda::batch(&header.epoch, header.hash_chain_index).0 {
        return Err(StakingError::InvalidSeeds.into());
    }
    Ok(header)
}

pub fn write_batch(data: &mut [u8], header: &BatchHeader) -> Result<(), ProgramError> {
    encode(
        header,
        data.get_mut(..BATCH_HEADER_SIZE)
            .ok_or(StakingError::InvalidAccount)?,
    )
}

fn entry_range(index: usize) -> Result<core::ops::Range<usize>, ProgramError> {
    if index >= N {
        return Err(StakingError::BatchFull.into());
    }
    let start = BATCH_HEADER_SIZE + index * ENTRY_SIZE;
    Ok(start..start + ENTRY_SIZE)
}

pub fn read_entry(data: &[u8], index: usize) -> Result<BatchEntry, ProgramError> {
    let header = batch_from_bytes(data)?;
    if index >= usize::from(header.filled_count) {
        return Err(StakingError::InvalidAccount.into());
    }
    let slot = data
        .get(entry_range(index)?)
        .ok_or(StakingError::InvalidAccount)?;
    let length = usize::from(u16::from_le_bytes(
        slot.get(..2)
            .ok_or(StakingError::InvalidAccount)?
            .try_into()
            .map_err(|_| StakingError::InvalidAccount)?,
    ));
    decode(
        slot.get(2..2 + length)
            .ok_or(StakingError::InvalidAccount)?,
    )
}

pub fn append_entries(data: &mut [u8], entries: &[BatchEntry]) -> Result<(), ProgramError> {
    let mut header = batch_from_bytes(data)?;
    let total = usize::from(header.filled_count)
        .checked_add(entries.len())
        .ok_or(StakingError::Overflow)?;
    if entries.is_empty() || total > N || total > usize::from(header.intent_count) {
        return Err(StakingError::BatchFull.into());
    }
    let mut encoded_entries = Vec::with_capacity(entries.len());
    for entry in entries {
        if let Some(output) = &entry.1 {
            if !matches!(output.owner_tag, OwnerTag::Inline(_)) {
                return Err(StakingError::InvalidOutput.into());
            }
            let ciphertext = output
                .data
                .as_ref()
                .ok_or(StakingError::InvalidCiphertext)?;
            if ciphertext.len() > MAX_CIPHERTEXT_BYTES {
                return Err(StakingError::InvalidCiphertext.into());
            }
        }
        let encoded =
            wincode::serialize(entry).map_err(|_| StakingError::InvalidInstructionData)?;
        if encoded.len() > ENTRY_SIZE - 2 {
            return Err(StakingError::InvalidOutput.into());
        }
        encoded_entries.push(encoded);
    }
    for (index, encoded) in encoded_entries.into_iter().enumerate() {
        let slot = data
            .get_mut(entry_range(usize::from(header.filled_count) + index)?)
            .ok_or(StakingError::InvalidAccount)?;
        slot.fill(0);
        let length = u16::try_from(encoded.len()).map_err(|_| StakingError::Overflow)?;
        slot.get_mut(..2)
            .ok_or(StakingError::InvalidAccount)?
            .copy_from_slice(&length.to_le_bytes());
        slot.get_mut(2..2 + encoded.len())
            .ok_or(StakingError::InvalidAccount)?
            .copy_from_slice(&encoded);
    }
    header.filled_count = u16::try_from(total).map_err(|_| StakingError::Overflow)?;
    write_batch(data, &header)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address(byte: u8) -> Address {
        Address::new_from_array([byte; 32])
    }

    fn crank() -> ShieldedAddress {
        ShieldedAddress {
            signing_pubkey: address(7),
            nullifier_pubkey: [8; 32],
            viewing_pubkey: [2; 33],
        }
    }

    fn epoch_data() -> Vec<u8> {
        let mut data = vec![0; EPOCH_SIZE];
        write_epoch(
            &mut data,
            &EpochHeader {
                discriminator: EPOCH_DISCRIMINATOR,
                config: address(1),
                epoch_id: 4,
                page_index: 0,
                submission_end_slot: 400,
                settlement_deadline: 0,
                locked: false,
                intent_count: 0,
                resolved_intent_count: 0,
                fee_balance_lamports: 0,
            },
        )
        .unwrap();
        data
    }

    fn batch_data(count: u16) -> Vec<u8> {
        let mut data = vec![0; BATCH_SIZE];
        write_batch(
            &mut data,
            &BatchHeader {
                discriminator: BATCH_DISCRIMINATOR,
                epoch: address(2),
                hash_chain_index: 0,
                utxo_hash_chain: [3; 32],
                intent_count: count,
                crank: crank(),
                rent_payer: address(4),
                tx_viewing_pk: [2; 33],
                salt: [5; 16],
                filled_count: 0,
            },
        )
        .unwrap();
        data
    }

    fn entry(byte: u8, cancelled: bool) -> BatchEntry {
        (
            InputUtxo {
                nullifier_hash: [byte; 32],
                tree_index: 0,
            },
            if cancelled {
                None
            } else {
                Some(TransactOutput {
                    utxo_hash: [byte; 32],
                    owner_tag: OwnerTag::Inline([byte; 32]),
                    data: Some(vec![byte; MAX_CIPHERTEXT_BYTES]),
                })
            },
        )
    }

    #[test]
    fn serialized_layouts_match_preallocated_account_sizes() {
        let config = Config {
            discriminator: CONFIG_DISCRIMINATOR,
            authority: address(1),
            stake_pool_program_id: address(2),
            stake_pool: address(3),
            pool_mint: address(4),
            token_program_id: address(5),
            escrow_tree: address(6),
            crank: crank(),
            intent_fee_lamports: 10,
            settlement_window_seconds: 600,
        };
        let sizes = (
            wincode::serialize(&config).unwrap().len(),
            wincode::serialize(&epoch_from_bytes(&epoch_data()).unwrap())
                .unwrap()
                .len(),
            wincode::serialize(&HashChain::default()).unwrap().len(),
            wincode::serialize(&batch_from_bytes(&batch_data(16)).unwrap())
                .unwrap()
                .len(),
        );
        assert_eq!(
            sizes,
            (
                CONFIG_SIZE,
                EPOCH_HEADER_SIZE,
                CHAIN_SIZE,
                BATCH_HEADER_SIZE
            )
        );
        const { assert!(EPOCH_SIZE <= 10_240 && EPOCH_SIZE + CHAIN_SIZE > 10_240) };
        const { assert!(BATCH_SIZE <= 10_240) };
    }

    #[test]
    fn full_chain_rolls_over_without_client_selected_index() {
        let mut data = epoch_data();
        let hash = zolana_hasher::primitives::right_align(&[1]);
        for _ in 0..N {
            assert_eq!(append_intent(&mut data, hash, 10).unwrap(), 0);
        }
        assert_eq!(append_intent(&mut data, hash, 10).unwrap(), 1);
        assert_eq!(
            (
                epoch_from_bytes(&data).unwrap().intent_count,
                epoch_from_bytes(&data).unwrap().fee_balance_lamports,
                read_chain(&data, 0).unwrap().intent_count,
                read_chain(&data, 1).unwrap().intent_count
            ),
            (17, 170, 16, 1)
        );
    }

    #[test]
    fn account_capacity_remains_fixed_while_appending_maximum_ciphertexts() {
        let mut data = batch_data(16);
        let expected: Vec<_> = (1..=16).map(|byte| entry(byte, false)).collect();
        append_entries(&mut data, expected.get(..5).unwrap()).unwrap();
        append_entries(&mut data, expected.get(5..).unwrap()).unwrap();
        let actual: Vec<_> = (0..N)
            .map(|index| read_entry(&data, index).unwrap())
            .collect();
        assert_eq!(
            (
                data.len(),
                batch_from_bytes(&data).unwrap().filled_count,
                actual
            ),
            (BATCH_SIZE, 16, expected)
        );
        let before = data.clone();
        assert_eq!(
            append_entries(&mut data, &[entry(20, false)]),
            Err(StakingError::BatchFull.into())
        );
        assert_eq!(data, before);
    }

    #[test]
    fn cancelled_entry_has_no_payout_record() {
        let mut data = batch_data(2);
        let expected = vec![entry(1, true), entry(2, false)];
        append_entries(&mut data, &expected).unwrap();
        assert_eq!(
            (read_entry(&data, 0).unwrap(), read_entry(&data, 1).unwrap()),
            (
                expected.first().unwrap().clone(),
                expected.get(1).unwrap().clone()
            )
        );
    }

    #[test]
    fn invalid_output_does_not_partially_append_an_earlier_valid_entry() {
        let mut data = batch_data(2);
        let mut oversized = entry(2, false);
        oversized.1.as_mut().unwrap().data = Some(vec![0; MAX_CIPHERTEXT_BYTES + 1]);
        let before = data.clone();
        assert_eq!(
            append_entries(&mut data, &[entry(1, false), oversized]),
            Err(StakingError::InvalidCiphertext.into())
        );
        assert_eq!(data, before);
    }

    #[test]
    fn cancelled_and_settled_counts_resolve_each_intent_once() {
        let mut data = epoch_data();
        let mut epoch = epoch_from_bytes(&data).unwrap();
        epoch.intent_count = 5;
        epoch.resolve(2).unwrap();
        epoch.resolve(3).unwrap();
        write_epoch(&mut data, &epoch).unwrap();
        assert_eq!(epoch_from_bytes(&data).unwrap().resolved_intent_count, 5);
        assert_eq!(
            epoch.resolve(1),
            Err(StakingError::UnresolvedIntents.into())
        );
        assert_eq!(epoch.resolved_intent_count, 5);
    }
}
