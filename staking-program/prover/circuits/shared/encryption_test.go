package shared

import (
	"encoding/json"
	"math/big"
	"os"
	"path/filepath"
	"testing"

	"github.com/consensys/gnark-crypto/ecc"
	"github.com/consensys/gnark/frontend"
	"github.com/consensys/gnark/test"
	"github.com/iden3/go-iden3-crypto/poseidon"
	"zolana/prover/circuits/verifiable-encryption/aes"
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
	Ciphertext      []byte `json:"ciphertext"`
	Commitment      []byte `json:"commitment"`
}

func loadEncryptionFixture(t *testing.T) encryptionCircuit {
	t.Helper()
	data, err := os.ReadFile("../testdata/refund-encryption.json")
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

type batchEncryptionCircuit struct {
	Encryption   BatchEncryption
	UserPoint    [65]frontend.Variable
	OutputIndex  frontend.Variable
	UtxoHash     frontend.Variable
	Plaintext    []frontend.Variable
	Ciphertext   []frontend.Variable
	ContextHash  frontend.Variable
	CrankKeyHash frontend.Variable
}

func (c *batchEncryptionCircuit) Define(api frontend.API) error {
	b := c.Encryption.Prepare(api, aes.NewAESGadget(api))
	ciphertext := b.Encrypt(b.Recipient(c.UserPoint), c.Plaintext, c.OutputIndex, c.UtxoHash)
	if len(ciphertext) != len(c.Ciphertext) {
		panic("ciphertext length")
	}
	for i, v := range ciphertext {
		api.AssertIsEqual(v, c.Ciphertext[i])
	}
	api.AssertIsEqual(b.ContextHash(ciphertext), c.ContextHash)
	api.AssertIsEqual(b.CrankKeyHash(), c.CrankKeyHash)
	return nil
}

// nativeHashBytes is zolana_hasher's hash_bytes: 31-byte big-endian chunks
// folded left to right with Poseidon.
func nativeHashBytes(t *testing.T, data []byte) *big.Int {
	t.Helper()
	var h *big.Int
	for offset := 0; offset < len(data); offset += 31 {
		end := min(offset+31, len(data))
		chunk := new(big.Int).SetBytes(data[offset:end])
		if h == nil {
			h = chunk
			continue
		}
		next, err := poseidon.Hash([]*big.Int{h, chunk})
		if err != nil {
			t.Fatal(err)
		}
		h = next
	}
	return h
}

func readFixture(t *testing.T, path string) encryptionFixture {
	t.Helper()
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var f encryptionFixture
	if err = json.Unmarshal(data, &f); err != nil {
		t.Fatal(err)
	}
	if len(f.Ciphertext) != 1+2*len(f.Plaintext) {
		t.Fatal("wrong ciphertext size")
	}
	return f
}

func batchWitness(t *testing.T, f encryptionFixture) (batchEncryptionCircuit, batchEncryptionCircuit) {
	t.Helper()
	size := len(f.Plaintext)
	circuit := batchEncryptionCircuit{Plaintext: make([]frontend.Variable, size), Ciphertext: make([]frontend.Variable, 1+2*size)}
	witness := batchEncryptionCircuit{Plaintext: make([]frontend.Variable, size), Ciphertext: make([]frontend.Variable, 1+2*size)}
	witness.OutputIndex = f.OutputIndex
	witness.UtxoHash = new(big.Int).SetBytes(f.UtxoHash)
	for i, b := range f.EphemeralSecret {
		witness.Encryption.EphemeralSecret[i] = b
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
	for i, b := range f.UserPoint {
		witness.UserPoint[i] = b
	}
	for i, b := range f.Plaintext {
		witness.Plaintext[i] = b
	}
	for i, b := range f.Ciphertext {
		witness.Ciphertext[i] = b
	}
	context, err := poseidon.Hash([]*big.Int{nativeHashBytes(t, f.Ciphertext),
		nativeHashBytes(t, f.TxViewingPK), new(big.Int).SetBytes(f.Salt)})
	if err != nil {
		t.Fatal(err)
	}
	witness.ContextHash = context
	compressed := append([]byte{2 + f.CrankPoint[64]&1}, f.CrankPoint[1:33]...)
	witness.CrankKeyHash = nativeHashBytes(t, compressed)
	return circuit, witness
}

// Every Rust encryption fixture present (refund 150 bytes, payout 44 bytes)
// must be reproduced byte for byte by the batch encryptor.
func TestBatchEncryptionMatchesRustFixtures(t *testing.T) {
	paths, err := filepath.Glob("../testdata/*-encryption.json")
	if err != nil || len(paths) == 0 {
		t.Fatal("no encryption fixtures")
	}
	for _, path := range paths {
		t.Run(filepath.Base(path), func(t *testing.T) {
			f := readFixture(t, path)
			circuit, witness := batchWitness(t, f)
			if err := test.IsSolved(&circuit, &witness, ecc.BN254.ScalarField()); err != nil {
				t.Fatal(err)
			}
			for _, tc := range []struct {
				name   string
				change func(*batchEncryptionCircuit)
			}{
				{"wrong_recipient", func(c *batchEncryptionCircuit) { c.UserPoint = c.Encryption.CrankPoint }},
				{"wrong_ephemeral", func(c *batchEncryptionCircuit) { c.Encryption.TxViewingPK[1] = 0 }},
				{"different_index", func(c *batchEncryptionCircuit) { c.OutputIndex = f.OutputIndex + 1 }},
				{"different_context", func(c *batchEncryptionCircuit) { c.ContextHash = 1 }},
				{"different_crank", func(c *batchEncryptionCircuit) { c.CrankKeyHash = 1 }},
			} {
				_, bad := batchWitness(t, f)
				tc.change(&bad)
				if err := test.IsSolved(&circuit, &bad, ecc.BN254.ScalarField()); err == nil {
					t.Fatalf("%s accepted", tc.name)
				}
			}
		})
	}
}
