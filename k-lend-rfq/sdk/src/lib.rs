//! Client side of the private kVault RFQ: vault and reserve readers and
//! pricing (`pair`, `kvault`), the swap wire types and the user's pre-signing
//! checks (`swap`, `user`), and the transfer and message helpers both the user
//! and the market maker build transactions with (`transfer`, `message`).

pub mod kvault;
pub mod message;
pub mod pair;
mod price;
pub mod swap;
pub mod transfer;
pub mod user;
