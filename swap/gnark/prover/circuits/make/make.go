package make

import (
	"circuits/orderterms"

	"github.com/consensys/gnark/frontend"

	"zolana/gnarksdk"
)

type Circuit struct {
	Public PublicInputs

	Order orderterms.OrderTerms

	OrderUtxo gnarksdk.Utxo
	Change    gnarksdk.Utxo

	// The maker's nullifier pubkey: with the public MakerOwnerPkField it forms
	// the preimage of Order.MakerOwnerHash.
	MakerNullifierPk frontend.Variable

	SourceInputHash   frontend.Variable
	PrivateTxBlinding frontend.Variable
}

func (c *Circuit) Define(api frontend.API) error {
	c.Order.Check(api)
	makerAddressFe := c.Order.MakerAddressFE(api)

	orderOutputUtxoHash := c.checkOrderOutputUtxo(api, makerAddressFe, c.Public.OrderOwnerHash)
	changeOutputUtxoHash := c.checkChangeOutputUtxo(api)
	c.checkMaker(api)

	privateTxHash := gnarksdk.PrivateTxHash(
		api,
		[]frontend.Variable{c.SourceInputHash, 0},
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
// derives the same order-authority PDA that owns the order utxo.
func (c *Circuit) checkMaker(api frontend.API) {
	recomputedOwnerHash := gnarksdk.Poseidon(api, c.Public.MakerOwnerPkField, c.MakerNullifierPk)
	api.AssertIsEqual(recomputedOwnerHash, c.Order.MakerOwnerHash)
}
