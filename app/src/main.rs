//! Ten-Cent Poker desktop client.
//!
//! By default this runs a headless smoke test that exercises the wallet, networking, and
//! crypto crates (so it works on a server/VM with no display). Pass `--gui` (requires the
//! `gui` feature: `cargo run -p poker-app --features gui -- --gui`) to launch the egui
//! desktop window.

use poker_crypto::{g1_scalar_mul, scalar_field_bits};
use poker_net::sample_table_uri;
use poker_wallet::{derive_address, DEV_MNEMONIC};

#[cfg(feature = "gui")]
mod ui;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "gui")]
    if std::env::args().any(|a| a == "--gui") {
        return Ok(ui::run()?);
    }

    smoke()
}

fn smoke() -> Result<(), Box<dyn std::error::Error>> {
    println!("┌─────────────────────────────────────────────┐");
    println!("│  Ten-Cent Poker — client smoke test          │");
    println!("└─────────────────────────────────────────────┘");

    // 1. Wallet — derive the dev address (account 0 of the canonical mnemonic).
    let addr = derive_address(DEV_MNEMONIC, 0)?;
    println!("wallet : account 0 address = {addr}");

    // 2. Net — produce a shareable, dialable table URI from a fresh peer identity.
    let uri = sample_table_uri();
    println!("net    : table URI         = {uri}");

    // 3. Crypto — exercise the BN254 proving stack used for ZK card dealing / settlement.
    let _point = g1_scalar_mul(42);
    println!("crypto : BN254 scalar field = {} bits (g·42 computed)", scalar_field_bits());

    println!("\nOK: wallet + net + crypto smoke passed.");
    Ok(())
}
