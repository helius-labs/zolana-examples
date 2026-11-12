use data_registry::{
    state::{
        decode_head, encode_head, BufferHeader, Head, BUFFER_HEADER_LEN, HEAD_LEN, HEAD_PDA_SEED,
        MAX_CHUNK_LEN, MAX_RECORD_LEN,
    },
    StorageError, APPLY, CLOSE, STORE,
};
use mollusk_svm::{
    result::{types::TransactionProgramResult, ProgramResult},
    Mollusk,
};
use solana_account::Account;
use solana_instruction::{AccountMeta, Instruction};
use solana_program_error::ProgramError;
use solana_pubkey::Pubkey;
use solana_sha256_hasher::hashv;

const SYSTEM: Pubkey = Pubkey::new_from_array([0; 32]);

struct Fixture {
    svm: Mollusk,
    program: Pubkey,
    owner: Pubkey,
    payer: Pubkey,
    head: Pubkey,
    buffer: Pubkey,
    accounts: Vec<(Pubkey, Account)>,
}

impl Fixture {
    fn new() -> Self {
        let program = Pubkey::new_from_array([11; 32]);
        let owner = Pubkey::new_from_array([12; 32]);
        let payer = Pubkey::new_from_array([13; 32]);
        let buffer = Pubkey::new_from_array([14; 32]);
        let (head, _) = Pubkey::find_program_address(&[HEAD_PDA_SEED, owner.as_ref()], &program);
        let mut svm = Mollusk::default();
        let executable = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/target/deploy/data_registry.so"
        ))
        .expect("build the SBF program before running integration tests");
        svm.add_program_with_loader_and_elf(
            &program,
            &mollusk_svm::program::loader_keys::LOADER_V3,
            &executable,
        );
        let accounts = vec![
            (owner, system_account(1_000_000)),
            (payer, system_account(1_000_000_000)),
            (head, system_account(0)),
            (buffer, system_account(0)),
            mollusk_svm::program::keyed_account_for_system_program(),
        ];
        Self {
            svm,
            program,
            owner,
            payer,
            head,
            buffer,
            accounts,
        }
    }

    fn account(&self, address: Pubkey) -> &Account {
        &self
            .accounts
            .iter()
            .find(|(key, _)| *key == address)
            .expect("fixture account")
            .1
    }

    fn account_mut(&mut self, address: Pubkey) -> &mut Account {
        &mut self
            .accounts
            .iter_mut()
            .find(|(key, _)| *key == address)
            .expect("fixture account")
            .1
    }

    fn store(&self, previous: Pubkey, total: u32, offset: u32, chunk: &[u8]) -> Instruction {
        let mut instruction_data = vec![STORE];
        instruction_data.extend_from_slice(self.owner.as_ref());
        instruction_data.extend_from_slice(previous.as_ref());
        instruction_data.extend_from_slice(&total.to_le_bytes());
        instruction_data.extend_from_slice(&offset.to_le_bytes());
        instruction_data.extend_from_slice(chunk);
        Instruction {
            program_id: self.program,
            accounts: vec![
                AccountMeta::new(self.buffer, true),
                AccountMeta::new(self.payer, true),
                AccountMeta::new_readonly(SYSTEM, false),
            ],
            data: instruction_data,
        }
    }

    fn apply(&self, data_hash: [u8; 32]) -> Instruction {
        let mut instruction_data = vec![APPLY];
        instruction_data.extend_from_slice(self.header().previous.as_ref());
        let revision = if self.account(self.head).data.is_empty() {
            0
        } else {
            decode_head(&self.account(self.head).data).unwrap().revision
        };
        instruction_data.extend_from_slice(&revision.to_le_bytes());
        instruction_data.extend_from_slice(&data_hash);
        let previous = Pubkey::new_from_array(*self.header().previous.as_array());
        Instruction {
            program_id: self.program,
            accounts: vec![
                AccountMeta::new(self.head, false),
                AccountMeta::new(self.buffer, false),
                AccountMeta::new_readonly(self.owner, true),
                AccountMeta::new(self.payer, true),
                AccountMeta::new_readonly(SYSTEM, false),
                if previous == SYSTEM {
                    AccountMeta::new_readonly(SYSTEM, false)
                } else {
                    AccountMeta::new(previous, false)
                },
            ],
            data: instruction_data,
        }
    }

    fn close(&self) -> Instruction {
        Instruction {
            program_id: self.program,
            accounts: vec![
                AccountMeta::new(self.buffer, true),
                AccountMeta::new(self.payer, false),
            ],
            data: vec![CLOSE],
        }
    }

    fn succeed(&mut self, instruction: Instruction) {
        let execution = self.svm.process_instruction(&instruction, &self.accounts);
        assert_eq!(execution.program_result, ProgramResult::Success);
        self.accounts = execution.resulting_accounts;
    }

    fn reject(&self, instruction: Instruction, error: ProgramError) {
        let execution = self.svm.process_instruction(&instruction, &self.accounts);
        assert_eq!(
            (execution.program_result, execution.resulting_accounts),
            (ProgramResult::Failure(error), self.accounts.clone()),
            "failure must return the exact error and preserve every account",
        );
    }

    fn upload(&mut self, previous: Pubkey, bytes: &[u8]) -> [u8; 32] {
        let mut data_hash = [0; 32];
        for (chunk_index, chunk) in bytes.chunks(MAX_CHUNK_LEN).enumerate() {
            let offset = chunk_index * MAX_CHUNK_LEN;
            self.succeed(self.store(previous, bytes.len() as u32, offset as u32, chunk));
            data_hash = hashv(&[&data_hash, chunk]).to_bytes();
        }
        data_hash
    }

    fn next_buffer(&mut self) -> Pubkey {
        let previous = self.buffer;
        self.buffer = Pubkey::new_unique();
        self.accounts.push((self.buffer, system_account(0)));
        previous
    }

    fn header(&self) -> BufferHeader {
        BufferHeader::decode(&self.account(self.buffer).data).unwrap()
    }
}

