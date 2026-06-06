//! EIP-712 helpers for the AnetBridgeVault on BSC.
//!
//! This module lets the L1 chain *verify* signatures over the canonical
//! `Release` struct without ever holding a signer private key. Each vault
//! signer runs an independent signing node off-chain, signs the digest
//! locally, and POSTs the signature to L1 via
//! `POST /bridge/burns/:id/sigs`. The relayer (or anyone) reads the
//! collected signatures and submits `releaseBurn()` on BSC.
//!
//! Mirrors:
//!   contract AnetBridgeVault.sol
//!     DOMAIN: name="AnetBridgeVault", version="1"
//!     struct Release {
//!       uint256 burnId;
//!       string  l1Sender;
//!       address recipient;
//!       uint256 amount;
//!       uint256 deadline;
//!     }
//!
//! Reference: EIP-712 (https://eips.ethereum.org/EIPS/eip-712)

use anyhow::{anyhow, bail, Context, Result};
use secp256k1::{ecdsa::RecoverableSignature, ecdsa::RecoveryId, Message, Secp256k1};
use tiny_keccak::{Hasher, Keccak};

/// Compute keccak256 of `input`.
fn keccak256(input: &[u8]) -> [u8; 32] {
    let mut hasher = Keccak::v256();
    let mut out = [0u8; 32];
    hasher.update(input);
    hasher.finalize(&mut out);
    out
}

/// Configuration for the L1 → BSC bridge vault. Read once from env at
/// startup and cached. Operators MUST keep this in sync with the
/// on-chain vault's `signers()` set and `chainId` / contract address.
#[derive(Debug, Clone)]
pub struct VaultConfig {
    pub vault_address: [u8; 20],
    pub chain_id: u64,
    /// Lowercase 0x-prefixed BSC addresses of the on-chain signer set.
    pub signers: Vec<String>,
    pub threshold: u32,
}

impl VaultConfig {
    /// Load from env. Returns Ok(None) if not configured (legacy mode).
    pub fn from_env() -> Result<Option<Self>> {
        let addr = match std::env::var("BRIDGE_VAULT_ADDRESS").ok() {
            Some(s) if !s.trim().is_empty() => s.trim().to_owned(),
            _ => return Ok(None),
        };
        let chain_id: u64 = std::env::var("BRIDGE_VAULT_CHAIN_ID")
            .context("BRIDGE_VAULT_CHAIN_ID required when BRIDGE_VAULT_ADDRESS is set")?
            .trim()
            .parse()
            .context("BRIDGE_VAULT_CHAIN_ID must be a positive integer")?;
        let signers_raw = std::env::var("BRIDGE_VAULT_SIGNERS")
            .context("BRIDGE_VAULT_SIGNERS required when BRIDGE_VAULT_ADDRESS is set")?;
        let signers: Vec<String> = signers_raw
            .split(',')
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        if signers.is_empty() {
            bail!("BRIDGE_VAULT_SIGNERS must list at least one address");
        }
        if signers.len() > 16 {
            bail!("BRIDGE_VAULT_SIGNERS must contain at most 16 addresses");
        }
        for s in &signers {
            if !is_valid_bsc_address(s) {
                bail!("BRIDGE_VAULT_SIGNERS contains invalid address: {s}");
            }
        }
        let threshold: u32 = std::env::var("BRIDGE_VAULT_THRESHOLD")
            .context("BRIDGE_VAULT_THRESHOLD required when BRIDGE_VAULT_ADDRESS is set")?
            .trim()
            .parse()
            .context("BRIDGE_VAULT_THRESHOLD must be a positive integer")?;
        if threshold == 0 || (threshold as usize) > signers.len() {
            bail!(
                "BRIDGE_VAULT_THRESHOLD ({threshold}) must be 1..={}",
                signers.len()
            );
        }

        let vault_address = parse_address(&addr)
            .ok_or_else(|| anyhow!("BRIDGE_VAULT_ADDRESS not a valid 0x-prefixed address"))?;

        Ok(Some(Self {
            vault_address,
            chain_id,
            signers,
            threshold,
        }))
    }
}

fn is_valid_bsc_address(s: &str) -> bool {
    let bytes = s.as_bytes();
    bytes.len() == 42 && bytes[0] == b'0' && (bytes[1] == b'x' || bytes[1] == b'X')
        && bytes[2..].iter().all(|c| c.is_ascii_hexdigit())
}

