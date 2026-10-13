package settlement

import (
	"encoding/json"
	"fmt"
	"math/big"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"stakingcircuits/internal/witness"

	"github.com/consensys/gnark-crypto/ecc"
	"github.com/consensys/gnark/constraint"
	"github.com/consensys/gnark/frontend"
	"github.com/consensys/gnark/frontend/cs/r1cs"
	"github.com/iden3/go-iden3-crypto/poseidon"
)

// The fixtures come from `cargo run -p private-staking-sdk --example
// settlement_fixture`: the exact witness map the SDK sends to the prover plus
// Meta_* keys the tests use to recompute the public input.
const metaPrefix = "Meta_"

var (
	compileOnce sync.Once
	compiled    constraint.ConstraintSystem
	compileErr  error
)

func constraintSystem(t *testing.T) constraint.ConstraintSystem {
	t.Helper()
	compileOnce.Do(func() {
		start := time.Now()
		compiled, compileErr = frontend.Compile(ecc.BN254.ScalarField(), r1cs.NewBuilder, &Circuit{})
		if compileErr == nil {
			t.Logf("compiled %d constraints in %s", compiled.GetNbConstraints(), time.Since(start))
		}
	})
	if compileErr != nil {
		t.Fatal(compileErr)
	}
	return compiled
}

type fixture map[string][]string

func load(t *testing.T, name string) fixture {
	t.Helper()
	data, err := os.ReadFile(filepath.Join("..", "testdata", name))
	if err != nil {
		t.Fatal(err)
	}
	var values fixture
	if err = json.Unmarshal(data, &values); err != nil {
		t.Fatal(err)
	}
	return values
}

func (f fixture) clone() fixture {
	out := make(fixture, len(f))
	for key, values := range f {
		out[key] = append([]string(nil), values...)
	}
	return out
}

func (f fixture) int(t *testing.T, key string, index int) *big.Int {
	t.Helper()
	values, ok := f[key]
	if !ok || index >= len(values) {
		t.Fatalf("fixture has no %s[%d]", key, index)
	}
	n, ok := new(big.Int).SetString(values[index], 10)
	if !ok {
		t.Fatalf("fixture %s[%d] is not decimal", key, index)
	}
	return n
}

func (f fixture) set(t *testing.T, key string, index int, value *big.Int) {
	t.Helper()
	if index >= len(f[key]) {
		t.Fatalf("fixture has no %s[%d]", key, index)
	}
	f[key][index] = value.String()
}

func (f fixture) add(t *testing.T, key string, delta int64) {
	t.Helper()
	f.set(t, key, 0, new(big.Int).Add(f.int(t, key, 0), big.NewInt(delta)))
}

func hash(t *testing.T, inputs ...*big.Int) *big.Int {
	t.Helper()
	h, err := poseidon.Hash(inputs)
	if err != nil {
		t.Fatal(err)
	}
	return h
}

// publicInputHash is the program's SettlementProofContext::hash over the
// fixture's context values.
func (f fixture) publicInputHash(t *testing.T) *big.Int {
	t.Helper()
	c := func(name string) *big.Int { return f.int(t, "Context_"+name, 0) }
	pool := hash(t, c("Pool_Lamports"), c("Pool_Supply"), c("Pool_DepositNumerator"),
		c("Pool_DepositDenominator"), c("Pool_WithdrawalNumerator"), c("Pool_WithdrawalDenominator"))
	batch := hash(t, c("Amounts_EscrowSol"), c("Amounts_EscrowPoolTokens"), c("Amounts_PayoutSol"),
		c("Amounts_PayoutPoolTokens"), f.int(t, metaPrefix+"EntriesHash", 0), c("NullifierRoot"))
	return hash(t, c("SpendPrivateTxHash"), c("PayoutPrivateTxHash"), c("CachedUtxoHash"),
		c("UtxoHashChain"), c("IntentCount"), c("EpochHash"), c("PoolMintHash"), c("CrankViewingHash"),
		c("EscrowOwnerIdentity"), c("TreeID"), pool, batch)
}

// rehash makes the public input consistent with mutated context values, so
// the targeted constraint, not the public hash, rejects the witness.
func (f fixture) rehash(t *testing.T) {
	t.Helper()
	f.set(t, "PublicInputHash", 0, f.publicInputHash(t))
}