fn system_account(lamports: u64) -> Account {
    Account {
        lamports,
        ..Account::default()
    }
}

fn custom(error: StorageError) -> ProgramError {
    ProgramError::Custom(error as u32)
}

#[test]
fn chunked_growth_preserves_the_active_record_until_atomic_replacement() {
    let mut fixture = Fixture::new();
    let first_bytes = b"first key record";
    let first_hash = fixture.upload(SYSTEM, first_bytes);
    fixture.succeed(fixture.apply(first_hash));
    let first_address = fixture.next_buffer();
    let first_account = fixture.account(first_address).clone();
    let previous_head = fixture.account(fixture.head).clone();
    let next_bytes = vec![37; 25_000];
    let next_hash = fixture.upload(first_address, &next_bytes);
    assert_eq!(
        next_hash,
        [
            54, 151, 119, 163, 141, 154, 45, 198, 56, 44, 61, 166, 218, 223, 51, 97, 241, 251, 228,
            184, 187, 207, 125, 13, 73, 252, 126, 21, 183, 231, 131, 83
        ]
    );
    assert_eq!(
        (
            fixture.account(first_address),
            fixture.account(fixture.head)
        ),
        (&first_account, &previous_head),
        "uploads must not change the published record or head",
    );
    let pending_header = fixture.header();
    assert_eq!(
        pending_header,
        BufferHeader {
            sealed: false,
            owner: pinocchio::Address::new_from_array(fixture.owner.to_bytes()),
            previous: pinocchio::Address::new_from_array(first_address.to_bytes()),
            total_length: next_bytes.len() as u32,
            data_hash: next_hash,
        },
    );
    let payer_balance = fixture.account(fixture.payer).lamports;
    fixture.succeed(fixture.apply(next_hash));
    let mut closed_first = first_account.clone();
    closed_first.lamports = 0;
    closed_first.data.clear();
    assert_eq!(
        (
            decode_head(&fixture.account(fixture.head).data).unwrap(),
            fixture.header(),
            fixture
                .account(fixture.buffer)
                .data
                .get(BUFFER_HEADER_LEN..)
                .unwrap(),
            fixture.account(first_address),
        ),
        (
            Head {
                current: pinocchio::Address::new_from_array(fixture.buffer.to_bytes()),
                revision: 2
            },
            BufferHeader {
                sealed: true,
                ..pending_header
            },
            next_bytes.as_slice(),
            &closed_first,
        ),
        "publication must seal the replacement and close only its predecessor",
    );
    assert_eq!(
        fixture.account(fixture.payer).lamports,
        payer_balance + first_account.lamports
    );
    assert_eq!(fixture.account(fixture.head).data.len(), HEAD_LEN);
}

