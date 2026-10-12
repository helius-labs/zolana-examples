package main

import "C"

import (
	"github.com/consensys/gnark/frontend"
	"stakingcircuits/cancel"
	"stakingcircuits/intent"
	"zolana/gnarkffiprover"
)

func init() {
	gnarkffiprover.Register("cancel", gnarkffiprover.Circuit{
		New:         func() frontend.Circuit { return &cancel.Circuit{} },
		Commitments: 1,
	})
	gnarkffiprover.Register("intent", gnarkffiprover.Circuit{
		New:         func() frontend.Circuit { return &intent.Circuit{} },
		Commitments: 1,
	})
}

func main() {}
