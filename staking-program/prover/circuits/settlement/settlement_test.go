package settlement

import (
	"encoding/json"
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
		c("Amounts_PayoutPoolTokens"), f.int(t, metaPrefix+"EntriesHash", 0))
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

func TestSettlementFixturesSolve(t *testing.T) {
	for _, name := range []string{"settlement-4.json", "settlement-16.json", "settlement-cancelled.json"} {
		t.Run(name, func(t *testing.T) {
			f := load(t, name)
			if got, want := f.publicInputHash(t), f.int(t, "PublicInputHash", 0); got.Cmp(want) != 0 {
				t.Fatalf("Go context hash %s differs from the SDK's %s", got, want)
			}
			if err := solve(t, f); err != nil {
				t.Fatal(err)
			}
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
			err := solve(t, f)
			if err == nil {
				t.Fatal("mutated witness was accepted")
			}
			message := err.Error()
			if len(message) > 300 {
				message = message[:300]
			}
			t.Logf("rejected: %s", message)
		})
	}
}
