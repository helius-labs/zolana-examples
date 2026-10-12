package shared

import (
	"github.com/consensys/gnark-crypto/ecc"
	"github.com/consensys/gnark/frontend"
	"github.com/consensys/gnark/test"
	"testing"
)

type poolCircuit struct {
	Pool               Pool
	DepositAmount      frontend.Variable
	WithdrawalAmount   frontend.Variable
	DepositTokens      frontend.Variable `gnark:",public"`
	WithdrawalLamports frontend.Variable `gnark:",public"`
}

func (c *poolCircuit) Define(api frontend.API) error {
	c.Pool.Validate(api)
	api.AssertIsEqual(c.Pool.Deposit(api, c.DepositAmount), c.DepositTokens)
	api.AssertIsEqual(c.Pool.Withdrawal(api, c.WithdrawalAmount), c.WithdrawalLamports)
	return nil
}

func TestPoolIntegerQuotes(t *testing.T) {
	for _, witness := range []poolCircuit{
		{Pool: Pool{Lamports: 1000, Supply: 500, DepositNumerator: 1, DepositDenominator: 100, WithdrawalNumerator: 1, WithdrawalDenominator: 100}, DepositAmount: 160, WithdrawalAmount: 80, DepositTokens: 79, WithdrawalLamports: 158},
		{Pool: Pool{Lamports: 0, Supply: 0, DepositNumerator: 0, DepositDenominator: 0, WithdrawalNumerator: 0, WithdrawalDenominator: 0}, DepositAmount: 123, WithdrawalAmount: 0, DepositTokens: 123, WithdrawalLamports: 0},
		{Pool: Pool{Lamports: 7, Supply: 3, DepositNumerator: 0, DepositDenominator: 0, WithdrawalNumerator: 0, WithdrawalDenominator: 0}, DepositAmount: 9, WithdrawalAmount: 2, DepositTokens: 3, WithdrawalLamports: 4},
	} {
		if err := test.IsSolved(&poolCircuit{}, &witness, ecc.BN254.ScalarField()); err != nil {
			t.Fatal(err)
		}
		witness.DepositTokens = 999
		if err := test.IsSolved(&poolCircuit{}, &witness, ecc.BN254.ScalarField()); err == nil {
			t.Fatal("incorrect quote accepted")
		}
	}
}

func TestPoolRejectsInvalidFee(t *testing.T) {
	witness := poolCircuit{Pool: Pool{Lamports: 1000, Supply: 500, DepositNumerator: 2, DepositDenominator: 1, WithdrawalNumerator: 0, WithdrawalDenominator: 0}, DepositAmount: 160, WithdrawalAmount: 0, DepositTokens: 0, WithdrawalLamports: 0}
	if err := test.IsSolved(&poolCircuit{}, &witness, ecc.BN254.ScalarField()); err == nil {
		t.Fatal("fee above 100 percent accepted")
	}
}

type budgetsCircuit struct {
	Pool        Pool
	Deposits    frontend.Variable
	Withdrawals frontend.Variable
	Sol         frontend.Variable `gnark:",public"`
	Tokens      frontend.Variable `gnark:",public"`
}

func (c *budgetsCircuit) Define(api frontend.API) error {
	c.Pool.Validate(api)
	sol, tokens := c.Pool.Budgets(api, c.Deposits, c.Withdrawals)
	api.AssertIsEqual(sol, c.Sol)
	api.AssertIsEqual(tokens, c.Tokens)
	return nil
}

func TestMixedSettlementBudgets(t *testing.T) {
	pool := Pool{Lamports: 1000000, Supply: 1000000, DepositNumerator: 1, DepositDenominator: 100, WithdrawalNumerator: 1, WithdrawalDenominator: 100}
	for _, amounts := range [][4]int{{100, 100, 100, 100}, {100, 20, 20, 99}, {20, 100, 99, 20}, {100, 0, 0, 99}, {0, 100, 99, 0}, {0, 0, 0, 0}} {
		witness := budgetsCircuit{Pool: pool, Deposits: amounts[0], Withdrawals: amounts[1], Sol: amounts[2], Tokens: amounts[3]}
		if err := test.IsSolved(&budgetsCircuit{}, &witness, ecc.BN254.ScalarField()); err != nil {
			t.Fatal(err)
		}
		witness.Tokens = 999
		if err := test.IsSolved(&budgetsCircuit{}, &witness, ecc.BN254.ScalarField()); err == nil {
			t.Fatal("incorrect settlement budget accepted")
		}
	}
}
