package shared

import (
	"encoding/binary"
	"math/big"
	"testing"

	"github.com/consensys/gnark-crypto/ecc"
	"github.com/consensys/gnark/frontend"
	"github.com/consensys/gnark/test"
	"zolana/gnarksdk"
)

type payoutBytesCircuit struct {
	TreeID   frontend.Variable
	Asset    frontend.Variable
	Amount   frontend.Variable
	Blinding frontend.Variable
	Expected [44]frontend.Variable
	Intent   IntentData
}

func (c *payoutBytesCircuit) Define(api frontend.API) error {
	bytes := PayoutBytes(api, TreeIDBytesLE(api, c.TreeID), c.Asset, c.Amount, c.Blinding)
	if len(bytes) != len(c.Expected) {
		panic("payout length")
	}
	for i, b := range bytes {
		api.AssertIsEqual(b, c.Expected[i])
	}
	hash := IntentDataHash(api, gnarksdk.HashBytes(api, c.Intent.Epoch[:]), c.Intent.SigningPK,
		c.Intent.NullifierPK, gnarksdk.HashBytes(api, c.Intent.ViewingPK[:]))
	api.AssertIsEqual(hash, c.Intent.Hash(api))
	return nil
}

func payoutBytesWitness(amount *big.Int) payoutBytesCircuit {
	blinding := new(big.Int).SetBytes([]byte{0x12, 0x34, 0x56})
	w := payoutBytesCircuit{TreeID: 0x0102, Asset: 1, Amount: amount, Blinding: blinding}
	expected := []byte{1, 0x02, 0x01, 1}
	expected = binary.LittleEndian.AppendUint64(expected, amount.Uint64())
	expected = append(expected, blinding.FillBytes(make([]byte, 32))...)
	for i, b := range expected {
		w.Expected[i] = b
	}
	for i := range w.Intent.Epoch {
		w.Intent.Epoch[i] = i
	}
	w.Intent.SigningPK[0] = 1
	for i := 1; i < 33; i++ {
		w.Intent.SigningPK[i] = i
	}
	w.Intent.SigningPK[33] = 0
	w.Intent.NullifierPK = 77
	w.Intent.ViewingPK[0] = 2
	for i := 1; i < 33; i++ {
		w.Intent.ViewingPK[i] = 100 + i
	}
	return w
}

func TestPayoutBytesAndIntentDataHash(t *testing.T) {
	witness := payoutBytesWitness(new(big.Int).SetUint64(0xfedcba9876543210))
	if err := test.IsSolved(&payoutBytesCircuit{}, &witness, ecc.BN254.ScalarField()); err != nil {
		t.Fatal(err)
	}
	witness.Amount = new(big.Int).Lsh(big.NewInt(1), 64)
	if err := test.IsSolved(&payoutBytesCircuit{}, &witness, ecc.BN254.ScalarField()); err == nil {
		t.Fatal("amount above 64 bits accepted")
	}
}