func solve(t *testing.T, f fixture) error {
	t.Helper()
	values := make(map[string][]string, len(f))
	for key, value := range f {
		if !strings.HasPrefix(key, metaPrefix) {
			values[key] = value
		}
	}
	var assignment Circuit
	// Strict: the fixture names every circuit variable and nothing else.
	if err := witness.Assign(&assignment, values, false); err != nil {
		t.Fatal(err)
	}
	w, err := frontend.NewWitness(&assignment, ecc.BN254.ScalarField())
	if err != nil {
		t.Fatal(err)
	}
	start := time.Now()
	err = constraintSystem(t).IsSolved(w)
	t.Logf("solved in %s", time.Since(start))
	return err
}

// cancelledKinds lists the slots with Cancelled = 1 and those of them with
// CancelledInTree = 1.
func (f fixture) cancelledKinds(t *testing.T) (cancelled, inTree []int) {
	t.Helper()
	for i := 0; i < N; i++ {
		prefix := fmt.Sprintf("Escrow%d_", i)
		if f.int(t, prefix+"Cancelled", 0).Sign() != 0 {
			cancelled = append(cancelled, i)
		}
		if f.int(t, prefix+"CancelledInTree", 0).Sign() != 0 {
			inTree = append(inTree, i)
		}
	}
	return cancelled, inTree
}

func TestSettlementFixturesSolve(t *testing.T) {
	for _, tc := range []struct {
		name      string
		cancelled []int
		inTree    []int
	}{
		{"settlement-4.json", []int{1}, []int{1}},
		{"settlement-16.json", []int{3, 8, 13}, []int{8, 13}},
		{"settlement-cancelled.json", []int{0, 1, 2}, []int{1}},
	} {
		t.Run(tc.name, func(t *testing.T) {
			f := load(t, tc.name)
			if got, want := f.publicInputHash(t), f.int(t, "PublicInputHash", 0); got.Cmp(want) != 0 {
				t.Fatalf("Go context hash %s differs from the SDK's %s", got, want)
			}
			cancelled, inTree := f.cancelledKinds(t)
			if fmt.Sprint(cancelled, inTree) != fmt.Sprint(tc.cancelled, tc.inTree) {
				t.Fatalf("cancelled %v in tree %v, want %v and %v", cancelled, inTree, tc.cancelled, tc.inTree)
			}
			if err := solve(t, f); err != nil {
				t.Fatal(err)
			}
		})
	}
}

func assertRejected(t *testing.T, f fixture) {
	t.Helper()
	err := solve(t, f)
	if err == nil {
		t.Fatal("mutated witness was accepted")
	}
	message := err.Error()
	if len(message) > 300 {
		message = message[:300]
	}
	t.Logf("rejected: %s", message)
}

// In settlement-4 slot 1 is cancelled in the nullifier tree; in settlement-16
// slot 3 is a queued cancellation.
func TestSettlementRejectsInvalidNullifierInclusion(t *testing.T) {
	one := big.NewInt(1)
	for _, tc := range []struct {
		name    string
		fixture string
		mutate  func(t *testing.T, f fixture)
	}{
		{"in-tree flag on an active slot", "settlement-4.json", func(t *testing.T, f fixture) {
			f.set(t, "Escrow0_CancelledInTree", 0, one)
		}},
		{"in-tree flag on an absent slot", "settlement-4.json", func(t *testing.T, f fixture) {
			f.set(t, "Escrow5_CancelledInTree", 0, one)
		}},
		{"non-boolean in-tree flag", "settlement-4.json", func(t *testing.T, f fixture) {
			f.set(t, "Escrow1_CancelledInTree", 0, big.NewInt(2))
		}},
		{"wrong next value", "settlement-4.json", func(t *testing.T, f fixture) {
			f.add(t, "Escrow1_NullifierNextValue", 1)
		}},
		{"wrong path index", "settlement-4.json", func(t *testing.T, f fixture) {
			f.add(t, "Escrow1_NullifierPathIndex", 1)
		}},
		{"wrong path element", "settlement-4.json", func(t *testing.T, f fixture) {
			f.set(t, "Escrow1_NullifierPathElements", 39, new(big.Int).Add(f.int(t, "Escrow1_NullifierPathElements", 39), one))
		}},
		{"wrong nullifier root", "settlement-4.json", func(t *testing.T, f fixture) {
			f.add(t, "Context_NullifierRoot", 1)
			f.rehash(t)
		}},
		{"absorbed cancellation claimed queued changes its leaf", "settlement-4.json", func(t *testing.T, f fixture) {
			f.set(t, "Escrow1_CancelledInTree", 0, big.NewInt(0))
		}},
		{"queued cancellation flagged in tree with zero proof", "settlement-16.json", func(t *testing.T, f fixture) {
			f.set(t, "Escrow3_CancelledInTree", 0, one)
		}},
		{"queued cancellation flagged in tree with another entry's proof", "settlement-16.json", func(t *testing.T, f fixture) {
			f.set(t, "Escrow3_CancelledInTree", 0, one)
			for _, field := range []string{"NullifierNextValue", "NullifierPathElements", "NullifierPathIndex"} {
				f["Escrow3_"+field] = append([]string(nil), f["Escrow8_"+field]...)
			}
		}},
	} {
		t.Run(tc.name, func(t *testing.T) {
			f := load(t, tc.fixture).clone()
			tc.mutate(t, f)
			assertRejected(t, f)
		})
	}
}

