package make

import (
	"circuits/orderterms"

	"github.com/consensys/gnark/frontend"

	gnarksdk "github.com/helius-labs/zolana/sdk-libs/gnark-sdk"
)

type Circuit struct {
	Public PublicInputs

	Order orderterms.OrderTerms

	// The maker's spent source UTXO. SPP authorizes the spend with the maker's
	// signature; its owner is the order's maker and receives the change.
	Source    gnarksdk.Utxo
	OrderUtxo gnarksdk.Utxo
	Change    gnarksdk.Utxo

	// The maker's nullifier pubkey: with the public MakerOwnerPkField it forms
	// the preimage of Order.MakerOwnerHash.
	MakerNullifierPk frontend.Variable

	PrivateTxBlinding frontend.Variable
}

func (c *Circuit) Define(api frontend.API) error {
	c.Order.Check(api)
	makerAddressFe := c.Order.MakerAddressFE(api)

	c.checkMaker(api)
	sourceInputUtxoHash := c.checkSourceInputUtxo(api)
	orderOutputUtxoHash := c.checkOrderOutputUtxo(api, makerAddressFe, c.Public.OrderOwnerHash)
	changeOutputUtxoHash := c.checkChangeOutputUtxo(api)

	privateTxHash := gnarksdk.PrivateTxHash(
		api,
		[]frontend.Variable{sourceInputUtxoHash, 0},
		[]frontend.Variable{changeOutputUtxoHash, orderOutputUtxoHash},
		c.PrivateTxBlinding,
	)
	api.AssertIsEqual(privateTxHash, c.Public.PrivateTxHash)

	c.Public.Check(api)
	return nil
}

type PublicInputs struct {
	PublicInputHash frontend.Variable `gnark:",public"`

	PrivateTxHash frontend.Variable
	// The owner hash of the maker's order-authority PDA, which the program
	// derives from the maker signer. Binding the order UTXO to it keeps the
	// maker, a transaction signer SPP would otherwise accept as owner, from
	// spending the order outside the program.
	OrderOwnerHash frontend.Variable
	// The maker signer's Solana owner identity, computed by the program.
	MakerOwnerPkField frontend.Variable
}

func (p PublicInputs) Check(api frontend.API) {
	publicInputHash := gnarksdk.Poseidon(api, p.PrivateTxHash, p.OrderOwnerHash, p.MakerOwnerPkField)
	api.AssertIsEqual(p.PublicInputHash, publicInputHash)
}

// checkSourceInputUtxo binds the spent source to the maker signer: it must be
// a plain token UTXO owned by Order.MakerOwnerHash, which checkMaker binds to
// the signer, and hold the order's asset. A UTXO owned by the maker's
// order-authority PDA, such as an open order, has another owner hash, so the
// maker cannot spend it through make.
func (c *Circuit) checkSourceInputUtxo(api frontend.API) frontend.Variable {
	c.Source.AssertDefaultRing(api)
	api.AssertIsEqual(c.Source.DataHash, 0)
	api.AssertIsEqual(c.Source.Owner, c.Order.MakerOwnerHash)
	api.AssertIsEqual(c.Source.Asset, c.OrderUtxo.Asset)
	return c.Source.Hash(api)
}

func (c *Circuit) checkOrderOutputUtxo(api frontend.API, makerAddressFe, orderOwnerHash frontend.Variable) frontend.Variable {
	c.OrderUtxo.AssertDefaultRing(api)
	api.AssertIsEqual(c.OrderUtxo.Owner, orderOwnerHash)
	api.AssertIsEqual(c.OrderUtxo.DataHash, c.Order.DataHash(api, makerAddressFe))
	api.AssertIsDifferent(c.OrderUtxo.Amount, 0)
	return c.OrderUtxo.Hash(api)
}

func (c *Circuit) checkChangeOutputUtxo(api frontend.API) frontend.Variable {
	c.Change.AssertDefaultRing(api)
	api.AssertIsEqual(c.Change.DataHash, 0)
	api.AssertIsEqual(c.Change.Asset, c.OrderUtxo.Asset)
	api.AssertIsEqual(c.Change.Owner, c.Order.MakerOwnerHash)
	return c.Change.Hash(api)
}

// checkMaker binds the order terms to the maker signer: the terms' maker owner
// hash must be built from the signer's identity, so the maker who cancels
// derives the same order-authority PDA that owns the order utxo, and the
// source the maker spends is the signer's own.
func (c *Circuit) checkMaker(api frontend.API) {
	recomputedOwnerHash := gnarksdk.Poseidon(api, c.Public.MakerOwnerPkField, c.MakerNullifierPk)
	api.AssertIsEqual(recomputedOwnerHash, c.Order.MakerOwnerHash)
}
