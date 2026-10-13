package shared

import (
	"github.com/consensys/gnark/frontend"
	"github.com/consensys/gnark/std/lookup/logderivlookup"
	"zolana/prover/circuits/verifiable-encryption/aes"
)

// AES is a byte-level AES-256 gadget. The zolana gadget decomposes every
// T-table word into bits; here S-box multiples and byte XOR are lookups, so a
// round column costs 28 queries instead of ~300 bit constraints. Values are
// identical to aes.AESGadget.
type AES struct {
	api           frontend.API
	sbox, s2, s3  logderivlookup.Table
	xor           logderivlookup.Table
	rcon          [8]int
	columnSources [4][4]logderivlookup.Table
}

func NewAES(api frontend.API) *AES {
	a := &AES{
		api: api, sbox: logderivlookup.New(api), s2: logderivlookup.New(api),
		s3: logderivlookup.New(api), xor: logderivlookup.New(api),
		rcon: [8]int{0x8d, 0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40},
	}
	// T[0][x] packs (2s, s, s, 3s) from its low byte up.
	for x := 0; x < 256; x++ {
		word, ok := aes.T[0][x].(int)
		if !ok {
			panic("aes: unexpected T-table entry")
		}
		a.s2.Insert(word & 0xff)
		a.sbox.Insert((word >> 8) & 0xff)
		a.s3.Insert((word >> 24) & 0xff)
	}
	for x := 0; x < 256; x++ {
		for y := 0; y < 256; y++ {
			a.xor.Insert(x ^ y)
		}
	}
	// Output byte j of T[k] is byte (j-k) mod 4 of T[0].
	byByte := [4]logderivlookup.Table{a.s2, a.sbox, a.sbox, a.s3}
	for j := 0; j < 4; j++ {
		for k := 0; k < 4; k++ {
			a.columnSources[j][k] = byByte[(j-k+4)%4]
		}
	}
	return a
}

// Xor returns x ^ y for byte-valued x and y.
func (a *AES) Xor(x, y frontend.Variable) frontend.Variable {
	return a.xor.Lookup(a.api.Add(a.api.Mul(x, 256), y))[0]
}

// ExpandKey is the AES-256 key schedule: 240 round-key bytes.
func (a *AES) ExpandKey(key [32]frontend.Variable) []frontend.Variable {
	expand := make([]frontend.Variable, 240)
	copy(expand, key[:])
	for i := 32; i < 240; i += 4 {
		t := []frontend.Variable{expand[i-4], expand[i-3], expand[i-2], expand[i-1]}
		if i%32 == 0 {
			t = a.sbox.Lookup(t[1], t[2], t[3], t[0])
			t[0] = a.Xor(t[0], a.rcon[i/32])
		} else if i%32 == 16 {
			t = a.sbox.Lookup(t...)
		}
		for j := 0; j < 4; j++ {
			expand[i+j] = a.Xor(expand[i-32+j], t[j])
		}
	}
	return expand
}

var shiftRows = [16]int{0, 5, 10, 15, 4, 9, 14, 3, 8, 13, 2, 7, 12, 1, 6, 11}

// encryptBlockExpanded encrypts one block under an expanded AES-256 key.
func (a *AES) encryptBlockExpanded(xk []frontend.Variable, block [16]frontend.Variable) [16]frontend.Variable {
	const rounds = 14
	var state [16]frontend.Variable
	for i := range state {
		state[i] = a.Xor(xk[i], block[i])
	}
	for round := 1; round < rounds; round++ {
		var next [16]frontend.Variable
		for c := 0; c < 4; c++ {
			for j := 0; j < 4; j++ {
				acc := xk[round*16+4*c+j]
				for k := 0; k < 4; k++ {
					acc = a.Xor(acc, a.columnSources[j][k].Lookup(state[shiftRows[4*c+k]])[0])
				}
				next[4*c+j] = acc
			}
		}
		state = next
	}
	var shifted [16]frontend.Variable
	for i := range shifted {
		shifted[i] = state[shiftRows[i]]
	}
	sub := a.sbox.Lookup(shifted[:]...)
	for i := range state {
		state[i] = a.Xor(sub[i], xk[rounds*16+i])
	}
	return state
}

// ctrEncryptExpanded produces the same bytes as aes.CTREncrypt with one key
// expansion. Plaintext bytes must already be range-checked.
func (a *AES) ctrEncryptExpanded(key [32]frontend.Variable, nonce [12]frontend.Variable, plaintext []frontend.Variable) []frontend.Variable {
	xk := a.ExpandKey(key)
	ciphertext := make([]frontend.Variable, len(plaintext))
	counter := aes.MakeJ0(a.api, nonce)
	for offset := 0; offset < len(plaintext); offset += 16 {
		counter = aes.Increment32(a.api, counter)
		keystream := a.encryptBlockExpanded(xk, counter)
		for j := 0; j < 16 && offset+j < len(plaintext); j++ {
			ciphertext[offset+j] = a.Xor(plaintext[offset+j], keystream[j])
		}
	}
	return ciphertext
}