#[test]
fn prefunded_buffer_and_head_cannot_block_initialization() {
    let mut fixture = Fixture::new();
    fixture.account_mut(fixture.buffer).lamports = 1_000_000;
    fixture.account_mut(fixture.head).lamports = 1_000_000;
    let data_hash = fixture.upload(SYSTEM, b"record");
    fixture.succeed(fixture.apply(data_hash));
    assert!(fixture.header().sealed);
}

#[test]
fn the_owner_can_also_pay_head_rent_as_in_a_squads_cpi() {
    let mut fixture = Fixture::new();
    fixture.account_mut(fixture.owner).lamports = 20_000_000;
    let data_hash = fixture.upload(SYSTEM, b"record");
    let mut instruction = fixture.apply(data_hash);
    instruction.accounts.get_mut(3).unwrap().pubkey = fixture.owner;
    fixture.succeed(instruction);
    assert!(fixture.header().sealed);
    let first = fixture.next_buffer();
    let next_hash = fixture.upload(first, b"replacement");
    let owner_balance = fixture.account(fixture.owner).lamports;
    let payer_balance = fixture.account(fixture.payer).lamports;
    let predecessor_balance = fixture.account(first).lamports;
    let mut instruction = fixture.apply(next_hash);
    instruction.accounts.get_mut(3).unwrap().pubkey = fixture.owner;
    fixture.succeed(instruction);
    assert_eq!(
        (
            fixture.account(fixture.owner).lamports,
            fixture.account(fixture.payer).lamports,
            fixture.account(first).lamports
        ),
        (owner_balance + predecessor_balance, payer_balance, 0),
        "the vault receives the refund when it is the apply rent payer",
    );
}

#[test]
fn repeated_or_skipped_offsets_fail() {
    let mut fixture = Fixture::new();
    fixture.succeed(fixture.store(SYSTEM, 8, 0, b"abc"));
    for offset in [0, 2, 4, u32::MAX] {
        fixture.reject(
            fixture.store(SYSTEM, 8, offset, b"d"),
            custom(StorageError::InvalidOffset),
        );
    }
}

#[test]
fn upload_metadata_cannot_be_changed() {
    let mut fixture = Fixture::new();
    fixture.succeed(fixture.store(SYSTEM, 8, 0, b"abc"));
    fixture.reject(
        fixture.store(SYSTEM, 9, 3, b"d"),
        custom(StorageError::MetadataMismatch),
    );
    fixture.reject(
        fixture.store(Pubkey::new_unique(), 8, 3, b"d"),
        custom(StorageError::MetadataMismatch),
    );
    let mut instruction = fixture.store(SYSTEM, 8, 3, b"d");
    instruction.data.get_mut(1..33).unwrap().fill(99);
    fixture.reject(instruction, custom(StorageError::MetadataMismatch));
}

#[test]
fn invalid_lengths_fail_before_mutating_accounts() {
    let fixture = Fixture::new();
    for (total, chunk) in [
        (0, vec![1]),
        (MAX_RECORD_LEN as u32 + 1, vec![1]),
        (1, vec![]),
        (1, vec![1, 2]),
        (2048, vec![1; MAX_CHUNK_LEN + 1]),
    ] {
        fixture.reject(
            fixture.store(SYSTEM, total, 0, &chunk),
            custom(StorageError::InvalidLength),
        );
    }
}

#[test]
fn store_requires_buffer_and_payer_signatures() {
    let fixture = Fixture::new();
    for signer_index in [0, 1] {
        let mut instruction = fixture.store(SYSTEM, 3, 0, b"abc");
        instruction
            .accounts
            .get_mut(signer_index)
            .unwrap()
            .is_signer = false;
        fixture.reject(instruction, ProgramError::MissingRequiredSignature);
    }
}

