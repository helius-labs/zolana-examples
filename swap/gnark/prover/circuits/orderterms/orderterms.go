package orderterms

import (
	"github.com/consensys/gnark/frontend"

	gnarksdk "github.com/helius-labs/zolana/sdk-libs/gnark-sdk"
)

type OrderTerms struct {
	DestinationAsset  frontend.Variable
	DestinationAmount frontend.Variable
	MakerOwnerHash    frontend.Variable
	MakerViewingPk    [33]frontend.Variable
	Expiry            frontend.Variable
	TakerPkFe         frontend.Variable
}

func (o OrderTerms) Check(api frontend.API) {
	api.AssertIsDifferent(o.DestinationAmount, 0)
	api.ToBinary(o.DestinationAmount, 64)
}

func (o OrderTerms) MakerAddressFE(api frontend.API) frontend.Variable {
	viewingKeyHash := gnarksdk.HashBytes(api, o.MakerViewingPk[:])
	return gnarksdk.Poseidon(api, o.MakerOwnerHash, viewingKeyHash)
}

func (o OrderTerms) DataHash(api frontend.API, makerAddressFe frontend.Variable) frontend.Variable {
	return gnarksdk.Poseidon(
		api,
		o.DestinationAsset,
		o.DestinationAmount,
		makerAddressFe,
		o.Expiry,
		o.TakerPkFe,
	)
}
