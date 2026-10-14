// Package settle_dvp is the settle_dvp circuit: the settlement authority
// swaps both legs atomically. Its SPP transaction has shape 3x4: input 0 is
// the DvP UTXO, inputs 1 and 2 are the delivery and payment deposits, and the
// outputs are the buyer's delivery, the seller's payment, the seller's
// delivery change and the buyer's payment change, in that order.
package settle_dvp

import (
	"circuits/dvpterms"

	"github.com/consensys/gnark/frontend"
	"github.com/consensys/gnark/std/math/cmp"

	"zolana/gnarksdk"
)

type Circuit struct {
	// PublicInputHash is Poseidon(PrivateTxHash, CreatorID, DvpID,
	// DvpAuthorityOwnerHash, SettlementAuthorityID, CurrentTimestamp), the
	// program's SettleDvpPublicInput.
	PublicInputHash frontend.Variable `gnark:",public"`

	PrivateTxHash         frontend.Variable
	CreatorID             frontend.Variable
	DvpID                 frontend.Variable
	DvpAuthorityOwnerHash frontend.Variable
	SettlementAuthorityID frontend.Variable
	// CurrentTimestamp is the settle instruction's timestamp, which the
	// program checks against the clock (TIME_WINDOW).
	CurrentTimestamp frontend.Variable

	Terms dvpterms.SwapDvp

	DvpUtxo         gnarksdk.Utxo
	DeliveryDeposit gnarksdk.Utxo
	PaymentDeposit  gnarksdk.Utxo

	BuyerSettlement  gnarksdk.Utxo
	SellerSettlement gnarksdk.Utxo
	SellerRefund     gnarksdk.Utxo
	BuyerRefund      gnarksdk.Utxo

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
			c.SettlementAuthorityID,
			c.CurrentTimestamp,
		),
		c.PublicInputHash,
	)

	c.Terms.CheckRanges(api)
	api.AssertIsEqual(c.Terms.DvpID(api), c.DvpID)
	api.AssertIsEqual(c.Terms.CreatorID, c.CreatorID)

	// Only the terms' settlement authority settles.
	api.AssertIsEqual(c.SettlementAuthorityID, c.Terms.SettlementAuthorityID)

	// Settlement window: not after expiry and, when set, not before the
	// earliest settlement timestamp. 63 bits keeps the comparisons
	// meaningful for a non-negative i64.
	api.ToBinary(c.CurrentTimestamp, 63)
	api.AssertIsEqual(cmp.IsLessOrEqual(api, c.CurrentTimestamp, c.Terms.ExpiryTimestamp), 1)
	afterEarliest := cmp.IsLessOrEqual(api, c.Terms.EarliestSettlementTimestamp, c.CurrentTimestamp)
	api.AssertIsEqual(api.Select(c.Terms.HasEarliestSettlement, afterEarliest, 1), 1)

	// Input 0 is the DvP UTXO. Spending it under the zero nullifier secret
	// gives the first nullifier, which closes the DvP for every later settle
	// or abort.
	dvpterms.AssertDvpUtxo(api, c.DvpUtxo, c.DvpID, c.DvpAuthorityOwnerHash)
	dvpUtxoHash := c.DvpUtxo.Hash(api)
	firstNullifier := gnarksdk.Poseidon(api, dvpUtxoHash, c.DvpUtxo.Blinding, 0)

	deliveryHash := c.assertDeposit(api, c.DeliveryDeposit, c.Terms.DeliveryAsset, c.Terms.DeliveryAmount)
	paymentHash := c.assertDeposit(api, c.PaymentDeposit, c.Terms.PaymentAsset, c.Terms.PaymentAmount)

	// Each leg's amount goes to the counterparty's settlement destination and
	// the surplus back to the funder's refund destination. A change output
	// without surplus is a zero-amount UTXO, not a dummy.
	buyerSettlementHash := assertOutput(
		api, c.BuyerSettlement, c.Terms.BuyerSettlementDestination,
		c.Terms.DeliveryAsset, c.Terms.DeliveryAmount,
	)
	sellerSettlementHash := assertOutput(
		api, c.SellerSettlement, c.Terms.SellerSettlementDestination,
		c.Terms.PaymentAsset, c.Terms.PaymentAmount,
	)
	sellerRefundHash := assertOutput(
		api, c.SellerRefund, c.Terms.SellerRefundDestination,
		c.Terms.DeliveryAsset, api.Sub(c.DeliveryDeposit.Amount, c.Terms.DeliveryAmount),
	)
	buyerRefundHash := assertOutput(
		api, c.BuyerRefund, c.Terms.BuyerRefundDestination,
		c.Terms.PaymentAsset, api.Sub(c.PaymentDeposit.Amount, c.Terms.PaymentAmount),
	)

	// Every output blinding derives from the settlement seed, so every party
	// can recompute and find its output from the terms alone.
	gnarksdk.AssertTransactionBlindings(
		api,
		firstNullifier,
		c.Terms.BlindingSeed(api, dvpterms.SettlementTag),
		c.PrivateTxBlinding,
		c.BuyerSettlement.Blinding,
		c.SellerSettlement.Blinding,
		c.SellerRefund.Blinding,
		c.BuyerRefund.Blinding,
	)

	privateTxHash := gnarksdk.PrivateTxHash(
		api,
		[]frontend.Variable{dvpUtxoHash, deliveryHash, paymentHash},
		[]frontend.Variable{buyerSettlementHash, sellerSettlementHash, sellerRefundHash, buyerRefundHash},
		c.PrivateTxBlinding,
	)
	api.AssertIsEqual(privateTxHash, c.PrivateTxHash)
	return nil
}

// assertDeposit checks a deposit input: a plain UTXO of the leg's asset held
// by the DvP authority, funded with at least the leg amount. Its 64-bit range
// check keeps the change Amount - legAmount within u64.
func (c *Circuit) assertDeposit(api frontend.API, deposit gnarksdk.Utxo, legAsset, legAmount frontend.Variable) frontend.Variable {
	deposit.AssertDefaultRing(api)
	api.AssertIsEqual(deposit.Owner, c.DvpAuthorityOwnerHash)
	api.AssertIsEqual(deposit.DataHash, 0)
	api.AssertIsEqual(deposit.Asset, legAsset)
	api.ToBinary(deposit.Amount, 64)
	api.AssertIsEqual(cmp.IsLessOrEqual(api, legAmount, deposit.Amount), 1)
	return deposit.Hash(api)
}

// assertOutput checks an output against its terms destination, asset and
// amount, with no data, and returns its hash.
func assertOutput(api frontend.API, output gnarksdk.Utxo, destination dvpterms.Destination, asset, amount frontend.Variable) frontend.Variable {
	output.AssertDefaultRing(api)
	api.AssertIsEqual(output.Owner, destination.OwnerHash)
	api.AssertIsEqual(output.Asset, asset)
	api.AssertIsEqual(output.Amount, amount)
	api.AssertIsEqual(output.DataHash, 0)
	return output.Hash(api)
}