#[test]
fn readonly_buffers_are_rejected() {
    let fixture = Fixture::new();
    let mut instruction = fixture.store(SYSTEM, 3, 0, b"abc");
    instruction.accounts.first_mut().unwrap().is_writable = false;
    fixture.reject(instruction, ProgramError::InvalidAccountData);
}

#[test]
fn foreign_buffer_owners_and_discriminators_are_rejected() {
    let mut fixture = Fixture::new();
    fixture.account_mut(fixture.buffer).owner = Pubkey::new_unique();
    fixture.reject(
        fixture.store(SYSTEM, 3, 0, b"abc"),
        ProgramError::InvalidAccountData,
    );
    fixture.account_mut(fixture.buffer).owner = fixture.program;
    fixture.account_mut(fixture.buffer).data = vec![0; BUFFER_HEADER_LEN];
    fixture.reject(
        fixture.store(SYSTEM, 3, 0, b"abc"),
        ProgramError::InvalidAccountData,
    );
}

#[test]
fn incomplete_buffers_cannot_be_applied() {
    let mut fixture = Fixture::new();
    fixture.succeed(fixture.store(SYSTEM, 8, 0, b"abc"));
    fixture.reject(
        fixture.apply(fixture.header().data_hash),
        custom(StorageError::InvalidLength),
    );
}

#[test]
fn the_approved_hash_must_match_the_uploaded_bytes() {
    let mut fixture = Fixture::new();
    fixture.upload(SYSTEM, b"abc");
    fixture.reject(fixture.apply([99; 32]), custom(StorageError::HashMismatch));
}

#[test]
fn apply_requires_the_owners_signature() {
    let mut fixture = Fixture::new();
    let data_hash = fixture.upload(SYSTEM, b"abc");
    let mut instruction = fixture.apply(data_hash);
    instruction.accounts.get_mut(2).unwrap().is_signer = false;
    fixture.reject(instruction, ProgramError::MissingRequiredSignature);
}

#[test]
fn a_buffer_for_another_owner_cannot_be_published() {
    let mut fixture = Fixture::new();
    let mut instruction = fixture.store(SYSTEM, 3, 0, b"abc");
    instruction.data.get_mut(1..33).unwrap().fill(98);
    fixture.succeed(instruction);
    fixture.reject(
        fixture.apply(fixture.header().data_hash),
        ProgramError::IllegalOwner,
    );
}

#[test]
fn the_head_must_be_the_owners_canonical_pda() {
    let mut fixture = Fixture::new();
    let data_hash = fixture.upload(SYSTEM, b"abc");
    let alternate = Pubkey::new_unique();
    fixture.accounts.push((alternate, system_account(0)));
    let mut instruction = fixture.apply(data_hash);
    instruction.accounts.first_mut().unwrap().pubkey = alternate;
    fixture.reject(instruction, ProgramError::InvalidSeeds);
}

#[test]
fn stale_uploads_cannot_replace_a_newer_head() {
    let mut fixture = Fixture::new();
    let first_hash = fixture.upload(SYSTEM, b"first");
    let first = fixture.next_buffer();
    let stale_hash = fixture.upload(SYSTEM, b"competing");
    let stale = fixture.buffer;
    fixture.buffer = first;
    fixture.succeed(fixture.apply(first_hash));
    fixture.buffer = stale;
    fixture.reject(fixture.apply(stale_hash), custom(StorageError::StaleHead));
}

#[test]
fn the_active_record_cannot_be_appended_reapplied_or_separately_closed() {
    let mut fixture = Fixture::new();
    let data_hash = fixture.upload(SYSTEM, b"abc");
    fixture.succeed(fixture.apply(data_hash));
    for instruction in [
        fixture.store(SYSTEM, 3, 3, b"x"),
        fixture.apply(data_hash),
        fixture.close(),
    ] {
        fixture.reject(instruction, custom(StorageError::Sealed));
    }
}

