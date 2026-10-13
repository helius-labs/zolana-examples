package cancel

import (
	"stakingcircuits/shared"

	"github.com/consensys/gnark/frontend"
	"zolana/gnarksdk"
	"zolana/prover/circuits/verifiable-encryption/aes"
	"zolana/prover/circuits/verifiable-encryption/p256"
)

type Circuit struct {
	PublicInputHash       frontend.Variable `gnark:",public"`
	PrivateTxHash         frontend.Variable
	PrivateTxBlinding     frontend.Variable
	NullifierHash         frontend.Variable
	EscrowOwnerIdentity   frontend.Variable
	SignerIdentity        frontend.Variable
	EpochHash             frontend.Variable
	PoolMintHash          frontend.Variable
	CrankViewingHash      frontend.Variable
	InputTreeID           frontend.Variable
	OutputTreeID          frontend.Variable
	CiphertextContextHash frontend.Variable
	Escrow                shared.Note
	Intent                shared.IntentData
	Refund                shared.Note
	Encryption            shared.Encryption
}

func (c *Circuit) Define(api frontend.API) error {
	api.AssertIsEqual(shared.OwnerIdentity(api, c.Escrow.SigningPK), c.EscrowOwnerIdentity)
	api.AssertIsEqual(c.Escrow.NullifierPK, gnarksdk.Poseidon(api, 0))
	api.AssertIsEqual(c.Escrow.TreeID, c.InputTreeID)
	api.AssertIsDifferent(c.Escrow.Amount, 0)
	escrow := c.Escrow.Utxo(api, c.Intent.Hash(api))
	solAsset := gnarksdk.Poseidon(api, 0, 0)
	api.AssertIsEqual(api.Mul(api.Sub(escrow.Asset, solAsset), api.Sub(escrow.Asset, c.PoolMintHash)), 0)
	api.AssertIsEqual(gnarksdk.HashBytes(api, c.Intent.Epoch[:]), c.EpochHash)
	api.AssertIsEqual(shared.OwnerIdentity(api, c.Intent.SigningPK), c.SignerIdentity)
	api.AssertIsEqual(gnarksdk.Poseidon(api, escrow.Hash(api), c.Escrow.Blinding, 0), c.NullifierHash)
	api.AssertIsEqual(c.Refund.TreeID, c.OutputTreeID)
	api.AssertIsEqual(c.Refund.Amount, c.Escrow.Amount)
	api.AssertIsEqual(c.Refund.NullifierPK, c.Intent.NullifierPK)
	for i, b := range c.Refund.SigningPK {
		api.AssertIsEqual(b, c.Intent.SigningPK[i])
	}
	for i, b := range c.Refund.Asset {
		api.AssertIsEqual(b, c.Escrow.Asset[i])
	}
	refund := c.Refund.Utxo(api, 0)
	user := p256.CompressPubkey(api, c.Encryption.UserPoint)
	for i, b := range user {
		api.AssertIsEqual(b, c.Intent.ViewingPK[i])
	}
	crank := p256.CompressPubkey(api, c.Encryption.CrankPoint)
	api.AssertIsEqual(gnarksdk.HashBytes(api, crank[:]), c.CrankViewingHash)
	api.AssertIsEqual(c.Encryption.OutputIndex, 0)
	ciphertext := c.Encryption.Encrypt(api, aes.NewAESGadget(api), c.Refund.Bytes(api, nil), refund.Hash(api))
	api.AssertIsEqual(c.CiphertextContextHash, gnarksdk.Poseidon(api, gnarksdk.HashBytes(api, ciphertext),
		gnarksdk.HashBytes(api, c.Encryption.TxViewingPK[:]), shared.Pack(api, c.Encryption.Salt[:])))
	api.AssertIsEqual(c.PrivateTxHash, gnarksdk.PrivateTxHash(api, []frontend.Variable{escrow.Hash(api), 0},
		[]frontend.Variable{refund.Hash(api), 0}, c.PrivateTxBlinding))
	api.AssertIsEqual(c.PublicInputHash, gnarksdk.Poseidon(api, c.PrivateTxHash, c.NullifierHash,
		c.EscrowOwnerIdentity, c.SignerIdentity, c.EpochHash, c.PoolMintHash, c.CrankViewingHash,
		c.InputTreeID, c.OutputTreeID, c.CiphertextContextHash))
	return nil
}
