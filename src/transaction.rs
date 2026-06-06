use std::collections::BTreeMap;

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use ripemd::Ripemd160;
use secp256k1::{
    ecdsa::{RecoverableSignature, RecoveryId},
    Message, PublicKey, Secp256k1,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

const ANET_ADDRESS_PREFIX: &str = "ANET";

/// Minimum transaction fee: 1 000 ANTS (0.00001 ANET).
/// Every transfer must include at least this fee, which is
/// distributed equally across all validator wallets in the block.
pub const MIN_FEE_ANTS: u64 = 1_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    #[serde(default = "default_tx_type")]
    pub tx_type: String,
    pub from: String,
    pub to: String,
    pub amount_ants: u64,
    pub fee_ants: u64,
    #[serde(default)]
    pub nonce: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub memo: String,
    pub timestamp: DateTime<Utc>,
    #[serde(default)]
    pub chain_id: String,
    #[serde(default = "default_payload")]
    pub payload: Value,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub signature: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tx_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedTransactionRequest {
    pub tx_type: String,
    pub from: String,
    pub to: String,
    pub amount_ants: u64,
    pub fee_ants: u64,
    pub nonce: u64,
    pub timestamp: DateTime<Utc>,
    pub chain_id: String,
    #[serde(default = "default_payload")]
    pub payload: Value,
    pub signature: String,
    pub tx_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedActionAuthorization {
    pub wallet: String,
    pub nonce: u64,
    pub timestamp: DateTime<Utc>,
    pub chain_id: String,
    #[serde(default = "default_payload")]
    pub payload: Value,
    pub signature: String,
    pub action_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TPoWProofSubmission {
    pub miner: String,
    pub nonce: u64,
    pub timestamp: DateTime<Utc>,
    pub chain_id: String,
    pub proof_hash: String,
    pub difficulty: u32,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionRequest {
    pub from: String,
    pub to: String,
    pub amount_ants: u64,
    pub fee_ants: u64,
    #[serde(default)]
    pub memo: String,
    #[serde(default)]
    pub nonce: u64,
    #[serde(default)]
    pub chain_id: String,
    #[serde(default = "default_payload")]
    pub payload: Value,
    #[serde(default)]
    pub signature: String,
    #[serde(default)]
    pub tx_hash: String,
    #[serde(default)]
    pub sender_seed: String,
}

impl SignedTransactionRequest {
    pub fn into_transaction(self) -> Result<Transaction> {
        let memo = extract_memo_from_payload(&self.payload)?;

        let tx = Transaction {
            tx_type: self.tx_type.trim().to_ascii_lowercase(),
            from: self.from.trim().to_uppercase(),
            to: self.to.trim().to_uppercase(),
            amount_ants: self.amount_ants,
            fee_ants: self.fee_ants,
            nonce: self.nonce,
            memo,
            timestamp: self.timestamp,
            chain_id: self.chain_id.trim().to_owned(),
            payload: self.payload,
            signature: self.signature.trim().to_owned(),
            tx_hash: self.tx_hash.trim().to_ascii_lowercase(),
        };

        tx.validate_signed_for_chain(&tx.chain_id)?;
        Ok(tx)
    }
}

impl TransactionRequest {
    pub fn into_transaction(self) -> Result<Transaction> {
        if !self.sender_seed.trim().is_empty() {
            return Err(anyhow!(
                "sender_seed transport is permanently disabled; submit signed transaction payload"
            ));
        }

        SignedTransactionRequest {
            tx_type: default_tx_type(),
            from: self.from,
            to: self.to,
            amount_ants: self.amount_ants,
            fee_ants: self.fee_ants,
            nonce: self.nonce,
            timestamp: Utc::now(),
            chain_id: self.chain_id,
            payload: if self.payload.is_null() {
                if self.memo.trim().is_empty() {
                    Value::Object(serde_json::Map::new())
                } else {
                    let mut object = serde_json::Map::new();
                    object.insert(
                        "memo".to_owned(),
                        Value::String(self.memo.trim().to_owned()),
                    );
                    Value::Object(object)
                }
            } else {
                self.payload
            },
            signature: self.signature,
            tx_hash: self.tx_hash,
        }
        .into_transaction()
    }
}

impl Transaction {
    pub fn validate(&self) -> Result<()> {
        self.validate_basic()?;
        if self.is_legacy() {
            return Ok(());
        }

        self.validate_signed_fields()?;
        Ok(())
    }

    pub fn validate_signed_for_chain(&self, expected_chain_id: &str) -> Result<()> {
        self.validate_basic()?;
        if self.is_legacy() {
            return Err(anyhow!(
                "legacy unsigned transaction is not accepted in mempool"
            ));
        }

        if self.chain_id.trim() != expected_chain_id.trim() {
            return Err(anyhow!("transaction chain_id does not match this node"));
        }

        // S2-4: mempool admission strictly requires nonce >= 1, regardless of
        // chain_id. The looser check inside `validate_signed_fields` exists
        // only so historical blocks (which may contain genesis-era signed txs
        // with nonce=0 from the chain's earliest deploy on `anet-mainnet`)
        // continue to re-validate during state replay. Fresh inbound traffic
        // never gets that exemption.
        if self.nonce < 1 {
            return Err(anyhow!(
                "transaction nonce must be greater than zero for mempool admission"
            ));
        }

        self.validate_signed_fields()
    }

    pub fn validate_basic(&self) -> Result<()> {
        if self.from.is_empty() {
            return Err(anyhow!("transaction sender is required"));
        }

        if self.to.is_empty() {
            return Err(anyhow!("transaction recipient is required"));
        }

        if self.from == self.to {
            return Err(anyhow!("transaction sender and recipient must differ"));
        }

        if self.amount_ants == 0 {
            return Err(anyhow!("transaction amount must be greater than zero"));
        }

        if self.fee_ants < MIN_FEE_ANTS {
            return Err(anyhow!(
                "transaction fee must be at least {MIN_FEE_ANTS} ANTS (0.00001 ANET)"
            ));
        }

        if !is_valid_anet_wallet(&self.from) {
            return Err(anyhow!("transaction sender must be a valid ANET wallet"));
        }

        if !is_valid_anet_wallet(&self.to) {
            return Err(anyhow!("transaction recipient must be a valid ANET wallet"));
        }

        if self.memo.chars().count() > 160 {
            return Err(anyhow!("transaction memo must be 160 characters or fewer"));
        }

        if self
            .memo
            .chars()
            .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t'))
        {
            return Err(anyhow!(
                "transaction memo contains unsupported control characters"
            ));
        }

        Ok(())
    }

    fn validate_signed_fields(&self) -> Result<()> {
        if self.tx_type.trim().is_empty() {
            return Err(anyhow!("tx_type is required"));
        }

        if self.nonce < 1 && self.chain_id != "anet-mainnet" {
            // Allow nonce=0 only for genesis/mainnet transactions
            // (legacy blocks before signed tx system was added)
            return Err(anyhow!(
                "nonce must be greater than zero for non-genesis chains"
            ));
        }

        if self.chain_id.trim().is_empty() {
            return Err(anyhow!("chain_id is required"));
        }

        if self.signature.trim().is_empty() {
            return Err(anyhow!("signature is required"));
        }

        let canonical_hash = self.compute_canonical_hash()?;
        if self.tx_hash.trim().to_ascii_lowercase() != canonical_hash {
            return Err(anyhow!(
                "tx_hash mismatch for canonical transaction payload"
            ));
        }

        let recovered = recover_address_from_signature(&canonical_hash, &self.signature)?;
        if recovered != self.from {
            return Err(anyhow!(
                "signature recovery does not match transaction sender"
            ));
        }

        Ok(())
    }

    fn is_legacy(&self) -> bool {
        // Legacy transactions have no signature and no tx_hash (pre-signed-payload era)
        self.tx_hash.trim().is_empty() && self.signature.trim().is_empty()
    }

    fn compute_canonical_hash(&self) -> Result<String> {
        let signing_bytes = canonical_signing_bytes(
            &self.tx_type,
            &self.from,
            &self.to,
            self.amount_ants,
            self.fee_ants,
            self.nonce,
            self.timestamp,
            &self.chain_id,
            &self.payload,
        )?;
        let mut hasher = Sha256::new();
        hasher.update(signing_bytes);
        Ok(hex::encode(hasher.finalize()))
    }

    pub fn id(&self) -> Result<String> {
        if !self.tx_hash.trim().is_empty() {
            return Ok(self.tx_hash.trim().to_ascii_lowercase());
        }

        let payload = serde_json::to_vec(self)?;
        let mut hasher = Sha256::new();
        hasher.update(payload);
        Ok(hex::encode(hasher.finalize()))
    }

    pub fn total_debit(&self) -> Result<u64> {
        self.amount_ants
            .checked_add(self.fee_ants)
            .ok_or_else(|| anyhow!("transaction total debit overflowed"))
    }
}

fn extract_memo_from_payload(payload: &Value) -> Result<String> {
    let memo = payload
        .as_object()
        .and_then(|object| object.get("memo"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned();

    if memo.chars().count() > 160 {
        return Err(anyhow!("transaction memo must be 160 characters or fewer"));
    }

    Ok(memo)
}

fn canonical_signing_bytes(
    tx_type: &str,
    from: &str,
    to: &str,
    amount_ants: u64,
    fee_ants: u64,
    nonce: u64,
    timestamp: DateTime<Utc>,
    chain_id: &str,
    payload: &Value,
) -> Result<Vec<u8>> {
    let payload_canonical = canonical_json_string(payload)?;
    let preimage = format!(
        "v1|{}|{}|{}|{}|{}|{}|{}|{}|{}",
        tx_type.trim().to_ascii_lowercase(),
        from.trim().to_uppercase(),
        to.trim().to_uppercase(),
        amount_ants,
        fee_ants,
        nonce,
        timestamp.timestamp_millis(),
        chain_id.trim(),
        payload_canonical
    );
    Ok(preimage.into_bytes())
}

fn canonical_action_signing_bytes(
    action_type: &str,
    wallet: &str,
    nonce: u64,
    timestamp: DateTime<Utc>,
    chain_id: &str,
    payload: &Value,
) -> Result<Vec<u8>> {
    let payload_canonical = canonical_json_string(payload)?;
    let preimage = format!(
        "action-v1|{}|{}|{}|{}|{}|{}",
        action_type.trim().to_ascii_lowercase(),
        wallet.trim().to_uppercase(),
        nonce,
        timestamp.timestamp_millis(),
        chain_id.trim(),
        payload_canonical
    );
    Ok(preimage.into_bytes())
}

fn canonical_proof_bytes(
    miner: &str,
    nonce: u64,
    timestamp: DateTime<Utc>,
    chain_id: &str,
    proof_hash: &str,
    difficulty: u32,
) -> Result<Vec<u8>> {
    let preimage = format!(
        "proof-v1|{}|{}|{}|{}|{}|{}",
        miner.trim().to_uppercase(),
        nonce,
        timestamp.timestamp_millis(),
        chain_id.trim(),
        proof_hash.trim().to_ascii_lowercase(),
        difficulty
    );
    Ok(preimage.into_bytes())
}

pub fn verify_tpow_proof_submission(
    proof: &TPoWProofSubmission,
    expected_chain_id: &str,
) -> Result<String> {
    let normalized_miner = proof.miner.trim().to_uppercase();
    if !is_valid_anet_wallet(&normalized_miner) {
        return Err(anyhow!("miner must be a valid ANET address"));
    }
    if proof.nonce == 0 {
        return Err(anyhow!("proof nonce must be greater than zero"));
    }
    if proof.chain_id.trim().is_empty() {
        return Err(anyhow!("proof chain_id is required"));
    }
    if proof.chain_id.trim() != expected_chain_id.trim() {
        return Err(anyhow!("proof chain_id does not match this node"));
    }
    if proof.signature.trim().is_empty() {
        return Err(anyhow!("proof signature is required"));
    }
    if proof.proof_hash.trim().is_empty() {
        return Err(anyhow!("proof_hash is required"));
    }
    if proof.difficulty < 1 || proof.difficulty > 32 {
        return Err(anyhow!("proof difficulty must be between 1 and 32"));
    }

    let normalized_proof_hash = proof.proof_hash.trim().to_ascii_lowercase();
    if normalized_proof_hash.len() != 64
        || !normalized_proof_hash
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(anyhow!(
            "proof_hash must be a 32-byte SHA256 digest encoded as hex"
        ));
    }

    let expected_proof_hash = compute_pow_hash(&normalized_miner, proof.timestamp, proof.nonce);
    if normalized_proof_hash != expected_proof_hash {
        return Err(anyhow!(
            "proof_hash does not match miner/timestamp/nonce preimage"
        ));
    }

    let proof_hash_bytes = hex::decode(&normalized_proof_hash)?;
    if leading_zero_bits(&proof_hash_bytes) < proof.difficulty {
        return Err(anyhow!(
            "proof_hash does not satisfy declared difficulty target"
        ));
    }

    let proof_bytes = canonical_proof_bytes(
        &normalized_miner,
        proof.nonce,
        proof.timestamp,
        &proof.chain_id,
        &normalized_proof_hash,
        proof.difficulty,
    )?;
    let mut hasher = Sha256::new();
    hasher.update(proof_bytes);
    let canonical_hash = hex::encode(hasher.finalize());

    let recovered = recover_address_from_signature(&canonical_hash, &proof.signature)?;
    if recovered != normalized_miner {
        return Err(anyhow!(
            "proof signature recovery does not match miner wallet"
        ));
    }

    Ok(normalized_miner)
}

fn compute_pow_hash(miner: &str, timestamp: DateTime<Utc>, nonce: u64) -> String {
    let preimage = format!(
        "{}|{}|{}",
        miner.trim().to_uppercase(),
        timestamp.timestamp_millis(),
        nonce
    );
    let mut hasher = Sha256::new();
    hasher.update(preimage.as_bytes());
    hex::encode(hasher.finalize())
}

fn leading_zero_bits(bytes: &[u8]) -> u32 {
    let mut count = 0_u32;
    for byte in bytes {
        if *byte == 0 {
            count += 8;
            continue;
        }
        count += byte.leading_zeros() - 24;
        break;
    }
    count
}

pub fn verify_signed_action_authorization(
    action_type: &str,
    auth: &SignedActionAuthorization,
    expected_chain_id: &str,
) -> Result<String> {
    let normalized_wallet = auth.wallet.trim().to_uppercase();
    if !is_valid_anet_wallet(&normalized_wallet) {
        return Err(anyhow!("wallet must be a valid ANET address"));
    }
    if auth.nonce == 0 {
        return Err(anyhow!("action nonce must be greater than zero"));
    }
    if auth.chain_id.trim().is_empty() {
        return Err(anyhow!("action chain_id is required"));
    }
    if auth.chain_id.trim() != expected_chain_id.trim() {
        return Err(anyhow!("action chain_id does not match this node"));
    }
    if auth.signature.trim().is_empty() {
        return Err(anyhow!("action signature is required"));
    }
    if auth.action_hash.trim().is_empty() {
        return Err(anyhow!("action_hash is required"));
    }

    let signing_bytes = canonical_action_signing_bytes(
        action_type,
        &normalized_wallet,
        auth.nonce,
        auth.timestamp,
        &auth.chain_id,
        &auth.payload,
    )?;
    let mut hasher = Sha256::new();
    hasher.update(signing_bytes);
    let canonical_hash = hex::encode(hasher.finalize());

    if auth.action_hash.trim().to_ascii_lowercase() != canonical_hash {
        return Err(anyhow!("action_hash mismatch for canonical action payload"));
    }

    let recovered = recover_address_from_signature(&canonical_hash, &auth.signature)?;
    if recovered != normalized_wallet {
        return Err(anyhow!("signature recovery does not match action wallet"));
    }

    Ok(normalized_wallet)
}

pub(crate) fn canonical_json_string(value: &Value) -> Result<String> {
    match value {
        Value::Null => Ok("null".to_owned()),
        Value::Bool(boolean) => Ok(if *boolean {
            "true".to_owned()
        } else {
            "false".to_owned()
        }),
        Value::Number(number) => Ok(number.to_string()),
        Value::String(text) => serde_json::to_string(text).map_err(Into::into),
        Value::Array(values) => {
            let mut out = String::from("[");
            for (index, item) in values.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&canonical_json_string(item)?);
            }
            out.push(']');
            Ok(out)
        }
        Value::Object(object) => {
            let mut sorted = BTreeMap::new();
            for (key, val) in object {
                sorted.insert(key, val);
            }

            let mut out = String::from("{");
            for (index, (key, val)) in sorted.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(key)?);
                out.push(':');
                out.push_str(&canonical_json_string(val)?);
            }
            out.push('}');
            Ok(out)
        }
    }
}

/// secp256k1 group order divided by 2, big-endian. Signatures whose `s`
/// component exceeds this value are the malleated twin of an equally valid
/// low-s signature and are rejected per EIP-2 / BIP-62. Matching the BSC
/// vault's behavior keeps cross-chain attestations consistent.
const SECP256K1_HALF_N_BE: [u8; 32] = [
    0x7F, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
    0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFE,
    0xBA, 0xAE, 0xDC, 0xE6, 0xAF, 0x48, 0xA0, 0x3B,
    0xBF, 0xD2, 0x5E, 0x8C, 0xD0, 0x36, 0x41, 0x40,
];

fn is_low_s(s_be: &[u8]) -> bool {
    // Lexicographic big-endian comparison is the correct numeric comparison
    // for fixed-width unsigned ints.
    s_be <= &SECP256K1_HALF_N_BE[..]
}

pub fn recover_address_from_signature(hash_hex: &str, signature_hex: &str) -> Result<String> {
    let hash = hex::decode(hash_hex.trim())?;
    if hash.len() != 32 {
        return Err(anyhow!("tx_hash must be a 32-byte SHA256 digest"));
    }

    let signature = hex::decode(signature_hex.trim())?;
    if signature.len() != 65 {
        return Err(anyhow!(
            "signature must be 65 bytes (r||s||recovery_id) encoded as hex"
        ));
    }

    // S1-4: reject high-s (signature malleability). Without this, every valid
    // signature has a malleated twin that recovers the same address but has a
    // different (r||s||v) byte layout, so `tx_hash` no longer uniquely
    // identifies a signed transaction.
    if !is_low_s(&signature[32..64]) {
        return Err(anyhow!(
            "signature has high-s; only low-s (EIP-2) signatures are accepted"
        ));
    }

    let recovery_raw = signature[64];
    let recovery_id = match recovery_raw {
        27 | 28 => RecoveryId::from_i32(i32::from(recovery_raw - 27))?,
        0..=3 => RecoveryId::from_i32(i32::from(recovery_raw))?,
        _ => {
            return Err(anyhow!(
                "signature recovery id must be one of 0,1,2,3,27,28"
            ))
        }
    };

    let recoverable = RecoverableSignature::from_compact(&signature[0..64], recovery_id)?;
    let message = Message::from_digest_slice(&hash)?;
    let secp = Secp256k1::new();
    let public_key = secp.recover_ecdsa(&message, &recoverable)?;

    Ok(derive_address_from_public_key(&public_key))
}

pub fn derive_address_from_public_key(public_key: &PublicKey) -> String {
    let compressed = public_key.serialize();
    let mut hasher = Ripemd160::new();
    hasher.update(compressed);
    let wallet_hash = hex::encode_upper(hasher.finalize());
    format!("{ANET_ADDRESS_PREFIX}{}", &wallet_hash[..36])
}

fn default_tx_type() -> String {
    "transfer".to_owned()
}

fn default_payload() -> Value {
    Value::Object(serde_json::Map::new())
}

pub fn derive_address_from_seed(seed: &str) -> String {
    let private_key = hash_hex(seed);
    let public_key = hash_hex(&private_key);

    let mut hasher = Ripemd160::new();
    hasher.update(public_key.as_bytes());
    let wallet_hash = hex::encode_upper(hasher.finalize());
    format!("{ANET_ADDRESS_PREFIX}{}", &wallet_hash[..36])
}

/// Legacy-derivation address from a raw 32-byte private key. This is the
/// reverse of the chain that `derive_address_from_seed` performs: the
/// "private_key" intermediate there is `hex_lower(SHA256(seed))` — which
/// equals `hex_lower(privkey_bytes)` for any seed-derived key (where
/// privkey_bytes = SHA256(seed)). The downstream hash chain is identical.
/// Used by `/wallet/migrate-legacy/reveal` to prove that a given EVM
/// secp private key is the SAME key that controlled the legacy address.
pub fn derive_legacy_address_from_privkey_bytes(privkey: &[u8; 32]) -> String {
    let private_key_str = hex::encode(privkey); // lowercase hex
    let public_key_str = {
        let mut sha = Sha256::new();
        sha.update(private_key_str.as_bytes());
        hex::encode(sha.finalize())
    };
    let mut rip = Ripemd160::new();
    rip.update(public_key_str.as_bytes());
    let wallet_hash = hex::encode_upper(rip.finalize());
    format!("{ANET_ADDRESS_PREFIX}{}", &wallet_hash[..36])
}

fn hash_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}

