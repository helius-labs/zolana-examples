//! The swap circuits: Rust that runs natively on the client to check the
//! witness, and compiles through Xark for proving. Each circuit witnesses the
//! SPP transaction's UTXO hash preimages and recomputes its private
//! transaction hash, like the gnark implementation.
mod cancel;
mod make;
mod shared;
mod take;

pub use cancel::{Cancel, CancelPrivateInputs, CancelPublicInputs};
pub use make::{Make, MakePrivateInputs, MakePublicInputs};
pub use shared::{Commitment, OrderTerms, ProofInputUtxo};
pub use take::{Take, TakePrivateInputs, TakePublicInputs};
