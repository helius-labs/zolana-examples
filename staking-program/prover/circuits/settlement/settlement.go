package settlement

import (
	"fmt"

	"stakingcircuits/shared"

	"github.com/consensys/gnark/frontend"
	"github.com/reilabs/gnark-lean-extractor/v3/abstractor"
	"zolana/gnarksdk"
	"zolana/prover/circuits/gadget"
	spp "zolana/prover/circuits/spp_transaction/shared"
)

// N is the number of batch entries one settlement proof covers.
const N = 16

type Amounts struct {
	EscrowSol        frontend.Variable
	EscrowPoolTokens frontend.Variable
	PayoutSol        frontend.Variable
	PayoutPoolTokens frontend.Variable
}

// Context holds every value the program recomputes; its hash is the single
// public input.
type Context struct {
	SpendPrivateTxHash  frontend.Variable
	PayoutPrivateTxHash frontend.Variable
	CachedUtxoHash      frontend.Variable
	UtxoHashChain       frontend.Variable
	IntentCount         frontend.Variable
	EpochHash           frontend.Variable
	PoolMintHash        frontend.Variable
	CrankViewingHash    frontend.Variable
	EscrowOwnerIdentity frontend.Variable
	TreeID              frontend.Variable
	Pool                shared.Pool
	Amounts             Amounts
	// NullifierRoot is the SPP nullifier tree root the in-tree cancellations
	// are proven against; the program reads it from the escrow tree's root
	// history at the instruction's nullifier_root_index.
	NullifierRoot frontend.Variable
}

// Hash is the program's SettlementProofContext::hash.
func (c *Context) Hash(api frontend.API, entriesHash frontend.Variable) frontend.Variable {
	pool := gnarksdk.Poseidon(api, c.Pool.Lamports, c.Pool.Supply, c.Pool.DepositNumerator,
		c.Pool.DepositDenominator, c.Pool.WithdrawalNumerator, c.Pool.WithdrawalDenominator)
	batch := gnarksdk.Poseidon(api, c.Amounts.EscrowSol, c.Amounts.EscrowPoolTokens,
		c.Amounts.PayoutSol, c.Amounts.PayoutPoolTokens, entriesHash, c.NullifierRoot)
	return gnarksdk.Poseidon(api, c.SpendPrivateTxHash, c.PayoutPrivateTxHash, c.CachedUtxoHash,
		c.UtxoHashChain, c.IntentCount, c.EpochHash, c.PoolMintHash, c.CrankViewingHash,
		c.EscrowOwnerIdentity, c.TreeID, pool, batch)
}

// Slot is one stored batch entry. Absent slots repeat slot 0's keys with
// Present = 0 so every point and scalar stays valid.
//
// A cancelled entry is either queued (its nullifier PDA still exists, which
// the program checks) or in the nullifier tree (the forester absorbed the
// nullifier and closed the PDA). CancelledInTree marks the latter, which the
// circuit proves by the inclusion of IndexedLeafHash(nullifier,
// NullifierNextValue) at NullifierPathIndex under Context.NullifierRoot. The
// proof fields are unconstrained (zeros) when CancelledInTree is 0.
type Slot struct {
	Present               frontend.Variable
	Cancelled             frontend.Variable
	CancelledInTree       frontend.Variable
	NullifierNextValue    frontend.Variable
	NullifierPathElements [spp.NullifierTreeHeight]frontend.Variable
	NullifierPathIndex    frontend.Variable
	PoolAsset             frontend.Variable
	Amount                frontend.Variable
	Blinding              frontend.Variable
	NullifierPK           frontend.Variable
	PayoutBlinding        frontend.Variable
	SigningPK             [34]frontend.Variable
	UserPoint             [65]frontend.Variable
}

type Circuit struct {
	PublicInputHash      frontend.Variable `gnark:",public"`
	Context              Context
	Encryption           shared.BatchEncryption
	WithdrawalTxBlinding frontend.Variable
	PayoutTxBlinding     frontend.Variable
	Escrow0              Slot
	Escrow1              Slot
	Escrow2              Slot
	Escrow3              Slot
	Escrow4              Slot
	Escrow5              Slot
	Escrow6              Slot
	Escrow7              Slot
	Escrow8              Slot
	Escrow9              Slot
	Escrow10             Slot
	Escrow11             Slot
	Escrow12             Slot
	Escrow13             Slot
	Escrow14             Slot
	Escrow15             Slot
}

func (c *Circuit) Define(api frontend.API) error {
	slots := []Slot{c.Escrow0, c.Escrow1, c.Escrow2, c.Escrow3, c.Escrow4, c.Escrow5, c.Escrow6, c.Escrow7,
		c.Escrow8, c.Escrow9, c.Escrow10, c.Escrow11, c.Escrow12, c.Escrow13, c.Escrow14, c.Escrow15}
	return Constrain(api, c.PublicInputHash, &c.Context, &c.Encryption, c.WithdrawalTxBlinding, c.PayoutTxBlinding, slots)
}