#[test]
fn recreating_a_cancelled_buffer_cannot_reuse_an_old_approval() {
    let mut fixture = Fixture::new();
    let approved_hash = fixture.upload(SYSTEM, b"approved record");
    let old_approval = fixture.apply(approved_hash);
    let cancelled = fixture.buffer;
    fixture.succeed(fixture.close());
    fixture.next_buffer();
    let intervening_hash = fixture.upload(SYSTEM, b"intervening record");
    fixture.succeed(fixture.apply(intervening_hash));
    let intervening = fixture.buffer;
    // A zero-lamport account is purged between transactions by the cluster.
    *fixture.account_mut(cancelled) = system_account(0);
    fixture.buffer = cancelled;
    assert_eq!(
        fixture.upload(intervening, b"approved record"),
        approved_hash
    );
    fixture.reject(old_approval, custom(StorageError::StaleHead));
}

#[test]
fn reusing_a_superseded_address_cannot_revive_a_stale_approval() {
    let mut fixture = Fixture::new();
    let original_hash = fixture.upload(SYSTEM, b"original");
    fixture.succeed(fixture.apply(original_hash));
    let original = fixture.next_buffer();
    let stale_hash = fixture.upload(original, b"stale update");
    let stale_approval = fixture.apply(stale_hash);
    let stale = fixture.buffer;
    fixture.next_buffer();
    let intervening_hash = fixture.upload(original, b"intervening");
    fixture.succeed(fixture.apply(intervening_hash));
    let intervening = fixture.buffer;
    assert_eq!(fixture.account(original).lamports, 0);
    // Simulate the cluster purging the closed account between transactions.
    *fixture.account_mut(original) = system_account(0);
    fixture.buffer = original;
    assert_eq!(fixture.upload(intervening, b"original"), original_hash);
    fixture.succeed(fixture.apply(original_hash));
    assert_eq!(
        decode_head(&fixture.account(fixture.head).data)
            .unwrap()
            .revision,
        3
    );
    fixture.buffer = stale;
    fixture.reject(stale_approval, custom(StorageError::StaleHead));
    fixture.succeed(fixture.apply(stale_hash));
}

#[test]
fn replacement_requires_the_exact_writable_predecessor() {
    let mut fixture = Fixture::new();
    let first_hash = fixture.upload(SYSTEM, b"first");
    fixture.succeed(fixture.apply(first_hash));
    let first = fixture.next_buffer();
    let next_hash = fixture.upload(first, b"next");
    let mut substituted = fixture.apply(next_hash);
    substituted.accounts.get_mut(5).unwrap().pubkey = fixture.owner;
    fixture.reject(substituted, ProgramError::InvalidArgument);
    let mut readonly = fixture.apply(next_hash);
    readonly.accounts.get_mut(5).unwrap().is_writable = false;
    fixture.reject(readonly, ProgramError::InvalidAccountData);
}

#[test]
fn replacement_rejects_a_foreign_or_unsealed_predecessor() {
    let mut fixture = Fixture::new();
    let first_hash = fixture.upload(SYSTEM, b"first");
    fixture.succeed(fixture.apply(first_hash));
    let first = fixture.next_buffer();
    let next_hash = fixture.upload(first, b"next");
    let original = fixture.account(first).clone();
    fixture.account_mut(first).owner = fixture.owner;
    fixture.reject(fixture.apply(next_hash), ProgramError::IllegalOwner);
    *fixture.account_mut(first) = original.clone();
    let mut header = BufferHeader::decode(&original.data).unwrap();
    header.owner = pinocchio::Address::new_from_array([99; 32]);
    header.encode(&mut fixture.account_mut(first).data).unwrap();
    fixture.reject(fixture.apply(next_hash), ProgramError::IllegalOwner);
    *fixture.account_mut(first) = original;
    let mut header = BufferHeader::decode(&fixture.account(first).data).unwrap();
    header.sealed = false;
    header.encode(&mut fixture.account_mut(first).data).unwrap();
    fixture.reject(fixture.apply(next_hash), ProgramError::InvalidAccountData);
}

