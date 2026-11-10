package escrowterms

import (
	"github.com/consensys/gnark/frontend"

	gnarksdk "github.com/helius-labs/zolana/sdk-libs/gnark-sdk"
)

type EscrowTerms struct {
	OwnerHash frontend.Variable
	Unlock    frontend.Variable
}

func (t EscrowTerms) DataHash(api frontend.API) frontend.Variable {
	return gnarksdk.Poseidon(api, t.OwnerHash, t.Unlock)
}
