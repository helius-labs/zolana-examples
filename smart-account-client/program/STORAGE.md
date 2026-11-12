# Buffered byte storage

The program stores opaque bytes. It does not parse key records or check
whether an application has preserved its historical keys.

Uploads use fresh signer accounts. The first `store` fixes the owner,
predecessor and total length. Every call requires the buffer's signature,
appends at the current length, grows the account and updates
`data_hash = SHA256(data_hash || chunk)`, starting with 32 zero bytes.
Chunks are at most 1,024 bytes. The full account is at most 10 MiB.

`apply` requires the owner's signature, approved hash, expected predecessor
and expected head revision. It checks the complete upload and current head,
seals the replacement, updates `[b"head", owner]`, and increments the revision.
It closes the superseded record and transfers its entire lamport balance to
the apply rent payer. Publication, closure and refund are one atomic
instruction. A later failing instruction in the same transaction rolls all
three back, including the revision increment.

The initial expected predecessor is the zero address and the expected
revision is zero. The first publication creates revision one. The head is
never closed or reset. Its monotonic revision prevents stale approvals
succeeding if a closed record address is reused.

The active record cannot be appended, reapplied or separately closed. An
unpublished buffer can be cancelled by its signer. The client carries all
old viewing keys inside the replacement before approving apply, so closing
the predecessor does not remove those keys. Superseded records are no longer
available for history traversal.

## Wire format

Integers are little endian; addresses and hashes are 32 bytes.

| Tag       | Arguments                                                       | Accounts                                                                                                                        |
| --------- | --------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------- |
| store (1) | owner, predecessor, total length u32, offset u32, chunk         | buffer writable signer, rent payer writable signer, System Program                                                              |
| apply (2) | expected predecessor, expected revision u64, approved data hash | head writable, buffer writable, owner signer, rent payer/refund recipient writable signer, System Program, predecessor writable |
| close (3) | none                                                            | unpublished buffer writable signer, refund recipient writable                                                                   |

The initial apply uses the readonly System Program as the predecessor
placeholder; there is no predecessor account to close.

Buffer header (109 bytes): `STORBUF1`, sealed u8, owner, predecessor,
total length u32, data hash, followed by uploaded bytes.
The predecessor is an update precondition, not a permanent history link.
Head (48 bytes): `STORHEAD`, current record address, revision u64.

## Correctness checks

- Fresh uploads accept only empty System Program accounts, including prefunded
  accounts. Existing accounts must have the correct owner and discriminator.
- Changed metadata, skipped/repeated offsets, excess bytes, incomplete uploads
  and incorrect hashes fail without changing the current record.
- Only the owner can apply. Signed predecessor and revision checks reject
  stale updates, including after pending or published addresses are reused.
- The account closed by apply must be the exact current predecessor, writable,
  owned by this program, sealed and associated with the same owner. It must
  differ from the head, replacement and refund recipient.
- The old record stays intact throughout uploading. A successful apply closes
  it and refunds its full balance; failures preserve all accounts.
- Revision or refund overflow fails without closing the current record.
- Clients validate key-record structure, encryption and historical-key
  preservation. Those application checks are not part of this program.

This replaces the earlier set_data protocol. Existing `[b"data", owner]`
accounts remain intact; automatic migration is outside this change.