func TestSettlementRejectsInvalidWitnesses(t *testing.T) {
	base := load(t, "settlement-4.json")
	one := big.NewInt(1)
	for _, tc := range []struct {
		name   string
		mutate func(t *testing.T, f fixture)
	}{
		{"wrong utxo hash chain", func(t *testing.T, f fixture) {
			f.add(t, "Context_UtxoHashChain", 1)
			f.rehash(t)
		}},
		{"intent count off by one", func(t *testing.T, f fixture) {
			f.add(t, "Context_IntentCount", 1)
			f.rehash(t)
		}},
		{"present gap", func(t *testing.T, f fixture) {
			f.set(t, "Escrow1_Present", 0, big.NewInt(0))
			f.set(t, "Escrow1_Cancelled", 0, big.NewInt(0))
		}},
		{"cancelled absent slot", func(t *testing.T, f fixture) {
			f.set(t, "Escrow5_Cancelled", 0, one)
		}},
		// The next three change values the escrow and payout UTXO hashes
		// commit to. The test cannot recompute those hashes natively, so the
		// witness fails at the escrow totals, the hash chain, or the payout
		// hashes, and the names say so instead of claiming one constraint.
		{"flipped pool asset breaks the escrow totals and chain", func(t *testing.T, f fixture) {
			f.set(t, "Escrow0_PoolAsset", 0, one)
		}},
		{"zero amount breaks the escrow totals and chain", func(t *testing.T, f fixture) {
			f.set(t, "Escrow0_Amount", 0, big.NewInt(0))
		}},
		{"wrong escrow owner identity", func(t *testing.T, f fixture) {
			f.add(t, "Context_EscrowOwnerIdentity", 1)
			f.rehash(t)
		}},
		{"user point replaced by the crank point", func(t *testing.T, f fixture) {
			f["Escrow0_UserPoint"] = append([]string(nil), f["Encryption_CrankPoint"]...)
		}},
		{"changed payout blinding breaks the payout hashes", func(t *testing.T, f fixture) {
			f.add(t, "Escrow0_PayoutBlinding", 1)
		}},
		{"payout pool tokens plus one", func(t *testing.T, f fixture) {
			f.add(t, "Context_Amounts_PayoutPoolTokens", 1)
			f.rehash(t)
		}},
		{"fee numerator above denominator", func(t *testing.T, f fixture) {
			f.set(t, "Context_Pool_DepositNumerator", 0,
				new(big.Int).Add(f.int(t, "Context_Pool_DepositDenominator", 0), one))
			f.rehash(t)
		}},
		{"wrong cached utxo hash", func(t *testing.T, f fixture) {
			f.add(t, "Context_CachedUtxoHash", 1)
			f.rehash(t)
		}},
		{"tx viewing key byte zeroed", func(t *testing.T, f fixture) {
			f.set(t, "Encryption_TxViewingPK", 1, big.NewInt(0))
		}},
		{"active entry flagged cancelled", func(t *testing.T, f fixture) {
			f.set(t, "Escrow0_Cancelled", 0, one)
		}},
	} {
		t.Run(tc.name, func(t *testing.T) {
			f := base.clone()
			tc.mutate(t, f)
			assertRejected(t, f)
		})
	}
}
