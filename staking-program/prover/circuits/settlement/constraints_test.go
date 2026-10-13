package settlement

import (
	"testing"
	"time"

	"stakingcircuits/cancel"
	"stakingcircuits/intent"

	"github.com/consensys/gnark-crypto/ecc"
	"github.com/consensys/gnark/constraint"
	"github.com/consensys/gnark/frontend"
	"github.com/consensys/gnark/frontend/cs/r1cs"
)

const maxSettlementConstraints = 3_200_000

func TestConstraintCounts(t *testing.T) {
	for _, tc := range []struct {
		name    string
		circuit frontend.Circuit
	}{
		{"intent", &intent.Circuit{}},
		{"cancel", &cancel.Circuit{}},
		{"settlement", &Circuit{}},
	} {
		start := time.Now()
		cs, err := frontend.Compile(ecc.BN254.ScalarField(), r1cs.NewBuilder, tc.circuit)
		if err != nil {
			t.Fatalf("%s: %v", tc.name, err)
		}
		t.Logf("%s: %d constraints, compiled in %s", tc.name, cs.GetNbConstraints(), time.Since(start))
		// The verifying keys (main.go `Commitments: 1`) expect exactly one
		// BSB22 commitment.
		commitments, ok := cs.GetCommitments().(constraint.Groth16Commitments)
		if !ok {
			t.Fatalf("%s: commitments are %T, not Groth16", tc.name, cs.GetCommitments())
		}
		if n := len(commitments.CommitmentIndexes()); n != 1 {
			t.Fatalf("%s: %d BSB22 commitments, want 1", tc.name, n)
		}
		if tc.name == "settlement" && cs.GetNbConstraints() > maxSettlementConstraints {
			t.Fatalf("settlement has %d constraints, limit %d", cs.GetNbConstraints(), maxSettlementConstraints)
		}
	}
}