pub fn is_valid_anet_wallet(address: &str) -> bool {
    address.len() == 40
        && address.starts_with(ANET_ADDRESS_PREFIX)
        && address[4..].bytes().all(|byte| byte.is_ascii_hexdigit())
        && address[4..].bytes().all(|byte| !byte.is_ascii_lowercase())
}

pub fn wallet_seed_matches(address: &str, seed: &str) -> bool {
    let normalized_address = address.trim().to_uppercase();
    if !is_valid_anet_wallet(&normalized_address) {
        return false;
    }

    derive_address_from_seed(seed.trim()) == normalized_address
}

#[cfg(test)]
mod tests {
    use super::{
        canonical_signing_bytes, derive_address_from_public_key, SignedTransactionRequest,
    };
    use chrono::Utc;
    use secp256k1::{Message, Secp256k1, SecretKey};
    use serde_json::json;
    use sha2::{Digest, Sha256};

    fn sign(hash_hex: &str, private_key_hex: &str) -> String {
        let secp = Secp256k1::new();
        let message =
            Message::from_digest_slice(&hex::decode(hash_hex).expect("hash hex")).expect("digest");
        let secret = SecretKey::from_slice(&hex::decode(private_key_hex).expect("secret hex"))
            .expect("secret");
        let sig = secp.sign_ecdsa_recoverable(&message, &secret);
        let (recovery_id, compact) = sig.serialize_compact();
        let mut out = Vec::from(compact);
        out.push(recovery_id.to_i32() as u8);
        hex::encode(out)
    }

