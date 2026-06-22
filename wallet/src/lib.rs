//! EVM wallet: BIP-39 / BIP-32 hierarchical-deterministic key derivation and signing,
//! built on `alloy`. This crate is unverified (it wraps audited cryptography and is
//! exercised through tests, not Verus).

use alloy_primitives::Address;
use alloy_signer_local::{coins_bip39::English, MnemonicBuilder};

/// The canonical Anvil/Foundry development mnemonic. Deterministic, well-known, and
/// **never** to be used with real funds — handy for a reproducible smoke test.
pub const DEV_MNEMONIC: &str = "test test test test test test test test test test test junk";

/// Derive the Ethereum address for `account_index` of the given BIP-39 mnemonic
/// (standard `m/44'/60'/0'/0/{index}` derivation path).
pub fn derive_address(phrase: &str, account_index: u32) -> Result<Address, Box<dyn std::error::Error>> {
    let signer = MnemonicBuilder::<English>::default()
        .phrase(phrase)
        .index(account_index)?
        .build()?;
    Ok(signer.address())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_mnemonic_account0_is_canonical() {
        // Account 0 of the standard dev mnemonic is a value every Ethereum dev recognizes.
        let addr = derive_address(DEV_MNEMONIC, 0).unwrap();
        assert_eq!(
            addr.to_string().to_lowercase(),
            "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266"
        );
    }

    #[test]
    fn distinct_indices_give_distinct_addresses() {
        let a0 = derive_address(DEV_MNEMONIC, 0).unwrap();
        let a1 = derive_address(DEV_MNEMONIC, 1).unwrap();
        assert_ne!(a0, a1);
    }
}