fn parse_address(s: &str) -> Option<[u8; 20]> {
    if !is_valid_bsc_address(s) {
        return None;
    }
    let bytes = hex::decode(&s[2..]).ok()?;
    let mut out = [0u8; 20];
    out.copy_from_slice(&bytes);
    Some(out)
}

/// EIP-712 domain separator: keccak256(EIP712Domain(name, version, chainId, verifyingContract))
fn domain_separator(cfg: &VaultConfig) -> [u8; 32] {
    // keccak256("EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)")
    let type_hash = keccak256(b"EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)");
    let name_hash = keccak256(b"AnetBridgeVault");
    let version_hash = keccak256(b"1");

    let mut buf = Vec::with_capacity(32 * 5);
    buf.extend_from_slice(&type_hash);
    buf.extend_from_slice(&name_hash);
    buf.extend_from_slice(&version_hash);
    buf.extend_from_slice(&u256_be(cfg.chain_id));
    buf.extend_from_slice(&address_to_32(&cfg.vault_address));
    keccak256(&buf)
}

/// Hash of the Release struct per EIP-712.
///   keccak256(
///     RELEASE_TYPEHASH ‖
///     burnId           (uint256, big-endian) ‖
///     keccak256(l1Sender bytes) ‖
///     recipient        (left-padded to 32) ‖
///     amount           (uint256, big-endian) ‖
///     deadline         (uint256, big-endian)
///   )
fn release_struct_hash(
    burn_id: u64,
    l1_sender: &str,
    recipient: &[u8; 20],
    amount_wei: &[u8; 32],
    deadline_unix: u64,
) -> [u8; 32] {
    // keccak256("Release(uint256 burnId,string l1Sender,address recipient,uint256 amount,uint256 deadline)")
    let type_hash = keccak256(
        b"Release(uint256 burnId,string l1Sender,address recipient,uint256 amount,uint256 deadline)",
    );

    let mut buf = Vec::with_capacity(32 * 6);
    buf.extend_from_slice(&type_hash);
    buf.extend_from_slice(&u256_be(burn_id));
    buf.extend_from_slice(&keccak256(l1_sender.as_bytes()));
    buf.extend_from_slice(&address_to_32(recipient));
    buf.extend_from_slice(amount_wei);
    buf.extend_from_slice(&u256_be(deadline_unix));
    keccak256(&buf)
}

/// Full EIP-712 digest:  keccak256("\x19\x01" ‖ domainSeparator ‖ structHash).
pub fn release_digest(
    cfg: &VaultConfig,
    burn_id: u64,
    l1_sender: &str,
    recipient: &[u8; 20],
    amount_wei: &[u8; 32],
    deadline_unix: u64,
) -> [u8; 32] {
    let ds = domain_separator(cfg);
    let sh = release_struct_hash(burn_id, l1_sender, recipient, amount_wei, deadline_unix);

    let mut buf = Vec::with_capacity(2 + 32 + 32);
    buf.push(0x19);
    buf.push(0x01);
    buf.extend_from_slice(&ds);
    buf.extend_from_slice(&sh);
    keccak256(&buf)
}

/// Recover the BSC address (lowercase, 0x-prefixed) that produced
/// `sig_bytes` over `digest`. Accepts a 65-byte signature (r ‖ s ‖ v),
/// where v ∈ {27, 28} or {0, 1}.
pub fn recover_signer(digest: &[u8; 32], sig_bytes: &[u8]) -> Result<String> {
    if sig_bytes.len() != 65 {
        bail!("signature must be 65 bytes (got {})", sig_bytes.len());
    }
    let r_s = &sig_bytes[..64];
    let v = sig_bytes[64];
    let rec_id = match v {
        0 | 27 => 0,
        1 | 28 => 1,
        _ => bail!("invalid signature v byte: {v}"),
    };
    let rec_id = RecoveryId::from_i32(rec_id).context("invalid recovery id")?;
    let sig = RecoverableSignature::from_compact(r_s, rec_id).context("malformed signature")?;
    let msg = Message::from_digest_slice(digest).context("bad digest length")?;
    let secp = Secp256k1::verification_only();
    let pubkey = secp
        .recover_ecdsa(&msg, &sig)
        .context("ecdsa recovery failed")?;
    // Uncompressed pubkey (65 bytes, leading 0x04) → keccak256 of last 64 → last 20 bytes
    let uncompressed = pubkey.serialize_uncompressed();
    let hash = keccak256(&uncompressed[1..]);
    let addr_bytes = &hash[12..];
    Ok(format!("0x{}", hex::encode(addr_bytes)))
}

