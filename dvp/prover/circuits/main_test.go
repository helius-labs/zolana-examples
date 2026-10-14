package main

import (
	"flag"
	"fmt"
	"os"
	"strings"
	"testing"

	"github.com/consensys/gnark-crypto/ecc"
	"github.com/consensys/gnark/frontend"
	"github.com/consensys/gnark/frontend/cs/r1cs"

	"circuits/abort_dvp"
	"circuits/create_dvp"
	"circuits/reclaim_dvp"
	"circuits/settle_dvp"
)

// constraintsPath lists `<circuit> <constraints>` per circuit, in key setup
// order. The dvp-test CU benchmark reads it into BENCHMARK.md.
const constraintsPath = "testdata/constraints.txt"

var update = flag.Bool("update", false, "rewrite "+constraintsPath)

// TestConstraintCounts compiles each circuit as the bridge does for key setup
// (BN254 R1CS, no compile options) and checks its constraint count against
// testdata/constraints.txt. Rewrite the file after a circuit change with
//
//	dvp/scripts/circuits-go.sh test . -run TestConstraintCounts -update
func TestConstraintCounts(t *testing.T) {
	circuits := []struct {
		name    string
		circuit frontend.Circuit
	}{
		{"create_dvp", &create_dvp.Circuit{}},
		{"reclaim_dvp", &reclaim_dvp.Circuit{}},
		{"settle_dvp", &settle_dvp.Circuit{}},
		{"abort_dvp", &abort_dvp.Circuit{}},
	}
	var got strings.Builder
	for _, c := range circuits {
		cs, err := frontend.Compile(ecc.BN254.ScalarField(), r1cs.NewBuilder, c.circuit)
		if err != nil {
			t.Fatalf("compile %s: %v", c.name, err)
		}
		fmt.Fprintf(&got, "%s %d\n", c.name, cs.GetNbConstraints())
	}

	if *update {
		if err := os.WriteFile(constraintsPath, []byte(got.String()), 0o644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(constraintsPath)
	if err != nil {
		t.Fatal(err)
	}
	if got.String() != string(want) {
		t.Fatalf("%s is stale, rerun with -update:\ngot:\n%swant:\n%s", constraintsPath, got.String(), want)
	}
}