// Constrain proves the spend (escrows -> cached UTXO) and payout (cached UTXO
// -> payouts) private transaction hashes for the given slots.
func Constrain(api frontend.API, publicInputHash frontend.Variable, ctx *Context, enc *shared.BatchEncryption,
	withdrawalTxBlinding, payoutTxBlinding frontend.Variable, slots []Slot) error {
	if len(slots) == 0 {
		return fmt.Errorf("settlement: no slots")
	}
	ctx.Pool.Validate(api)
	treeLE := shared.TreeIDBytesLE(api, ctx.TreeID)
	encryptor := enc.Prepare(api, shared.NewAES(api))
	api.AssertIsEqual(encryptor.CrankKeyHash(), ctx.CrankViewingHash)

	solAsset := gnarksdk.Poseidon(api, 0, 0)
	escrowOwner := gnarksdk.Poseidon(api, ctx.EscrowOwnerIdentity, gnarksdk.Poseidon(api, 0))
	payoutTotal := func(poolAsset frontend.Variable) (frontend.Variable, frontend.Variable) {
		escrow := api.Select(poolAsset, ctx.Amounts.EscrowPoolTokens, ctx.Amounts.EscrowSol)
		payout := api.Select(poolAsset, ctx.Amounts.PayoutSol, ctx.Amounts.PayoutPoolTokens)
		return api.Select(api.IsZero(escrow), 1, escrow), payout
	}

	var chain, entries, count, activeCount frontend.Variable = 0, 0, 0, 0
	var depositPrefix, depositPaid, withdrawalPrefix, withdrawalPaid frontend.Variable = 0, 0, 0, 0
	var solPayouts, tokenPayouts frontend.Variable = 0, 0
	api.AssertIsEqual(slots[0].Present, 1)
	var previous frontend.Variable = 1
	spendInputs := make([]frontend.Variable, len(slots))
	payoutOutputs := make([]frontend.Variable, len(slots))
	for i, s := range slots {
		api.AssertIsBoolean(s.Present)
		api.AssertIsBoolean(s.Cancelled)
		api.AssertIsBoolean(s.PoolAsset)
		// Present entries form a prefix starting at slot 0.
		api.AssertIsEqual(api.Mul(s.Present, api.Sub(1, previous)), 0)
		previous = s.Present
		api.AssertIsEqual(api.Mul(s.Cancelled, api.Sub(1, s.Present)), 0)
		api.AssertIsBoolean(s.CancelledInTree)
		api.AssertIsEqual(api.Mul(s.CancelledInTree, api.Sub(1, s.Cancelled)), 0)
		active := api.Sub(s.Present, s.Cancelled)
		api.ToBinary(s.Amount, 64)
		api.AssertIsEqual(api.Mul(s.Present, api.IsZero(s.Amount)), 0)

		user := encryptor.Recipient(s.UserPoint)
		escrow := gnarksdk.Utxo{
			Domain: spp.UtxoDomain, Owner: escrowOwner,
			Asset: api.Select(s.PoolAsset, ctx.PoolMintHash, solAsset), Amount: s.Amount, Blinding: s.Blinding,
			DataHash:     shared.IntentDataHash(api, ctx.EpochHash, s.SigningPK, s.NullifierPK, user.KeyHash),
			RingDataHash: 0, RingProgramID: 0, TreeID: ctx.TreeID,
		}
		escrowHash := escrow.Hash(api)
		nullifier := gnarksdk.Poseidon(api, escrowHash, s.Blinding, 0)
		chain = api.Select(s.Present, gnarksdk.Poseidon(api, chain, escrowHash), chain)
		assertNullifierInTree(api, s, nullifier, ctx.NullifierRoot)
		count = api.Add(count, s.Present)
		spendInputs[i] = api.Select(active, escrowHash, 0)

		// Pro-rata allocation over prefix sums, as sdk allocation::allocate.
		activeAmount := api.Mul(active, s.Amount)
		depositAmount := api.Mul(api.Sub(1, s.PoolAsset), activeAmount)
		withdrawalAmount := api.Mul(s.PoolAsset, activeAmount)
		depositPrefix = api.Add(depositPrefix, depositAmount)
		withdrawalPrefix = api.Add(withdrawalPrefix, withdrawalAmount)
		escrowTotal, payout := payoutTotal(s.PoolAsset)
		prefix := api.Select(s.PoolAsset, withdrawalPrefix, depositPrefix)
		paid := shared.DivFloor(api, api.Mul(prefix, payout), escrowTotal)
		paidBefore := api.Select(s.PoolAsset, withdrawalPaid, depositPaid)
		amount := api.Mul(active, api.Sub(paid, paidBefore))
		isDeposit := api.Mul(active, api.Sub(1, s.PoolAsset))
		isWithdrawal := api.Mul(active, s.PoolAsset)
		depositPaid = api.Select(isDeposit, paid, depositPaid)
		withdrawalPaid = api.Select(isWithdrawal, paid, withdrawalPaid)
		tokenPayouts = api.Add(tokenPayouts, api.Mul(isDeposit, amount))
		solPayouts = api.Add(solPayouts, api.Mul(isWithdrawal, amount))

		payoutUtxo := gnarksdk.Utxo{
			Domain: spp.UtxoDomain, Owner: gnarksdk.Poseidon(api, shared.OwnerIdentity(api, s.SigningPK), s.NullifierPK),
			Asset: api.Select(s.PoolAsset, solAsset, ctx.PoolMintHash), Amount: amount, Blinding: s.PayoutBlinding,
			DataHash: 0, RingDataHash: 0, RingProgramID: 0, TreeID: ctx.TreeID,
		}
		payoutHash := payoutUtxo.Hash(api)
		plaintext := shared.PayoutBytes(api, treeLE, api.Sub(1, s.PoolAsset), amount, s.PayoutBlinding)
		ciphertext := encryptor.Encrypt(user, plaintext, activeCount, payoutHash)
		contextHash := encryptor.ContextHash(ciphertext)
		activeCount = api.Add(activeCount, active)
		payoutOutputs[i] = api.Select(active, payoutHash, 0)

		// 0 active, 1 queued cancellation, 2 cancelled in the nullifier tree.
		cancelledCode := api.Add(s.Cancelled, s.CancelledInTree)
		leaf := gnarksdk.Poseidon(api, nullifier, cancelledCode, payoutOutputs[i], api.Select(active, contextHash, 0))
		entries = api.Select(s.Present, gnarksdk.Poseidon(api, entries, leaf), entries)
	}

	api.AssertIsEqual(ctx.IntentCount, count)
	api.AssertIsEqual(ctx.UtxoHashChain, chain)
	api.AssertIsEqual(ctx.Amounts.EscrowSol, depositPrefix)
	api.AssertIsEqual(ctx.Amounts.EscrowPoolTokens, withdrawalPrefix)
	sol, tokens := ctx.Pool.Budgets(api, depositPrefix, withdrawalPrefix)
	api.AssertIsEqual(ctx.Amounts.PayoutSol, sol)
	api.AssertIsEqual(ctx.Amounts.PayoutPoolTokens, tokens)
	api.AssertIsEqual(solPayouts, sol)
	api.AssertIsEqual(tokenPayouts, tokens)

	// With every entry cancelled no SPP transaction runs and the legs are zero.
	anyActive := api.Sub(1, api.IsZero(activeCount))
	spend := gnarksdk.PrivateTxHash(api, spendInputs, []frontend.Variable{ctx.CachedUtxoHash, 0}, withdrawalTxBlinding)
	payoutTx := gnarksdk.PrivateTxHash(api, []frontend.Variable{ctx.CachedUtxoHash}, payoutOutputs, payoutTxBlinding)
	api.AssertIsEqual(ctx.SpendPrivateTxHash, api.Select(anyActive, spend, 0))
	api.AssertIsEqual(ctx.PayoutPrivateTxHash, api.Select(anyActive, payoutTx, 0))
	api.AssertIsEqual(ctx.CachedUtxoHash, api.Select(anyActive, ctx.CachedUtxoHash, 0))

	api.AssertIsEqual(publicInputHash, ctx.Hash(api, entries))
	return nil
}

// assertNullifierInTree proves, when s.CancelledInTree is 1, that the indexed
// leaf (nullifier, s.NullifierNextValue) lies at s.NullifierPathIndex in the
// nullifier tree with root nullifierRoot. Inclusion of the nullifier's own
// leaf proves it was inserted, so the escrow is spent.
func assertNullifierInTree(api frontend.API, s Slot, nullifier, nullifierRoot frontend.Variable) {
	root := abstractor.Call(api, gadget.MerkleRootGadget{
		Hash:   gadget.IndexedLeafHash(api, nullifier, s.NullifierNextValue),
		Index:  api.ToBinary(s.NullifierPathIndex, spp.NullifierTreeHeight),
		Path:   s.NullifierPathElements[:],
		Height: spp.NullifierTreeHeight,
	})
	api.AssertIsEqual(api.Mul(s.CancelledInTree, api.Sub(root, nullifierRoot)), 0)
}