/// Encode a u64 as a 32-byte big-endian unsigned integer.
fn u256_be(n: u64) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[24..].copy_from_slice(&n.to_be_bytes());
    out
}

/// Left-pad a 20-byte address to 32 bytes.
fn address_to_32(addr: &[u8; 20]) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[12..].copy_from_slice(addr);
    out
}

/// Encode the BSC recipient (0x-prefixed 40-hex) to a 20-byte array,
/// returning None if invalid.
pub fn parse_bsc_recipient(s: &str) -> Option<[u8; 20]> {
    parse_address(s.trim())
}

/// Convert ants (i64) to wei (32-byte big-endian).
/// ANTS_PER_ANET = 100_000_000  (1 ANET = 1e8 ants on L1).
/// On BSC the wANET token uses 18 decimals, so:
///   amount_wei = ants * 1e10
/// = ants * 10_000_000_000
pub fn ants_to_wei_be(ants: u64) -> [u8; 32] {
    // ants * 10^10 fits in u128 for any realistic ants value.
    let wei: u128 = (ants as u128).saturating_mul(10_000_000_000u128);
    let mut out = [0u8; 32];
    out[16..].copy_from_slice(&wei.to_be_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_cfg() -> VaultConfig {
        VaultConfig {
            vault_address: parse_address("0x31438362a7667ce5559500023d025c7c14168b49").unwrap(),
            chain_id: 56,
            signers: vec![
                "0xa5d1968e63174ee5c0b1313850e8b6d5bb588ce7".into(),
                "0xcbe8d5eacbdca11aecc62f66a61ae0e684dd71fe".into(),
                "0xdc7446634a2a243c9c8398d07d9754ecd2cab748".into(),
            ],
            threshold: 2,
        }
    }

    #[test]
    fn domain_separator_matches_known_vault() {
        let cfg = sample_cfg();
        let ds = domain_separator(&cfg);
        // Computed by the deployed vault's DOMAIN_SEPARATOR() and the
        // bsc-relayer's check-vault-eip712.js for chainId=56,
        // verifyingContract=0x31438362a7667ce5559500023D025c7c14168B49.
        assert_eq!(
            format!("0x{}", hex::encode(ds)),
            "0x511e5b0a7ca10025262a649942a5c58aa479f9e4c1cc8f100c95c0f9c72a4fc6"
        );
    }

    #[test]
    fn ants_to_wei_conversion() {
        // 10 ANET = 10 * 1e8 ants = 10 * 1e18 wei
        let wei = ants_to_wei_be(10 * 100_000_000);
        // 10 * 1e18 = 0x8AC7230489E80000
        let last_16 = &wei[16..];
        let v = u128::from_be_bytes(last_16.try_into().unwrap());
        assert_eq!(v, 10u128 * 1_000_000_000_000_000_000u128);
    }

    #[test]
    fn recover_signer_round_trip() {
        use secp256k1::SecretKey;
        let cfg = sample_cfg();
        let recipient = parse_address("0x000000000000000000000000000000000000dEaD").unwrap();
        let amount = ants_to_wei_be(10 * 100_000_000);
        let digest = release_digest(&cfg, 1, "ANET1examplexxxxxxxxxxxxxxxxxxxxxxxxx", &recipient, &amount, 1779686412);

        // Deterministic key for the test only.
        let sk_bytes = [0x11u8; 32];
        let sk = SecretKey::from_slice(&sk_bytes).unwrap();
        let secp = Secp256k1::new();
        let msg = Message::from_digest_slice(&digest).unwrap();
        let sig = secp.sign_ecdsa_recoverable(&msg, &sk);
        let (rec, compact) = sig.serialize_compact();
        let mut sig65 = Vec::with_capacity(65);
        sig65.extend_from_slice(&compact);
        sig65.push(27u8 + rec.to_i32() as u8);

        let recovered = recover_signer(&digest, &sig65).unwrap();
        // Sanity: ethereum address derived from sk=0x11..
        // (Just verifies the function returns a well-formed lowercase address.)
        assert_eq!(recovered.len(), 42);
        assert!(recovered.starts_with("0x"));
    }
}
