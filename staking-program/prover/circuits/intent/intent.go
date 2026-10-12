package intent

import (
	"stakingcircuits/shared"

	"github.com/consensys/gnark/frontend"
	"zolana/gnarksdk"
	spp "zolana/prover/circuits/spp_transaction/shared"
	"zolana/prover/circuits/verifiable-encryption/aes"
	"zolana/prover/circuits/verifiable-encryption/p256"
)

// Fixed 2x2 funding transaction: the intent is output zero, followed by one
// data-free change output or compact padding. There are no address inputs.
type Circuit struct {
	PublicInputHash       frontend.Variable `gnark:",public"`
	PrivateTxHash         frontend.Variable
	PrivateTxBlinding     frontend.Variable
	EscrowHash            frontend.Variable
	EscrowOwnerIdentity   frontend.Variable
	EpochHash             frontend.Variable
	PoolMintHash          frontend.Variable
	CrankViewingHash      frontend.Variable
	TreeID                frontend.Variable
	InputCount            frontend.Variable
	OutputCount           frontend.Variable
	CiphertextContextHash frontend.Variable
	Input0                gnarksdk.Utxo
	Input1                gnarksdk.Utxo
	InputOwnerIdentity    [2]frontend.Variable
	InputNullifierPK      [2]frontend.Variable
	Escrow                shared.Note
	Intent                shared.IntentData
	Change                gnarksdk.Utxo
	Encryption            shared.Encryption
}

func (c *Circuit) Define(api frontend.API) error {
	api.AssertIsEqual(api.Mul(api.Sub(c.InputCount, 1), api.Sub(c.InputCount, 2)), 0)
	api.AssertIsEqual(api.Mul(api.Sub(c.OutputCount, 1), api.Sub(c.OutputCount, 2)), 0)
	escrowNullifierPK := gnarksdk.Poseidon(api, 0)
	api.AssertIsEqual(c.Escrow.NullifierPK, escrowNullifierPK)
	api.AssertIsEqual(shared.OwnerIdentity(api, c.Escrow.SigningPK), c.EscrowOwnerIdentity)
	api.AssertIsEqual(c.Escrow.TreeID, c.TreeID)
	api.AssertIsDifferent(c.Escrow.Amount, 0)
	dataHash := c.Intent.Hash(api)
	// Default-ring SPP outputs bind Solana owner tags.
	api.AssertIsDifferent(c.Intent.SigningPK[0], 0)
	escrow := c.Escrow.Utxo(api, dataHash)
	solAsset := gnarksdk.Poseidon(api, 0, 0)
	api.AssertIsEqual(api.Mul(api.Sub(escrow.Asset, solAsset), api.Sub(escrow.Asset, c.PoolMintHash)), 0)
	escrowHash := escrow.Hash(api)
	api.AssertIsEqual(c.EscrowHash, escrowHash)
	api.AssertIsEqual(gnarksdk.HashBytes(api, c.Intent.Epoch[:]), c.EpochHash)
	user := p256.CompressPubkey(api, c.Encryption.UserPoint)
	for i, b := range user {
		api.AssertIsEqual(b, c.Intent.ViewingPK[i])
	}
	crank := p256.CompressPubkey(api, c.Encryption.CrankPoint)
	api.AssertIsEqual(gnarksdk.HashBytes(api, crank[:]), c.CrankViewingHash)
	api.AssertIsEqual(c.Encryption.OutputIndex, 0)
	ciphertext := c.Encryption.Encrypt(api, aes.NewAESGadget(api), c.Escrow.Bytes(api, &c.Intent), escrowHash)
	ciphertextContext := gnarksdk.Poseidon(api, gnarksdk.HashBytes(api, ciphertext),
		gnarksdk.HashBytes(api, c.Encryption.TxViewingPK[:]), shared.Pack(api, c.Encryption.Salt[:]))
	api.AssertIsEqual(ciphertextContext, c.CiphertextContextHash)
	inputHashes := make([]frontend.Variable, 2)
	for i, input := range []gnarksdk.Utxo{c.Input0, c.Input1} {
		active := frontend.Variable(1)
		if i == 1 {
			active = api.IsZero(api.Sub(c.InputCount, 2))
		}
		assertDefaultRing(api, input, active)
		owner := gnarksdk.Poseidon(api, c.InputOwnerIdentity[i], c.InputNullifierPK[i])
		api.AssertIsEqual(api.Mul(active, api.Sub(owner, input.Owner)), 0)
		api.AssertIsDifferent(api.Select(active, api.Sub(c.InputOwnerIdentity[i], c.EscrowOwnerIdentity), 1), 0)
		inputHashes[i] = api.Select(active, input.Hash(api), 0)
	}
	assertDefaultRing(api, c.Change, api.IsZero(api.Sub(c.OutputCount, 2)))
	api.AssertIsEqual(c.Change.DataHash, 0)
	changeHash := api.Select(api.IsZero(api.Sub(c.OutputCount, 2)), c.Change.Hash(api), 0)
	api.AssertIsEqual(c.PrivateTxHash, gnarksdk.PrivateTxHash(api, inputHashes,
		[]frontend.Variable{escrowHash, changeHash}, c.PrivateTxBlinding))
	escrowOwner := gnarksdk.Poseidon(api, c.EscrowOwnerIdentity, escrowNullifierPK)
	api.AssertIsEqual(c.PublicInputHash, gnarksdk.Poseidon(api, c.PrivateTxHash, c.EscrowHash,
		escrowOwner, c.EscrowOwnerIdentity, c.EpochHash, c.PoolMintHash, c.CrankViewingHash,
		c.TreeID, c.InputCount, c.OutputCount, c.CiphertextContextHash))
	return nil
}

func assertDefaultRing(api frontend.API, utxo gnarksdk.Utxo, active frontend.Variable) {
	api.AssertIsEqual(api.Mul(active, api.Sub(utxo.Domain, spp.UtxoDomain)), 0)
	api.AssertIsEqual(api.Mul(active, utxo.RingDataHash), 0)
	api.AssertIsEqual(api.Mul(active, utxo.RingProgramID), 0)
}
