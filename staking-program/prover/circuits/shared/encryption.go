package shared

import (
	"math/big"

	"github.com/consensys/gnark/frontend"
	"github.com/consensys/gnark/std/math/cmp"
	"zolana/gnarksdk"
	ve "zolana/prover/circuits/verifiable-encryption"
	"zolana/prover/circuits/verifiable-encryption/aes"
	"zolana/prover/circuits/verifiable-encryption/p256"
)

type Encryption struct {
	EphemeralSecret [32]frontend.Variable
	UserPoint       [65]frontend.Variable
	CrankPoint      [65]frontend.Variable
	TxViewingPK     [33]frontend.Variable
	Salt            [16]frontend.Variable
	OutputIndex     frontend.Variable
}

// Canonical bytes prevent witnesses for x+p, y+p, or sk+n from naming a
// different key than the host encryption implementation.
func lessThanBytes(api frontend.API, value []frontend.Variable, bound []byte) {
	comparator := cmp.NewBoundedComparator(api, big.NewInt(256), false)
	var equal, less frontend.Variable = 1, 0
	for i, b := range value {
		api.ToBinary(b, 8)
		less = api.Add(less, api.Mul(equal, comparator.IsLess(b, bound[i])))
		equal = api.Mul(equal, api.IsZero(api.Sub(b, bound[i])))
	}
	api.AssertIsEqual(less, 1)
}

func validatePoint(api frontend.API, point [65]frontend.Variable) {
	modulus, ok := new(big.Int).SetString("ffffffff00000001000000000000000000000000ffffffffffffffffffffffff", 16)
	if !ok {
		panic("invalid P-256 modulus")
	}
	api.AssertIsEqual(point[0], 4)
	lessThanBytes(api, point[1:33], modulus.FillBytes(make([]byte, 32)))
	lessThanBytes(api, point[33:65], modulus.FillBytes(make([]byte, 32)))
	var sum frontend.Variable = 0
	for _, b := range point[1:] {
		sum = api.Add(sum, b)
	}
	api.AssertIsDifferent(sum, 0)
	p256.PointOnCurve(api, point)
}

func Pack(api frontend.API, bytes []frontend.Variable) frontend.Variable {
	var value frontend.Variable = 0
	for _, b := range bytes {
		api.ToBinary(b, 8)
		value = api.Add(api.Mul(value, 256), b)
	}
	return value
}

// Encrypt returns the exact bytes published as TransactOutput.data. The caller
// binds their hash, recipient keys, and context to the program's public inputs.
func (e *Encryption) Encrypt(api frontend.API, g *aes.AESGadget, plaintext []frontend.Variable, utxoHash frontend.Variable) []frontend.Variable {
	lessThanBytes(api, e.EphemeralSecret[:], p256.GroupOrder().FillBytes(make([]byte, 32)))
	var scalarSum frontend.Variable = 0
	for _, b := range e.EphemeralSecret {
		scalarSum = api.Add(scalarSum, b)
	}
	api.AssertIsDifferent(scalarSum, 0)
	ephemeral := p256.CompressPubkey(api, p256.ScalarMulGenerator(api, e.EphemeralSecret))
	for i, b := range e.TxViewingPK {
		api.AssertIsEqual(b, ephemeral[i])
	}
	api.ToBinary(e.OutputIndex, 32)
	salt := Pack(api, e.Salt[:])
	for _, b := range plaintext {
		api.ToBinary(b, 8)
	}
	result := []frontend.Variable{1}
	info := make([]frontend.Variable, 10)
	for i, b := range []byte("STAKING001") {
		info[i] = b
	}
	domain := new(big.Int).SetBytes([]byte("stake_pool_encryption"))
	for role, recipient := range [][65]frontend.Variable{e.UserPoint, e.CrankPoint} {
		validatePoint(api, recipient)
		compressed := p256.CompressPubkey(api, recipient)
		dh := p256.ECDH(api, e.EphemeralSecret, recipient)
		shared := gnarksdk.Poseidon(api, domain, gnarksdk.HashBytes(api, dh[:]),
			gnarksdk.HashBytes(api, e.TxViewingPK[:]), gnarksdk.HashBytes(api, compressed[:]),
			salt, e.OutputIndex, role, utxoHash)
		key, nonce := ve.KeySchedule(api, shared, info, 10)
		result = append(result, aes.CTREncrypt(api, g, key, nonce, plaintext)...)
	}
	return result
}

// BatchEncryption is one transaction's encryption key material shared by every
// output it encrypts; the user point varies per output.
type BatchEncryption struct {
	EphemeralSecret [32]frontend.Variable
	CrankPoint      [65]frontend.Variable
	TxViewingPK     [33]frontend.Variable
	Salt            [16]frontend.Variable
}

