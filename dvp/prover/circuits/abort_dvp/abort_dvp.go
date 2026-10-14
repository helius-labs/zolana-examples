// Package abort_dvp is the abort_dvp circuit, shared by cancel_dvp (the
// settlement authority aborts) and reject_dvp (a party aborts). Its SPP
// transaction has shape 3x2: input 0 is the DvP UTXO, inputs 1 and 2 hold the
// deposits or random padding, output 0 refunds the delivery leg to the
// seller's refund destination and output 1 the payment leg to the buyer's.
//
// D3: the deposits fill the input slots first and padding comes last, as
// SPP's dummies-last rule requires. Which leg sits in slot 1 is the private
// bit PaymentFirst, so the slot order does not reveal which legs were funded.
// The outputs stay fixed by leg; a leg without a deposit gets a zero-amount
// refund (D2). With both deposits PaymentFirst may take either value; with
// only the payment deposit it is 1 and slot 2 is padding.
package abort_dvp

import (
	"circuits/dvpterms"

	"github.com/consensys/gnark/frontend"

	"zolana/gnarksdk"
	spp "zolana/prover/circuits/spp_transaction/shared"
)

type Circuit struct {
	// PublicInputHash is Poseidon(PrivateTxHash, CreatorID, DvpID,
	// DvpAuthorityOwnerHash, SignerID, Mode), the program's
	// AbortDvpPublicInput.
	PublicInputHash frontend.Variable `gnark:",public"`

	PrivateTxHash         frontend.Variable
	CreatorID             frontend.Variable
	DvpID                 frontend.Variable
	DvpAuthorityOwnerHash frontend.Variable
	SignerID              frontend.Variable
	// Mode is 0 for cancel_dvp and 1 for reject_dvp; the program sets it from
	// the instruction tag.
	Mode frontend.Variable

	Terms dvpterms.SwapDvp

	// Leg names the rejecting party in reject mode: 0 for the seller, 1 for
	// the buyer. A bit rather than an equality test, so a seller who is also
	// the buyer can still reject. Unused in cancel mode.
	Leg frontend.Variable
	// PaymentFirst is 1 when input slot 1 carries the payment leg and slot 2
	// the delivery leg, 0 for the reverse (D3).
	PaymentFirst frontend.Variable
	// Padding1 and Padding2 mark input slots 1 and 2 as random padding.
	Padding1 frontend.Variable
	Padding2 frontend.Variable

	DvpUtxo gnarksdk.Utxo
	Slot1   gnarksdk.Utxo
	Slot2   gnarksdk.Utxo

	SellerRefund gnarksdk.Utxo
	BuyerRefund  gnarksdk.Utxo

	PrivateTxBlinding frontend.Variable
}

