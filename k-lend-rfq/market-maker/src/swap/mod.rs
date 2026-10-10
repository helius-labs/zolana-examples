//! The swap protocol on the maker's side: quote, fill, co-sign and settle,
//! with the open-order book that binds a fill to the quote it answers.

pub mod expiry;
pub mod fill;
pub mod orders;
pub mod quote;
pub mod settle;
