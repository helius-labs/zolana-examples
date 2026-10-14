// Command circuits is built as the DvP example's C archive: the
// zolana/gnarkffiprover bridge with the DvP circuits registered.
package main

import "C"

import (
	"github.com/consensys/gnark/frontend"

	"circuits/create_dvp"
	"zolana/gnarkffiprover"
)

func init() {
	gnarkffiprover.Register("create_dvp", gnarkffiprover.Circuit{
		New: func() frontend.Circuit { return &create_dvp.Circuit{} },
	})
}

func main() {}