func (c *Circuit) Define(api frontend.API) error {
	api.AssertIsEqual(
		gnarksdk.Poseidon(
			api,
			c.PrivateTxHash,
			c.CreatorID,
			c.DvpID,
			c.DvpAuthorityOwnerHash,
			c.SignerID,
			c.Mode,
		),
		c.PublicInputHash,
	)

	c.Terms.CheckRanges(api)
	api.AssertIsEqual(c.Terms.DvpID(api), c.DvpID)
	api.AssertIsEqual(c.Terms.CreatorID, c.CreatorID)

	api.AssertIsBoolean(c.Mode)
	api.AssertIsBoolean(c.Leg)
	api.AssertIsBoolean(c.PaymentFirst)
	api.AssertIsBoolean(c.Padding1)
	api.AssertIsBoolean(c.Padding2)
	// Padding is last: a padding slot 1 forces a padding slot 2.
	api.AssertIsEqual(api.Mul(c.Padding1, api.Sub(1, c.Padding2)), 0)

	// Cancel: only the settlement authority. Reject: only the party Leg names.
	party := api.Select(c.Leg, c.Terms.BuyerID, c.Terms.SellerID)
	api.AssertIsEqual(c.SignerID, api.Select(c.Mode, party, c.Terms.SettlementAuthorityID))

	// Input 0 is the DvP UTXO; its nullifier is the first nullifier and
	// closes the DvP for every later settle or abort.
	dvpterms.AssertDvpUtxo(api, c.DvpUtxo, c.DvpID, c.DvpAuthorityOwnerHash)
	dvpUtxoHash := c.DvpUtxo.Hash(api)
	firstNullifier := gnarksdk.Poseidon(api, dvpUtxoHash, c.DvpUtxo.Blinding, 0)

	slot1Asset := api.Select(c.PaymentFirst, c.Terms.PaymentAsset, c.Terms.DeliveryAsset)
	slot2Asset := api.Select(c.PaymentFirst, c.Terms.DeliveryAsset, c.Terms.PaymentAsset)
	slot1Hash, slot1Amount := c.assertSlot(api, c.Slot1, c.Padding1, slot1Asset)
	slot2Hash, slot2Amount := c.assertSlot(api, c.Slot2, c.Padding2, slot2Asset)
	deliveryRefundAmount := api.Select(c.PaymentFirst, slot2Amount, slot1Amount)
	paymentRefundAmount := api.Select(c.PaymentFirst, slot1Amount, slot2Amount)

	sellerRefundHash := assertRefund(
		api, c.SellerRefund, c.Terms.SellerRefundDestination,
		c.Terms.DeliveryAsset, deliveryRefundAmount,
	)
	buyerRefundHash := assertRefund(
		api, c.BuyerRefund, c.Terms.BuyerRefundDestination,
		c.Terms.PaymentAsset, paymentRefundAmount,
	)

	// Both refund blindings derive from the abort seed, so each party can
	// recompute and find its refund from the terms alone.
	gnarksdk.AssertTransactionBlindings(
		api,
		firstNullifier,
		c.Terms.BlindingSeed(api, dvpterms.AbortTag),
		c.PrivateTxBlinding,
		c.SellerRefund.Blinding,
		c.BuyerRefund.Blinding,
	)

	// A padding slot is an SPP dummy, which contributes 0 to the private tx
	// hash.
	privateTxHash := gnarksdk.PrivateTxHash(
		api,
		[]frontend.Variable{dvpUtxoHash, slot1Hash, slot2Hash},
		[]frontend.Variable{sellerRefundHash, buyerRefundHash},
		c.PrivateTxBlinding,
	)
	api.AssertIsEqual(privateTxHash, c.PrivateTxHash)
	return nil
}

// assertSlot checks a non-padding input slot as a deposit of legAsset: a
// plain UTXO held by the DvP authority with no data. It returns the slot's
// private tx hash contribution and refund amount, both 0 for padding. The
// amount is range-checked to 64 bits either way.
func (c *Circuit) assertSlot(api frontend.API, slot gnarksdk.Utxo, padding, legAsset frontend.Variable) (frontend.Variable, frontend.Variable) {
	isDeposit := api.Sub(1, padding)
	assertWhen := func(a, b frontend.Variable) {
		api.AssertIsEqual(api.Mul(isDeposit, api.Sub(a, b)), 0)
	}
	assertWhen(slot.Domain, spp.UtxoDomain)
	assertWhen(slot.RingDataHash, 0)
	assertWhen(slot.RingProgramID, 0)
	assertWhen(slot.Owner, c.DvpAuthorityOwnerHash)
	assertWhen(slot.DataHash, 0)
	assertWhen(slot.Asset, legAsset)
	api.ToBinary(slot.Amount, 64)
	return api.Mul(isDeposit, slot.Hash(api)), api.Mul(isDeposit, slot.Amount)
}

// assertRefund checks a refund output against its terms destination, asset
// and amount, with no data, and returns its hash.
func assertRefund(api frontend.API, refund gnarksdk.Utxo, destination dvpterms.Destination, asset, amount frontend.Variable) frontend.Variable {
	refund.AssertDefaultRing(api)
	api.AssertIsEqual(refund.Owner, destination.OwnerHash)
	api.AssertIsEqual(refund.Asset, asset)
	api.AssertIsEqual(refund.Amount, amount)
	api.AssertIsEqual(refund.DataHash, 0)
	return refund.Hash(api)
}
