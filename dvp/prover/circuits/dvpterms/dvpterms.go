// Package dvpterms holds the SwapDvp terms every DvP circuit takes as private
// witnesses, the dvp_id hash over them (D4) and the checks the circuits share.
// The Rust twin is dvp-sdk's state/swap_dvp.rs.
package dvpterms

import (
	"math/big"

	"github.com/consensys/gnark/frontend"
	"github.com/consensys/gnark/std/math/cmp"

	"zolana/gnarksdk"
)

// Blinding seed domain tags, u32::from_be_bytes of "DVPC", "DVPS" and "DVPA"
// (dvp_program CREATE_TAG, SETTLEMENT_TAG, ABORT_TAG).
const (
	CreateTag     uint64 = 0x44565043
	SettlementTag uint64 = 0x44565053
	AbortTag      uint64 = 0x44565041
)

// utxoDomain is the domain tag of a spendable UTXO (spp.UtxoDomain).
const utxoDomain = 3

// solAssetField is zolana_interface::SOL_ASSET_FIELD, Poseidon(0, 0): the
// asset of the zero-amount SOL DvP UTXO.
var solAssetField, _ = new(big.Int).SetString("2098f5fb9e239eab3ceac3f27b81e481dc3124d55ffed523a839ee8446b64864", 16)

// Destination is a shielded destination address as its compressed hash
// preimage: the owner hash and hash_bytes of the 33-byte viewing pubkey.
type Destination struct {
	OwnerHash     frontend.Variable
	ViewingPkHash frontend.Variable
}

// Hash is CompressedShieldedAddress::hash, Poseidon(OwnerHash, ViewingPkHash).
func (d Destination) Hash(api frontend.API) frontend.Variable {
	return gnarksdk.Poseidon(api, d.OwnerHash, d.ViewingPkHash)
}

// SwapDvp is the DvP terms as field elements. Party fields are Solana owner
// identities, assets are hash_bytes(mint), and RefStringHash is
// hash_bytes(ref_string[64]). EarliestSettlementTimestamp is 0 when
// HasEarliestSettlement is 0.
type SwapDvp struct {
	SellerID                    frontend.Variable
	BuyerID                     frontend.Variable
	SettlementAuthorityID       frontend.Variable
	CreatorID                   frontend.Variable
	DeliveryAsset               frontend.Variable
	PaymentAsset                frontend.Variable
	DeliveryAmount              frontend.Variable
	PaymentAmount               frontend.Variable
	ExpiryTimestamp             frontend.Variable
	HasEarliestSettlement       frontend.Variable
	EarliestSettlementTimestamp frontend.Variable
	Nonce                       frontend.Variable
	RefStringHash               frontend.Variable
	SellerSettlementDestination Destination
	BuyerSettlementDestination  Destination
	SellerRefundDestination     Destination
	BuyerRefundDestination      Destination
	Blinding                    frontend.Variable
}

// DvpID is the D4 layout, nested because Poseidon takes at most 12 inputs.
func (t SwapDvp) DvpID(api frontend.API) frontend.Variable {
	parties := gnarksdk.Poseidon(api, t.SellerID, t.BuyerID, t.SettlementAuthorityID, t.CreatorID)
	legs := gnarksdk.Poseidon(api, t.DeliveryAsset, t.PaymentAsset, t.DeliveryAmount, t.PaymentAmount)
	schedule := gnarksdk.Poseidon(api, t.ExpiryTimestamp, t.HasEarliestSettlement, t.EarliestSettlementTimestamp, t.Nonce)
	destinations := gnarksdk.Poseidon(
		api,
		t.SellerSettlementDestination.Hash(api),
		t.BuyerSettlementDestination.Hash(api),
		t.SellerRefundDestination.Hash(api),
		t.BuyerRefundDestination.Hash(api),
	)
	return gnarksdk.Poseidon(api, parties, legs, schedule, destinations, t.RefStringHash, t.Blinding)
}

// CheckRanges bounds the amounts to 64 bits and the timestamps to 63 bits
// (D5: non-negative i64), so the comparisons below are well defined and the
// amounts match the u64 the shielded pool commits. An unset earliest
// settlement timestamp must be 0, so the flag fully determines it.
func (t SwapDvp) CheckRanges(api frontend.API) {
	api.ToBinary(t.DeliveryAmount, 64)
	api.ToBinary(t.PaymentAmount, 64)
	api.ToBinary(t.ExpiryTimestamp, 63)
	api.ToBinary(t.EarliestSettlementTimestamp, 63)
	api.AssertIsBoolean(t.HasEarliestSettlement)
	api.AssertIsEqual(api.Mul(api.Sub(1, t.HasEarliestSettlement), t.EarliestSettlementTimestamp), 0)
}

// CheckCreate is what create_dvp accepts beyond CheckRanges, which it
// assumes: both legs move value, the legs are different assets, and a set
// earliest settlement timestamp is not after expiry.
func (t SwapDvp) CheckCreate(api frontend.API) {
	api.AssertIsDifferent(t.DeliveryAmount, 0)
	api.AssertIsDifferent(t.PaymentAmount, 0)
	api.AssertIsDifferent(t.DeliveryAsset, t.PaymentAsset)
	earliestBeforeExpiry := cmp.IsLessOrEqual(api, t.EarliestSettlementTimestamp, t.ExpiryTimestamp)
	api.AssertIsEqual(api.Select(t.HasEarliestSettlement, earliestBeforeExpiry, 1), 1)
}

// DvpUtxo is the DvP UTXO (D9: no ciphertext): a zero-amount SOL UTXO owned
// by the dvp_authority owner hash whose data hash is dvp_id. It takes the
// dvp_id the circuit already computed with DvpID instead of hashing the terms
// a second time.
func DvpUtxo(dvpID, ownerHash, blinding, treeID frontend.Variable) gnarksdk.Utxo {
	return gnarksdk.Utxo{
		Domain:        utxoDomain,
		Owner:         ownerHash,
		Asset:         solAssetField,
		Amount:        0,
		Blinding:      blinding,
		DataHash:      dvpID,
		RingDataHash:  0,
		RingProgramID: 0,
		TreeID:        treeID,
	}
}

// AssertDvpUtxo asserts the witness u is the DvP UTXO of dvpID under
// ownerHash. Only its blinding and tree id are free: the blinding is checked
// by the caller's blinding derivation, the tree id by the transact proof.
func AssertDvpUtxo(api frontend.API, u gnarksdk.Utxo, dvpID, ownerHash frontend.Variable) {
	expected := DvpUtxo(dvpID, ownerHash, u.Blinding, u.TreeID)
	api.AssertIsEqual(u.Domain, expected.Domain)
	api.AssertIsEqual(u.Owner, expected.Owner)
	api.AssertIsEqual(u.Asset, expected.Asset)
	api.AssertIsEqual(u.Amount, expected.Amount)
	api.AssertIsEqual(u.DataHash, expected.DataHash)
	api.AssertIsEqual(u.RingDataHash, expected.RingDataHash)
	api.AssertIsEqual(u.RingProgramID, expected.RingProgramID)
}

// BlindingSeed is Poseidon(tag, Blinding), the seed of one instruction's
// output blindings.
func (t SwapDvp) BlindingSeed(api frontend.API, tag uint64) frontend.Variable {
	return gnarksdk.Poseidon(api, tag, t.Blinding)
}
