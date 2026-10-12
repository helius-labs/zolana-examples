package shared

import (
	stdaes "crypto/aes"
	"crypto/cipher"
	"testing"

	"github.com/consensys/gnark-crypto/ecc"
	"github.com/consensys/gnark/frontend"
	"github.com/consensys/gnark/test"
	"zolana/prover/circuits/verifiable-encryption/aes"
)

type ctrCircuit struct {
	Key        [32]frontend.Variable
	Nonce      [12]frontend.Variable
	Plaintext  []frontend.Variable
	Ciphertext []frontend.Variable
}

func (c *ctrCircuit) Define(api frontend.API) error {
	g := aes.NewAESGadget(api)
	expanded := ctrEncryptExpanded(api, g, c.Key, c.Nonce, c.Plaintext)
	reference := aes.CTREncrypt(api, g, c.Key, c.Nonce, c.Plaintext)
	for i := range c.Ciphertext {
		api.AssertIsEqual(expanded[i], reference[i])
		api.AssertIsEqual(expanded[i], c.Ciphertext[i])
	}
	return nil
}

// nativeCTR is AES-256-CTR starting at J0 + 1 = nonce || 0x00000002.
func nativeCTR(t *testing.T, key []byte, nonce []byte, plaintext []byte) []byte {
	t.Helper()
	block, err := stdaes.NewCipher(key)
	if err != nil {
		t.Fatal(err)
	}
	iv := append(append([]byte{}, nonce...), 0, 0, 0, 2)
	out := make([]byte, len(plaintext))
	cipher.NewCTR(block, iv).XORKeyStream(out, plaintext)
	return out
}

func TestCTREncryptExpandedMatchesCTREncrypt(t *testing.T) {
	for _, size := range []int{44, 150} {
		key := make([]byte, 32)
		nonce := make([]byte, 12)
		plaintext := make([]byte, size)
		for i := range key {
			key[i] = byte(7*i + 3)
		}
		for i := range nonce {
			nonce[i] = byte(11*i + 5)
		}
		for i := range plaintext {
			plaintext[i] = byte(13*i + 1)
		}
		ciphertext := nativeCTR(t, key, nonce, plaintext)
		witness := ctrCircuit{Plaintext: make([]frontend.Variable, size), Ciphertext: make([]frontend.Variable, size)}
		for i, b := range key {
			witness.Key[i] = b
		}
		for i, b := range nonce {
			witness.Nonce[i] = b
		}
		for i := range plaintext {
			witness.Plaintext[i] = plaintext[i]
			witness.Ciphertext[i] = ciphertext[i]
		}
		circuit := ctrCircuit{Plaintext: make([]frontend.Variable, size), Ciphertext: make([]frontend.Variable, size)}
		if err := test.IsSolved(&circuit, &witness, ecc.BN254.ScalarField()); err != nil {
			t.Fatalf("size %d: %v", size, err)
		}
		witness.Ciphertext[size-1] = ciphertext[size-1] ^ 1
		if err := test.IsSolved(&circuit, &witness, ecc.BN254.ScalarField()); err == nil {
			t.Fatalf("size %d: wrong ciphertext accepted", size)
		}
	}
}
