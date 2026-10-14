package dvpterms

import (
	"encoding/json"
	"math/big"
	"os"
	"testing"

	"github.com/consensys/gnark-crypto/ecc"
	"github.com/consensys/gnark/frontend"
	"github.com/consensys/gnark/test"
)

// vector is testdata/swap_dvp_vector.json, written and checked by the dvp-sdk
// unit test dvp_id_vector: the witness of fixed terms and the dvp_id and
// create blinding seed Rust computes for them.
type vector struct {
	DvpID              string              `json:"dvp_id"`
	CreateBlindingSeed string              `json:"create_blinding_seed"`
	Witness            map[string][]string `json:"witness"`
}

func loadVector(t *testing.T) vector {
	t.Helper()
	raw, err := os.ReadFile("testdata/swap_dvp_vector.json")
	if err != nil {
		t.Fatal(err)
	}
	var v vector
	if err := json.Unmarshal(raw, &v); err != nil {
		t.Fatal(err)
	}
	return v
}

func (v vector) terms(t *testing.T) SwapDvp {
	t.Helper()
	get := func(key string) frontend.Variable {
		values, ok := v.Witness["Terms_"+key]
		if !ok || len(values) != 1 {
			t.Fatalf("vector has no single value for %s", key)
		}
		return values[0]
	}
	destination := func(name string) Destination {
		return Destination{OwnerHash: get(name + "_OwnerHash"), ViewingPkHash: get(name + "_ViewingPkHash")}
	}
	return SwapDvp{
		SellerID:                    get("SellerID"),
		BuyerID:                     get("BuyerID"),
		SettlementAuthorityID:       get("SettlementAuthorityID"),
		CreatorID:                   get("CreatorID"),
		DeliveryAsset:               get("DeliveryAsset"),
		PaymentAsset:                get("PaymentAsset"),
		DeliveryAmount:              get("DeliveryAmount"),
		PaymentAmount:               get("PaymentAmount"),
		ExpiryTimestamp:             get("ExpiryTimestamp"),
		HasEarliestSettlement:       get("HasEarliestSettlement"),
		EarliestSettlementTimestamp: get("EarliestSettlementTimestamp"),
		Nonce:                       get("Nonce"),
		RefStringHash:               get("RefStringHash"),
		SellerSettlementDestination: destination("SellerSettlementDestination"),
		BuyerSettlementDestination:  destination("BuyerSettlementDestination"),
		SellerRefundDestination:     destination("SellerRefundDestination"),
		BuyerRefundDestination:      destination("BuyerRefundDestination"),
		Blinding:                    get("Blinding"),
	}
}

type dvpIDCircuit struct {
	Terms              SwapDvp
	DvpID              frontend.Variable `gnark:",public"`
	CreateBlindingSeed frontend.Variable `gnark:",public"`
}

func (c *dvpIDCircuit) Define(api frontend.API) error {
	api.AssertIsEqual(c.Terms.DvpID(api), c.DvpID)
	api.AssertIsEqual(c.Terms.BlindingSeed(api, CreateTag), c.CreateBlindingSeed)
	return nil
}

// TestDvpIDMatchesRust is the cross-language check of the D4 layout.
func TestDvpIDMatchesRust(t *testing.T) {
	v := loadVector(t)
	assignment := &dvpIDCircuit{Terms: v.terms(t), DvpID: v.DvpID, CreateBlindingSeed: v.CreateBlindingSeed}
	if err := test.IsSolved(&dvpIDCircuit{}, assignment, ecc.BN254.ScalarField()); err != nil {
		t.Fatal(err)
	}

	wrong := *assignment
	wrong.Terms.Nonce = 8
	if err := test.IsSolved(&dvpIDCircuit{}, &wrong, ecc.BN254.ScalarField()); err == nil {
		t.Fatal("a changed nonce kept the dvp_id")
	}
}

type createCircuit struct {
	Terms SwapDvp
}

func (c *createCircuit) Define(api frontend.API) error {
	c.Terms.CheckRanges(api)
	c.Terms.CheckCreate(api)
	return nil
}

func TestCheckCreate(t *testing.T) {
	assert := test.NewAssert(t)
	base := loadVector(t).terms(t)
	twoPow := func(bits uint) *big.Int { return new(big.Int).Lsh(big.NewInt(1), bits) }

	assert.SolvingSucceeded(&createCircuit{}, &createCircuit{Terms: base}, test.WithCurves(ecc.BN254))

	unset := base
	unset.HasEarliestSettlement = 0
	unset.EarliestSettlementTimestamp = 0
	assert.SolvingSucceeded(&createCircuit{}, &createCircuit{Terms: unset}, test.WithCurves(ecc.BN254))

	invalid := map[string]func(*SwapDvp){
		"zero delivery amount":         func(t *SwapDvp) { t.DeliveryAmount = 0 },
		"zero payment amount":          func(t *SwapDvp) { t.PaymentAmount = 0 },
		"equal assets":                 func(t *SwapDvp) { t.PaymentAsset = t.DeliveryAsset },
		"earliest after expiry":        func(t *SwapDvp) { t.EarliestSettlementTimestamp = 1_800_000_001 },
		"earliest without flag":        func(t *SwapDvp) { t.HasEarliestSettlement = 0 },
		"non-boolean flag":             func(t *SwapDvp) { t.HasEarliestSettlement = 2 },
		"delivery amount over 64 bits": func(t *SwapDvp) { t.DeliveryAmount = twoPow(64) },
		"payment amount over 64 bits":  func(t *SwapDvp) { t.PaymentAmount = twoPow(64) },
		"expiry over 63 bits":          func(t *SwapDvp) { t.ExpiryTimestamp = twoPow(63) },
		"earliest over 63 bits":        func(t *SwapDvp) { t.EarliestSettlementTimestamp = twoPow(63) },
	}
	for name, mutate := range invalid {
		terms := base
		mutate(&terms)
		assert.Run(func(assert *test.Assert) {
			assert.SolvingFailed(&createCircuit{}, &createCircuit{Terms: terms}, test.WithCurves(ecc.BN254))
		}, name)
	}
}
