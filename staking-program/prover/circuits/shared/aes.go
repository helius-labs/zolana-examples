package shared

import (
	"github.com/consensys/gnark/frontend"
	"zolana/prover/circuits/verifiable-encryption/aes"
)

// encryptBlockExpanded is the AES-256 round body of AESGadget.Encrypt over an
// already expanded key, so one expansion serves every counter block.
func encryptBlockExpanded(g *aes.AESGadget, xk []frontend.Variable, block [16]frontend.Variable) [16]frontend.Variable {
	const rounds = 14
	var state [16]frontend.Variable
	for i := range state {
		state[i] = g.VariableXor(xk[i], block[i], 8)
	}
	for round := 1; round < rounds; round++ {
		k := round * 16
		t0 := g.XorSubWords(state[0], state[5], state[10], state[15], xk[k:k+4])
		t1 := g.XorSubWords(state[4], state[9], state[14], state[3], xk[k+4:k+8])
		t2 := g.XorSubWords(state[8], state[13], state[2], state[7], xk[k+8:k+12])
		t3 := g.XorSubWords(state[12], state[1], state[6], state[11], xk[k+12:k+16])
		copy(state[:4], t0)
		copy(state[4:8], t1)
		copy(state[8:12], t2)
		copy(state[12:16], t3)
	}
	copy(state[:], g.ShiftSub(state))
	k := rounds * 16
	for i := range state {
		state[i] = g.VariableXor(state[i], xk[k+i], 8)
	}
	return state
}

// ctrEncryptExpanded produces the same bytes as aes.CTREncrypt but expands the
// key once instead of once per block.
func ctrEncryptExpanded(api frontend.API, g *aes.AESGadget, key [32]frontend.Variable, nonce [12]frontend.Variable, plaintext []frontend.Variable) []frontend.Variable {
	xk := g.ExpandKey(key[:])
	ciphertext := make([]frontend.Variable, len(plaintext))
	counter := aes.MakeJ0(api, nonce)
	for offset := 0; offset < len(plaintext); offset += 16 {
		counter = aes.Increment32(api, counter)
		keystream := encryptBlockExpanded(g, xk, counter)
		for j := 0; j < 16 && offset+j < len(plaintext); j++ {
			ciphertext[offset+j] = aes.XorByte(api, plaintext[offset+j], keystream[j])
		}
	}
	return ciphertext
}
