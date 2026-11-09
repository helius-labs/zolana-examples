//! Mollusk tests of the checks each instruction runs before its SPP CPI. They
//! load the SBF program from `target/deploy`, so build it first:
//! `cargo build-sbf --manifest-path program/Cargo.toml --tools-version v1.54 -- --features bpf-entrypoint`.
mod common;

#[path = "failing/dispatch.rs"]
mod dispatch;
#[path = "failing/escrow.rs"]
mod escrow;
#[path = "failing/withdraw.rs"]
mod withdraw;
