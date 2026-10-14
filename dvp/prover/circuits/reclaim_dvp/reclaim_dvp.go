// Package reclaim_dvp is the reclaim_dvp circuit: a party takes back its own
// deposit while the DvP is open. Its SPP transaction has shape 1x2:
// input 0 is the deposit, output 0 is the refund and output 1 is compact
// padding, so the private transaction hash folds [deposit] inputs and
// [refund, 0] outputs.
package reclaim_dvp

import (
	"circuits/dvpterms"

	"github.com/consensys/gnark/frontend"

	"zolana/gnarksdk"
)

type Circuit struct {
	// PublicInputHash is Poseidon(PrivateTxHash, CreatorID, DvpID,
	// DvpAuthorityOwnerHash, SignerID), the program's ReclaimDvpPublicInput.
	PublicInputHash frontend.Variable `gnark:",public"`

	PrivateTxHash         frontend.Variable
	CreatorID             frontend.Variable
	DvpID                 frontend.Variable
	DvpAuthorityOwnerHash frontend.Variable
	SignerID              frontend.Variable

	Terms dvpterms.SwapDvp
	// Leg is 0 for the seller's delivery leg and 1 for the buyer's payment
	// leg. A bit instead of an equality test on SignerID still works when
	// seller and buyer are the same key.
	Leg     frontend.Variable
	Deposit gnarksdk.Utxo
	Refund  gnarksdk.Utxo

	PrivateTxBlinding frontend.Variable
}

func (c *Circuit) Define(api frontend.API) error {
	api.AssertIsEqual(
		gnarksdk.Poseidon(api, c.PrivateTxHash, c.CreatorID, c.DvpID, c.DvpAuthorityOwnerHash, c.SignerID),
		c.PublicInputHash,
	)

	c.Terms.CheckRanges(api)
	api.AssertIsEqual(c.Terms.DvpID(api), c.DvpID)
	api.AssertIsEqual(c.Terms.CreatorID, c.CreatorID)

	// Only the party that funds a leg can reclaim it.
	api.AssertIsBoolean(c.Leg)
	api.AssertIsEqual(c.SignerID, api.Select(c.Leg, c.Terms.BuyerID, c.Terms.SellerID))
	legAsset := api.Select(c.Leg, c.Terms.PaymentAsset, c.Terms.DeliveryAsset)

	// The deposit is a plain leg UTXO held by the DvP authority. Its amount
	// is whatever the funder sent; the refund returns all of it.
	c.Deposit.AssertDefaultRing(api)
	api.AssertIsEqual(c.Deposit.Owner, c.DvpAuthorityOwnerHash)
	api.AssertIsEqual(c.Deposit.DataHash, 0)
	api.AssertIsEqual(c.Deposit.Asset, legAsset)
	depositHash := c.Deposit.Hash(api)

	// The refund's owner is free: the signer picks where its deposit goes.
	c.Refund.AssertDefaultRing(api)
	api.AssertIsEqual(c.Refund.Asset, c.Deposit.Asset)
	api.AssertIsEqual(c.Refund.Amount, c.Deposit.Amount)
	api.AssertIsEqual(c.Refund.DataHash, 0)
	refundHash := c.Refund.Hash(api)

	// The output blindings are random (no derived seed): nothing has to
	// find the refund but the signer, who builds it.
	privateTxHash := gnarksdk.PrivateTxHash(
		api,
		[]frontend.Variable{depositHash},
		[]frontend.Variable{refundHash, 0},
		c.PrivateTxBlinding,
	)
	api.AssertIsEqual(privateTxHash, c.PrivateTxHash)
	return nil
}
