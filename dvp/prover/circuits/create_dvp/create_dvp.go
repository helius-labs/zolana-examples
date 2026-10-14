// Package create_dvp is the create_dvp circuit. Its SPP transaction has shape
// 1x2 (D1): input 0 is the address slot that reserves the DvP's address,
// output 0 is the DvP UTXO and output 1 is compact padding, so the private
// transaction hash folds [0] inputs, [DvP UTXO, 0] outputs and [address].
package create_dvp

import (
	"circuits/dvpterms"

	"github.com/consensys/gnark/frontend"

	"zolana/gnarksdk"
	spp "zolana/prover/circuits/spp_transaction/shared"
)

// DvpAddressTreeID is dvp_program::DVP_ADDRESS_TREE_ID. Pinning the address
// slot's tree (D6) makes the address unique per DvP: an address exists once
// per owner, seed and tree, so with a free tree id the same terms could be
// created once per tree.
const DvpAddressTreeID = 0

type Circuit struct {
	// PublicInputHash is Poseidon(PrivateTxHash, CreatorID, DvpID,
	// DvpAuthorityOwnerHash), the program's CreateDvpPublicInput.
	PublicInputHash frontend.Variable `gnark:",public"`

	PrivateTxHash         frontend.Variable
	CreatorID             frontend.Variable
	DvpID                 frontend.Variable
	DvpAuthorityOwnerHash frontend.Variable

	Terms   dvpterms.SwapDvp
	DvpUtxo gnarksdk.Utxo

	PrivateTxBlinding frontend.Variable
}

func (c *Circuit) Define(api frontend.API) error {
	api.AssertIsEqual(
		gnarksdk.Poseidon(api, c.PrivateTxHash, c.CreatorID, c.DvpID, c.DvpAuthorityOwnerHash),
		c.PublicInputHash,
	)

	c.Terms.CheckRanges(api)
	c.Terms.CheckCreate(api)
	api.AssertIsEqual(c.Terms.DvpID(api), c.DvpID)
	api.AssertIsEqual(c.Terms.CreatorID, c.CreatorID)

	address := c.address(api)

	dvpterms.AssertDvpUtxo(api, c.DvpUtxo, c.DvpID, c.DvpAuthorityOwnerHash)
	dvpUtxoHash := c.DvpUtxo.Hash(api)

	// The address slot's nullifier is the first nullifier. The compact
	// padding output's blinding is derived too, but the transact proof checks
	// it, since its hash is 0 and never reaches the private transaction hash.
	gnarksdk.AssertTransactionBlindings(
		api,
		address,
		c.Terms.BlindingSeed(api, dvpterms.CreateTag),
		c.PrivateTxBlinding,
		c.DvpUtxo.Blinding,
	)

	privateTxHash := spp.PrivateTxHashCircuit(
		api,
		[]frontend.Variable{0},
		[]frontend.Variable{dvpUtxoHash, 0},
		[]frontend.Variable{address},
		c.PrivateTxBlinding,
	)
	api.AssertIsEqual(privateTxHash, c.PrivateTxHash)
	return nil
}

// address is the compressed address the address slot creates
// (zolana_program::compression::NewAddress): the slot's nullifier
// Poseidon(slotHash, DvpID, 0) under nullifier secret 0. The slot is owned by
// the DvP authority, uses DvpID as its seed and lives in the pinned tree.
func (c *Circuit) address(api frontend.API) frontend.Variable {
	slot := gnarksdk.Utxo{
		Domain:        spp.AddressDomain,
		Owner:         c.DvpAuthorityOwnerHash,
		Asset:         0,
		Amount:        0,
		Blinding:      c.DvpID,
		DataHash:      0,
		RingDataHash:  0,
		RingProgramID: 0,
		TreeID:        DvpAddressTreeID,
	}
	return gnarksdk.Poseidon(api, slot.Hash(api), c.DvpID, 0)
}