    #[test]
    fn accepts_valid_signed_transaction() {
        let secp = Secp256k1::new();
        let secret_key = SecretKey::from_slice(
            &hex::decode("4f3edf983ac636a65a842ce7c78d9aa706d3b113bce036f7a4f6f7d6f7f6f7f6")
                .expect("secret"),
        )
        .expect("secret key");
        let pubkey = secp256k1::PublicKey::from_secret_key(&secp, &secret_key);
        let from = derive_address_from_public_key(&pubkey);
        let timestamp = Utc::now();
        let payload = json!({ "memo": "Payroll settlement" });
        let signing = canonical_signing_bytes(
            "transfer",
            &from,
            "ANET1234567890ABCDEF1234567890ABCDEF1234",
            100,
            1_000,
            1,
            timestamp,
            "anet-mainnet",
            &payload,
        )
        .expect("preimage");
        let hash = hex::encode(Sha256::digest(signing));
        let signature = sign(
            &hash,
            "4f3edf983ac636a65a842ce7c78d9aa706d3b113bce036f7a4f6f7d6f7f6f7f6",
        );

        let request = SignedTransactionRequest {
            tx_type: "transfer".to_owned(),
            from,
            to: "ANET1234567890ABCDEF1234567890ABCDEF1234".to_owned(),
            amount_ants: 100,
            fee_ants: 1_000,
            nonce: 1,
            timestamp,
            chain_id: "anet-mainnet".to_owned(),
            payload,
            signature,
            tx_hash: hash,
        };

        let transaction = request
            .into_transaction()
            .expect("transaction should validate");
        assert_eq!(transaction.amount_ants, 100);
        assert_eq!(transaction.nonce, 1);
        assert_eq!(transaction.memo, "Payroll settlement");
    }
}
