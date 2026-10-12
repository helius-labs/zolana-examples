package shared

import (
	"encoding/json"
	"math/big"
	"os"
	"testing"

	"github.com/consensys/gnark-crypto/ecc"
	"github.com/consensys/gnark/frontend"
	"github.com/consensys/gnark/test"
)

type encryptionCircuit struct {
	PublicInputHash frontend.Variable `gnark:",public"`
	Encryption      Encryption
	Plaintext       [150]frontend.Variable
	UtxoHash        frontend.Variable
}

func (c *encryptionCircuit) Define(api frontend.API) error {
	api.AssertIsEqual(c.PublicInputHash, c.Encryption.Commitment(api, c.Plaintext[:], c.UtxoHash))
	return nil
}

type encryptionFixture struct {
	EphemeralSecret []byte `json:"ephemeral_secret"`
	UserPoint       []byte `json:"user_point"`
	CrankPoint      []byte `json:"crank_point"`
	TxViewingPK     []byte `json:"tx_viewing_pk"`
	Salt            []byte `json:"salt"`
	OutputIndex     uint32 `json:"output_index"`
	UtxoHash        []byte `json:"utxo_hash"`
	Plaintext       []byte `json:"plaintext"`
	Commitment      []byte `json:"commitment"`
}

func loadEncryptionFixture(t *testing.T) encryptionCircuit {
	t.Helper()
	data, err := os.ReadFile("../testdata/payout-encryption.json")
	if err != nil {
		t.Fatal(err)
	}
	var f encryptionFixture
	if err = json.Unmarshal(data, &f); err != nil {
		t.Fatal(err)
	}
	if len(f.Plaintext) != 150 {
		t.Fatal("wrong plaintext size")
	}
	var witness encryptionCircuit
	witness.PublicInputHash = new(big.Int).SetBytes(f.Commitment)
	witness.UtxoHash = new(big.Int).SetBytes(f.UtxoHash)
	witness.Encryption.OutputIndex = f.OutputIndex
	for i, b := range f.EphemeralSecret {
		witness.Encryption.EphemeralSecret[i] = b
	}
	for i, b := range f.UserPoint {
		witness.Encryption.UserPoint[i] = b
	}
	for i, b := range f.CrankPoint {
		witness.Encryption.CrankPoint[i] = b
	}
	for i, b := range f.TxViewingPK {
		witness.Encryption.TxViewingPK[i] = b
	}
	for i, b := range f.Salt {
		witness.Encryption.Salt[i] = b
	}
	for i, b := range f.Plaintext {
		witness.Plaintext[i] = b
	}
	return witness
}

func TestEncryptionMatchesRustFixture(t *testing.T) {
	witness := loadEncryptionFixture(t)
	if err := test.IsSolved(&encryptionCircuit{}, &witness, ecc.BN254.ScalarField()); err != nil {
		t.Fatal(err)
	}
}

func TestEncryptionRejectsInvalidKeysAndMismatchedRecipients(t *testing.T) {
	for _, tc := range []struct {
		name   string
		change func(*encryptionCircuit)
	}{
		{"wrong_recipient", func(c *encryptionCircuit) { c.Encryption.UserPoint = c.Encryption.CrankPoint }},
		{"wrong_ephemeral", func(c *encryptionCircuit) { c.Encryption.TxViewingPK[1] = 0 }},
		{"invalid_point", func(c *encryptionCircuit) { c.Encryption.UserPoint[1] = 0 }},
		{"invalid_prefix", func(c *encryptionCircuit) { c.Encryption.UserPoint[0] = 3 }},
		{"non_byte_coordinate", func(c *encryptionCircuit) { c.Encryption.UserPoint[1] = 256 }},
		{"zero_scalar", func(c *encryptionCircuit) {
			for i := range c.Encryption.EphemeralSecret {
				c.Encryption.EphemeralSecret[i] = 0
			}
		}},
		{"scalar_at_order", func(c *encryptionCircuit) {
			n, _ := new(big.Int).SetString("ffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc632551", 16)
			for i, b := range n.FillBytes(make([]byte, 32)) {
				c.Encryption.EphemeralSecret[i] = b
			}
		}},
		{"different_plaintext", func(c *encryptionCircuit) { c.Plaintext[0] = 2 }},
		{"different_context", func(c *encryptionCircuit) { c.Encryption.OutputIndex = 4 }},
	} {
		t.Run(tc.name, func(t *testing.T) {
			witness := loadEncryptionFixture(t)
			tc.change(&witness)
			if err := test.IsSolved(&encryptionCircuit{}, &witness, ecc.BN254.ScalarField()); err == nil {
				t.Fatal("invalid encryption witness accepted")
			}
		})
	}
}
