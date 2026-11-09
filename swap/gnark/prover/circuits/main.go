// Command circuits is built as the swap example's C archive: the
// zolana/gnarkffiprover bridge with the make, cancel and take circuits
// registered.
package main

import "C"

import (
	"github.com/consensys/gnark/frontend"

	"circuits/cancel"
	makecircuit "circuits/make"
	"circuits/take"
	"zolana/gnarkffiprover"
)

func init() {
	gnarkffiprover.Register("make", gnarkffiprover.Circuit{
		New: func() frontend.Circuit { return &makecircuit.Circuit{} },
	})
	gnarkffiprover.Register("cancel", gnarkffiprover.Circuit{
		New: func() frontend.Circuit { return &cancel.Circuit{} },
	})
	gnarkffiprover.Register("take", gnarkffiprover.Circuit{
		New: func() frontend.Circuit { return &take.Circuit{} },
	})
}

func main() {}
