package shared

import (
	"math/big"

	"github.com/consensys/gnark/frontend"
	"zolana/gnarksdk"
	spp "zolana/prover/circuits/spp_transaction/shared"
	ve "zolana/prover/circuits/verifiable-encryption"
)

// The owner identity is algorithm-tagged exactly as SPP's PublicKey encoding.
// Ed25519 and PDA keys share their public identity; P-256 uses its x-coordinate.
func OwnerIdentity(api frontend.API, key [34]frontend.Variable) frontend.Variable {
	for _, b := range key {
		api.ToBinary(b, 8)
	}
	api.AssertIsEqual(api.Mul(key[0], api.Sub(key[0], 1), api.Sub(key[0], 2)), 0)
	p256 := api.IsZero(key[0])
	api.AssertIsEqual(api.Mul(p256, api.Sub(key[1], 2), api.Sub(key[1], 3)), 0)
	api.AssertIsEqual(api.Mul(api.Sub(1, p256), key[33]), 0)
	tagged := make([]frontend.Variable, 33)
	tagged[0] = api.Select(p256, 0x50, 0x53)
	for i := 0; i < 32; i++ {
		tagged[i+1] = api.Select(p256, key[i+2], key[i+1])
	}
	return gnarksdk.HashBytes(api, tagged)
}

type IntentData struct {
	Epoch       [32]frontend.Variable
	SigningPK   [34]frontend.Variable
	NullifierPK frontend.Variable
	ViewingPK   [33]frontend.Variable
}

func (d *IntentData) Hash(api frontend.API) frontend.Variable {
	OwnerIdentity(api, d.SigningPK)
	return gnarksdk.Poseidon(api, new(big.Int).SetBytes([]byte("stake_pool_intent")),
		gnarksdk.HashBytes(api, d.Epoch[:]), gnarksdk.HashBytes(api, d.SigningPK[:]),
		d.NullifierPK, gnarksdk.HashBytes(api, d.ViewingPK[:]))
}

func (d *IntentData) Bytes(api frontend.API) []frontend.Variable {
	result := append([]frontend.Variable{}, d.Epoch[:]...)
	result = append(result, d.SigningPK[:]...)
	result = append(result, ve.FieldToBytesBE(api, d.NullifierPK, 32)...)
	return append(result, d.ViewingPK[:]...)
}

type Note struct {
	TreeID      frontend.Variable
	Asset       [32]frontend.Variable
	AssetID     frontend.Variable
	Amount      frontend.Variable
	Blinding    frontend.Variable
	SigningPK   [34]frontend.Variable
	NullifierPK frontend.Variable
}

func (n *Note) Utxo(api frontend.API, dataHash frontend.Variable) gnarksdk.Utxo {
	api.ToBinary(n.TreeID, 16)
	api.ToBinary(n.Amount, 64)
	return gnarksdk.Utxo{
		Domain: spp.UtxoDomain, Owner: gnarksdk.Poseidon(api, OwnerIdentity(api, n.SigningPK), n.NullifierPK),
		Asset: gnarksdk.HashBytes(api, n.Asset[:]), Amount: n.Amount, Blinding: n.Blinding,
		DataHash: dataHash, RingDataHash: 0, RingProgramID: 0, TreeID: n.TreeID,
	}
}

func littleEndian(api frontend.API, value frontend.Variable, len int) []frontend.Variable {
	bytes := ve.FieldToBytesBE(api, value, len)
	for left, right := 0, len-1; left < right; left, right = left+1, right-1 {
		bytes[left], bytes[right] = bytes[right], bytes[left]
	}
	return bytes
}

// Bytes mirrors SDK Note's wincode layout; ownership and the full intent data
// remain recoverable by either recipient, rather than publishing a partial note.
func (n *Note) Bytes(api frontend.API, intent *IntentData) []frontend.Variable {
	result := []frontend.Variable{1}
	result = append(result, littleEndian(api, n.TreeID, 2)...)
	result = append(result, n.Asset[:]...)
	result = append(result, littleEndian(api, n.AssetID, 8)...)
	result = append(result, littleEndian(api, n.Amount, 8)...)
	result = append(result, ve.FieldToBytesBE(api, n.Blinding, 32)...)
	result = append(result, n.SigningPK[:]...)
	result = append(result, ve.FieldToBytesBE(api, n.NullifierPK, 32)...)
	if intent == nil {
		return append(result, 0)
	}
	result = append(result, 1)
	return append(result, intent.Bytes(api)...)
}

// IntentDataHash equals IntentData.Hash with the epoch and viewing key hashes
// supplied by the caller, so a batch hashes them once.
func IntentDataHash(api frontend.API, epochHash frontend.Variable, signingPK [34]frontend.Variable, nullifierPK, viewingKeyHash frontend.Variable) frontend.Variable {
	OwnerIdentity(api, signingPK)
	return gnarksdk.Poseidon(api, new(big.Int).SetBytes([]byte("stake_pool_intent")),
		epochHash, gnarksdk.HashBytes(api, signingPK[:]), nullifierPK, viewingKeyHash)
}

func TreeIDBytesLE(api frontend.API, treeID frontend.Variable) []frontend.Variable {
	return littleEndian(api, treeID, 2)
}

// PayoutBytes mirrors SDK PayoutNote's 44-byte wincode layout. The caller
// constrains asset to a bit; the byte decompositions bound amount to 64 bits
// and blinding to 32 bytes.
func PayoutBytes(api frontend.API, treeLE []frontend.Variable, asset, amount, blinding frontend.Variable) []frontend.Variable {
	if len(treeLE) != 2 {
		panic("tree id must be 2 bytes")
	}
	result := []frontend.Variable{1}
	result = append(result, treeLE...)
	result = append(result, asset)
	result = append(result, littleEndian(api, amount, 8)...)
	return append(result, ve.FieldToBytesBE(api, blinding, 32)...)
}