// Recipient holds the per-recipient hashes the KDF consumes.
type Recipient struct {
	Compressed [33]frontend.Variable
	KeyHash    frontend.Variable
	DHHash     frontend.Variable
}

// BatchEncryptor computes the ephemeral base multiplication, crank ECDH, and
// fixed hashes once, then encrypts any number of outputs.
type BatchEncryptor struct {
	api           frontend.API
	g             *aes.AESGadget
	secret        [32]frontend.Variable
	txViewingHash frontend.Variable
	salt          frontend.Variable
	crank         Recipient
	info          []frontend.Variable
}

func (e *BatchEncryption) Prepare(api frontend.API, g *aes.AESGadget) *BatchEncryptor {
	lessThanBytes(api, e.EphemeralSecret[:], p256.GroupOrder().FillBytes(make([]byte, 32)))
	var scalarSum frontend.Variable = 0
	for _, b := range e.EphemeralSecret {
		scalarSum = api.Add(scalarSum, b)
	}
	api.AssertIsDifferent(scalarSum, 0)
	ephemeral := p256.CompressPubkey(api, p256.ScalarMulGenerator(api, e.EphemeralSecret))
	for i, b := range e.TxViewingPK {
		api.AssertIsEqual(b, ephemeral[i])
	}
	info := make([]frontend.Variable, 10)
	for i, b := range []byte("STAKING001") {
		info[i] = b
	}
	b := &BatchEncryptor{
		api: api, g: g, secret: e.EphemeralSecret, info: info,
		txViewingHash: gnarksdk.HashBytes(api, e.TxViewingPK[:]), salt: Pack(api, e.Salt[:]),
	}
	b.crank = b.Recipient(e.CrankPoint)
	return b
}

// Recipient validates an uncompressed P-256 point and derives its hashes.
func (b *BatchEncryptor) Recipient(point [65]frontend.Variable) Recipient {
	validatePoint(b.api, point)
	compressed := p256.CompressPubkey(b.api, point)
	dh := p256.ECDH(b.api, b.secret, point)
	return Recipient{
		Compressed: compressed,
		KeyHash:    gnarksdk.HashBytes(b.api, compressed[:]),
		DHHash:     gnarksdk.HashBytes(b.api, dh[:]),
	}
}

// Encrypt returns 1 || ct_user || ct_crank, byte-identical to Encryption.Encrypt
// for the same keys and context.
func (b *BatchEncryptor) Encrypt(user Recipient, plaintext []frontend.Variable, outputIndex, utxoHash frontend.Variable) []frontend.Variable {
	api := b.api
	api.ToBinary(outputIndex, 32)
	for _, v := range plaintext {
		api.ToBinary(v, 8)
	}
	result := []frontend.Variable{1}
	domain := new(big.Int).SetBytes([]byte("stake_pool_encryption"))
	for role, recipient := range []Recipient{user, b.crank} {
		shared := gnarksdk.Poseidon(api, domain, recipient.DHHash, b.txViewingHash,
			recipient.KeyHash, b.salt, outputIndex, role, utxoHash)
		key, nonce := ve.KeySchedule(api, shared, b.info, 10)
		result = append(result, ctrEncryptExpanded(api, b.g, key, nonce, plaintext)...)
	}
	return result
}

// ContextHash mirrors the program's ciphertext_context_hash.
func (b *BatchEncryptor) ContextHash(ciphertext []frontend.Variable) frontend.Variable {
	return gnarksdk.Poseidon(b.api, gnarksdk.HashBytes(b.api, ciphertext), b.txViewingHash, b.salt)
}

func (b *BatchEncryptor) CrankKeyHash() frontend.Variable {
	return b.crank.KeyHash
}

func (e *Encryption) Commitment(api frontend.API, plaintext []frontend.Variable, utxoHash frontend.Variable) frontend.Variable {
	g := aes.NewAESGadget(api)
	ciphertext := e.Encrypt(api, g, plaintext, utxoHash)
	user := p256.CompressPubkey(api, e.UserPoint)
	crank := p256.CompressPubkey(api, e.CrankPoint)
	return gnarksdk.Poseidon(api, utxoHash, gnarksdk.HashBytes(api, user[:]),
		gnarksdk.HashBytes(api, crank[:]), gnarksdk.HashBytes(api, e.TxViewingPK[:]),
		Pack(api, e.Salt[:]), e.OutputIndex, gnarksdk.HashBytes(api, ciphertext))
}
