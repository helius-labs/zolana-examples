# Buffered byte storage

The program stores opaque bytes. It does not parse key records or check
whether an application has preserved its historical keys.

Uploads use fresh signer accounts. The first `store` fixes the owner, the
rent payer, the refund destination, the predecessor and the total length.
Every call requires the buffer's and the same rent payer's signatures, names
the same destination, appends at the current length, grows the account and
updates `data_hash = SHA256(data_hash || chunk)`, starting with 32 zero bytes.
Every chunk except the last is exactly 768 bytes, so the hash depends only on
the bytes, and one `store` fits a 1,232-byte transaction with two signers, a
separate destination and compute budget instructions. The full account is at
most 10 MiB.

The rent payer funds the buffer; the refund destination gets that rent back.
They can differ, so a sponsor can pay for an upload and send the rent back to
itself or to anyone else. The destination must be a non-executable System
Program account other than the buffer when the upload starts.

`apply` requires the owner's signature, approved hash, expected predecessor
and expected head revision. It checks the complete upload and current head,
seals the replacement, updates `[b"head", owner]`, and increments the revision.
It closes the superseded record and transfers its entire lamport balance to
that record's refund destination. Publication, closure and refund are one
atomic instruction. A later failing instruction in the same transaction rolls
all three back, including the revision increment.

A replacement cannot name its owner's head as destination. If a destination
has since become a deployed program, it can no longer be credited, so apply
sends that refund to the apply rent payer instead. A record therefore always
stays replaceable, whatever happens to its destination.

The initial expected predecessor is the zero address and the expected
revision is zero. The first publication creates revision one. The head is
never closed or reset. Its monotonic revision prevents stale approvals
succeeding if a closed record address is reused.

The active record cannot be appended, reapplied or separately closed. The
rent payer of an unpublished buffer can cancel it, refunding the destination;
the buffer keypair is not needed. The client carries all old viewing keys
inside the replacement before approving apply, so closing the predecessor does
not remove those keys.

## Wire format

Integers are little endian; addresses and hashes are 32 bytes.

| Tag       | Arguments                                                       | Accounts                                                                                                                                                  |
| --------- | --------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------- |
| store (1) | owner, predecessor, total length u32, offset u32, chunk         | buffer writable signer, rent payer writable signer, System Program, refund destination                                                                    |
| apply (2) | expected predecessor, expected revision u64, approved data hash | head writable, buffer writable, owner signer, rent payer writable signer, System Program, predecessor writable, predecessor's refund destination writable |
| close (3) | none                                                            | unpublished buffer writable, its rent payer signer, its refund destination writable                                                                       |

The initial apply passes the readonly System Program in both predecessor
slots; there is nothing to close or refund. The apply rent payer funds only
the head on the first publication.

Buffer header (173 bytes): `STORBUF1`, sealed u8, owner, rent payer, refund
destination, predecessor, total length u32, data hash, followed by uploaded
bytes. The predecessor is an update precondition, not a permanent history
link. Head (48 bytes): `STORHEAD`, current record address, revision u64.

## Correctness checks

- Fresh uploads accept only empty System Program accounts, including prefunded
  accounts. Existing accounts must have the correct owner and discriminator.
- Changed metadata, rent payer or destination, skipped/repeated offsets, short
  non-final chunks, excess bytes, incomplete uploads and incorrect hashes fail
  without changing the current record.
- Only the owner can apply. Signed predecessor and revision checks reject
  stale updates, including after pending or published addresses are reused.
- The account closed by apply must be the exact current predecessor, writable,
  owned by this program, sealed and associated with the same owner. Its refund
  goes only to its recorded destination, or to the apply rent payer if that
  destination is now executable. Both must differ from the head and the
  replacement.
- Closing hands the account back to the System Program with no data, so later
  instructions in the same transaction cannot load it.
- The old record stays intact throughout uploading. A successful apply closes
  it and refunds its full balance; failures preserve all accounts.
- Revision or refund overflow fails without closing the current record.
- Clients validate key-record structure, encryption and historical-key
  preservation. Those application checks are not part of this program.
