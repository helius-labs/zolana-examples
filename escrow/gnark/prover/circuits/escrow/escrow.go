package escrow

import (
	"circuits/escrowterms"

	"github.com/consensys/gnark/frontend"

	gnarksdk "github.com/helius-labs/zolana/sdk-libs/gnark-sdk"
)

type Circuit struct {
	Public PublicInputs

	Terms escrowterms.EscrowTerms

	Source     gnarksdk.Utxo
	EscrowUtxo gnarksdk.Utxo
	Change     gnarksdk.Utxo

	OwnerPkField frontend.Variable
	NullifierPk  frontend.Variable

	PrivateTxBlinding frontend.Variable
}

func (c *Circuit) Define(api frontend.API) error {
	c.checkCreator(api)
	sourceInputUtxoHash := c.checkSourceInputUtxo(api)
	escrowOutputUtxoHash := c.checkEscrowOutputUtxo(api)
	changeOutputUtxoHash := c.checkChangeOutputUtxo(api)

	privateTxHash := gnarksdk.PrivateTxHash(
		api,
		[]frontend.Variable{sourceInputUtxoHash, 0},
		[]frontend.Variable{changeOutputUtxoHash, escrowOutputUtxoHash},
		c.PrivateTxBlinding,
	)
	api.AssertIsEqual(privateTxHash, c.Public.PrivateTxHash)

	c.Public.Check(api, c.OwnerPkField)
	return nil
}

// PublicInputs folds the private tx hash, the escrow utxo owner and the
// creator's owner identity into the circuit's single public input. The
// program supplies EscrowOwnerHash as the owner hash of the creator's
// escrow-authority PDA (nullifier secret 0) and OwnerPkField from the creator
// signer.
type PublicInputs struct {
	PublicInputHash frontend.Variable `gnark:",public"`

	PrivateTxHash   frontend.Variable
	EscrowOwnerHash frontend.Variable
}

func (p PublicInputs) Check(api frontend.API, ownerPkField frontend.Variable) {
	publicInputHash := gnarksdk.Poseidon(api, p.PrivateTxHash, p.EscrowOwnerHash, ownerPkField)
	api.AssertIsEqual(p.PublicInputHash, publicInputHash)
}

// checkCreator binds the escrow terms to the creator signer: the terms'
// owner hash must be built from the signer's identity, so the creator who
// withdraws derives the same escrow-authority PDA that owns the escrow utxo.
func (c *Circuit) checkCreator(api frontend.API) {
	recomputedOwnerHash := gnarksdk.Poseidon(api, c.OwnerPkField, c.NullifierPk)
	api.AssertIsEqual(recomputedOwnerHash, c.Terms.OwnerHash)
}

// checkSourceInputUtxo binds the spent source to the creator: a plain token
// utxo owned by the creator signer, in the escrow's asset. SPP requires the
// owner of a spent utxo to sign, so the creator signs the transaction itself,
// and no utxo of the creator's escrow-authority PDA (the program signs for
// it) can fund an escrow.
func (c *Circuit) checkSourceInputUtxo(api frontend.API) frontend.Variable {
	c.Source.AssertDefaultRing(api)
	api.AssertIsEqual(c.Source.DataHash, 0)
	api.AssertIsEqual(c.Source.Owner, c.Terms.OwnerHash)
	api.AssertIsEqual(c.Source.Asset, c.EscrowUtxo.Asset)
	return c.Source.Hash(api)
}

func (c *Circuit) checkEscrowOutputUtxo(api frontend.API) frontend.Variable {
	c.EscrowUtxo.AssertDefaultRing(api)
	api.AssertIsEqual(c.EscrowUtxo.Owner, c.Public.EscrowOwnerHash)
	api.AssertIsEqual(c.EscrowUtxo.DataHash, c.Terms.DataHash(api))
	api.AssertIsDifferent(c.EscrowUtxo.Amount, 0)
	return c.EscrowUtxo.Hash(api)
}

func (c *Circuit) checkChangeOutputUtxo(api frontend.API) frontend.Variable {
	c.Change.AssertDefaultRing(api)
	api.AssertIsEqual(c.Change.DataHash, 0)
	api.AssertIsEqual(c.Change.Asset, c.EscrowUtxo.Asset)
	api.AssertIsEqual(c.Change.Owner, c.Terms.OwnerHash)
	return c.Change.Hash(api)
}
