package main

import "C"

import (
	"github.com/consensys/gnark/frontend"
	"stakingcircuits/intent"
	"zolana/gnarkffiprover"
)

func init() {
	gnarkffiprover.Register("intent", gnarkffiprover.Circuit{
		New:         func() frontend.Circuit { return &intent.Circuit{} },
		Commitments: 1,
	})
}

func main() {}