#[test]
fn revision_overflow_cannot_close_the_active_record() {
    let mut fixture = Fixture::new();
    let first_hash = fixture.upload(SYSTEM, b"first");
    fixture.succeed(fixture.apply(first_hash));
    let first = fixture.next_buffer();
    let next_hash = fixture.upload(first, b"next");
    encode_head(
        &mut fixture.account_mut(fixture.head).data,
        &pinocchio::Address::new_from_array(first.to_bytes()),
        u64::MAX,
    )
    .unwrap();
    fixture.reject(fixture.apply(next_hash), ProgramError::ArithmeticOverflow);
}

#[test]
fn refund_overflow_rolls_back_publication_and_closure() {
    let mut fixture = Fixture::new();
    let first_hash = fixture.upload(SYSTEM, b"first");
    fixture.succeed(fixture.apply(first_hash));
    let first = fixture.next_buffer();
    let next_hash = fixture.upload(first, b"next");
    fixture.account_mut(fixture.payer).lamports = u64::MAX;
    fixture.reject(fixture.apply(next_hash), ProgramError::ArithmeticOverflow);
}

#[test]
fn closing_an_unpublished_buffer_refunds_its_full_rent() {
    let mut fixture = Fixture::new();
    fixture.upload(SYSTEM, b"abc");
    let total_lamports =
        fixture.account(fixture.payer).lamports + fixture.account(fixture.buffer).lamports;
    fixture.succeed(fixture.close());
    assert_eq!(
        (
            fixture.account(fixture.buffer).lamports,
            fixture.account(fixture.buffer).data.as_slice(),
            fixture.account(fixture.payer).lamports
        ),
        (0, &[][..], total_lamports),
    );
}

#[test]
fn closing_requires_the_buffer_signature_and_a_distinct_recipient() {
    let mut fixture = Fixture::new();
    fixture.upload(SYSTEM, b"abc");
    let mut unsigned = fixture.close();
    unsigned.accounts.first_mut().unwrap().is_signer = false;
    fixture.reject(unsigned, ProgramError::MissingRequiredSignature);
    let mut aliased = fixture.close();
    aliased.accounts.get_mut(1).unwrap().pubkey = fixture.buffer;
    fixture.reject(aliased, ProgramError::InvalidArgument);
}

#[test]
fn wrong_system_program_and_aliased_payer_are_rejected() {
    let fixture = Fixture::new();
    let mut wrong_program = fixture.store(SYSTEM, 3, 0, b"abc");
    wrong_program.accounts.get_mut(2).unwrap().pubkey = fixture.owner;
    fixture.reject(wrong_program, ProgramError::IncorrectProgramId);
    let mut aliased = fixture.store(SYSTEM, 3, 0, b"abc");
    aliased.accounts.get_mut(1).unwrap().pubkey = fixture.buffer;
    fixture.reject(aliased, ProgramError::InvalidArgument);
}

#[test]
fn malformed_instructions_fail_without_account_changes() {
    let fixture = Fixture::new();
    for instruction_data in [
        vec![],
        vec![0],
        vec![255],
        vec![STORE],
        vec![APPLY],
        vec![CLOSE, 0],
    ] {
        fixture.reject(
            Instruction {
                program_id: fixture.program,
                accounts: vec![],
                data: instruction_data,
            },
            ProgramError::InvalidInstructionData,
        );
    }
}

#[test]
fn apply_rolls_back_if_a_later_instruction_fails() {
    let mut fixture = Fixture::new();
    let first_hash = fixture.upload(SYSTEM, b"first");
    fixture.succeed(fixture.apply(first_hash));
    let first = fixture.next_buffer();
    let next_hash = fixture.upload(first, b"next");
    let before = fixture.accounts.clone();
    let execution = fixture.svm.process_transaction_instructions(
        &[
            fixture.apply(next_hash),
            Instruction {
                program_id: fixture.program,
                accounts: vec![],
                data: vec![255],
            },
        ],
        &fixture.accounts,
    );
    assert_eq!(
        execution.program_result,
        TransactionProgramResult::Failure(1, ProgramError::InvalidInstructionData)
    );
    assert_eq!(
        execution.resulting_accounts, before,
        "both publication and sealing must roll back"
    );
    fixture.succeed(fixture.apply(next_hash));
    assert!(fixture.header().sealed);
}
