package shared

import (
	"fmt"
	"math/big"

	"github.com/consensys/gnark/constraint/solver"
	"github.com/consensys/gnark/frontend"
	"github.com/consensys/gnark/std/math/cmp"
)

func init() { solver.RegisterHint(divRem) }

func divRem(_ *big.Int, inputs, outputs []*big.Int) error {
	if len(inputs) != 2 || len(outputs) != 2 || inputs[1].Sign() == 0 {
		return fmt.Errorf("invalid integer division")
	}
	outputs[0].QuoRem(inputs[0], inputs[1], outputs[1])
	return nil
}

// DivFloor proves integer division rather than division in the scalar field.
// Products of two u64 values fit in 128 bits, below the native field modulus.
func DivFloor(api frontend.API, numerator, denominator frontend.Variable) frontend.Variable {
	api.ToBinary(numerator, 128)
	api.ToBinary(denominator, 64)
	api.AssertIsDifferent(denominator, 0)
	result, err := api.Compiler().NewHint(divRem, 2, numerator, denominator)
	if err != nil {
		panic(err)
	}
	quotient, remainder := result[0], result[1]
	api.ToBinary(quotient, 64)
	api.ToBinary(remainder, 64)
	api.AssertIsEqual(numerator, api.Add(api.Mul(quotient, denominator), remainder))
	cmp.NewBoundedComparator(api, new(big.Int).Lsh(big.NewInt(1), 64), false).AssertIsLess(remainder, denominator)
	return quotient
}

func fee(api frontend.API, amount, numerator, denominator frontend.Variable) frontend.Variable {
	api.ToBinary(numerator, 64)
	api.ToBinary(denominator, 64)
	zero := api.IsZero(numerator)
	safeDenominator := api.Select(zero, 1, denominator)
	cmp.NewBoundedComparator(api, new(big.Int).Lsh(big.NewInt(1), 64), false).AssertIsLessEq(numerator, safeDenominator)
	// Native Fee.apply rounds up, including when the fee consumes the full amount.
	product := api.Mul(amount, numerator)
	rounded := DivFloor(api, api.Add(product, api.Sub(safeDenominator, 1)), safeDenominator)
	return api.Select(zero, 0, rounded)
}

type Pool struct {
	Lamports              frontend.Variable
	Supply                frontend.Variable
	DepositNumerator      frontend.Variable
	DepositDenominator    frontend.Variable
	WithdrawalNumerator   frontend.Variable
	WithdrawalDenominator frontend.Variable
}

func (p *Pool) Validate(api frontend.API) {
	api.ToBinary(p.Lamports, 64)
	api.ToBinary(p.Supply, 64)
	api.ToBinary(p.DepositNumerator, 64)
	api.ToBinary(p.DepositDenominator, 64)
	api.ToBinary(p.WithdrawalNumerator, 64)
	api.ToBinary(p.WithdrawalDenominator, 64)
}

func (p *Pool) GrossDeposit(api frontend.API, amount frontend.Variable) frontend.Variable {
	api.ToBinary(amount, 64)
	bootstrap := api.Or(api.IsZero(p.Lamports), api.IsZero(p.Supply))
	result := DivFloor(api, api.Mul(amount, api.Select(bootstrap, 1, p.Supply)), api.Select(bootstrap, 1, p.Lamports))
	return result
}

func (p *Pool) Deposit(api frontend.API, amount frontend.Variable) frontend.Variable {
	gross := p.GrossDeposit(api, amount)
	net := api.Sub(gross, fee(api, gross, p.DepositNumerator, p.DepositDenominator))
	api.ToBinary(net, 64)
	return net
}

func (p *Pool) GrossWithdrawal(api frontend.API, tokens frontend.Variable) frontend.Variable {
	api.ToBinary(tokens, 64)
	api.AssertIsEqual(api.Mul(api.IsZero(p.Supply), tokens), 0)
	// An unused zero-valued withdrawal remains defined for a bootstrap pool.
	return DivFloor(api, api.Mul(tokens, p.Lamports), api.Select(api.IsZero(p.Supply), 1, p.Supply))
}

func (p *Pool) Withdrawal(api frontend.API, tokens frontend.Variable) frontend.Variable {
	api.ToBinary(tokens, 64)
	burnt := api.Sub(tokens, fee(api, tokens, p.WithdrawalNumerator, p.WithdrawalDenominator))
	api.ToBinary(burnt, 64)
	return p.GrossWithdrawal(api, burnt)
}

// Budgets matches opposing intents before applying the pool fee to the net
// operation. Neither cohort pays a pool-operation fee for internally matched value.
func (p *Pool) Budgets(api frontend.API, deposits, withdrawals frontend.Variable) (sol, tokens frontend.Variable) {
	api.ToBinary(deposits, 64)
	api.ToBinary(withdrawals, 64)
	grossWithdrawal := p.GrossWithdrawal(api, withdrawals)
	depositSide := cmp.NewBoundedComparator(api, new(big.Int).Lsh(big.NewInt(1), 64), false).IsLessEq(grossWithdrawal, deposits)
	netDeposit := api.Select(depositSide, api.Sub(deposits, grossWithdrawal), 0)
	// This quote is only used on the withdrawal side; zero avoids overflow in
	// the unused conversion when the deposit cohort is larger than the pool.
	matchedTokens := p.GrossDeposit(api, api.Select(depositSide, 0, deposits))
	netWithdrawal := api.Select(depositSide, 0, api.Sub(withdrawals, matchedTokens))
	api.ToBinary(netDeposit, 64)
	api.ToBinary(netWithdrawal, 64)
	minted := p.Deposit(api, netDeposit)
	withdrawn := p.Withdrawal(api, netWithdrawal)
	api.AssertIsEqual(api.Mul(api.Sub(1, api.IsZero(netDeposit)), api.IsZero(minted)), 0)
	api.AssertIsEqual(api.Mul(api.Sub(1, api.IsZero(netWithdrawal)), api.IsZero(withdrawn)), 0)
	sol = api.Select(depositSide, grossWithdrawal, api.Add(deposits, withdrawn))
	tokens = api.Select(depositSide, api.Add(withdrawals, minted), matchedTokens)
	api.ToBinary(sol, 64)
	api.ToBinary(tokens, 64)
	return sol, tokens
}
