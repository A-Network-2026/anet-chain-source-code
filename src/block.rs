use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

use crate::{
    dex::DexPool, events::ChainEvent, token::Anrc20Token,
    transaction::{canonical_json_string, Transaction},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Block {
    pub block_height: u64,
    pub epoch_start: DateTime<Utc>,
    pub epoch_end: DateTime<Utc>,
    pub previous_hash: String,
    pub hash: String,
    pub transactions: Vec<Transaction>,
    pub activated_supply_ants: u64,
    pub total_fees_ants: u64,
    pub miners: Vec<String>,
    pub fee_per_miner: u64,
    pub block_event: Option<String>,
    #[serde(default)]
    pub events: Vec<ChainEvent>,
    #[serde(default)]
    pub dex_pools: HashMap<String, DexPool>,
    #[serde(default)]
    pub token_registry: HashMap<String, Anrc20Token>,
    #[serde(default)]
    pub tpow_proofs: Vec<serde_json::Value>,
    #[serde(default)]
    pub validator_votes: Vec<serde_json::Value>,
    #[serde(default)]
    pub state_root: String,
    #[serde(default)]
    pub proof_root: String,
}

#[derive(Debug, Clone, Serialize)]
struct BlockHashPayload<'a> {
    block_height: u64,
    epoch_start: DateTime<Utc>,
    epoch_end: DateTime<Utc>,
    previous_hash: &'a str,
    transactions: &'a [Transaction],
    activated_supply_ants: u64,
    total_fees_ants: u64,
    miners: &'a [String],
    fee_per_miner: u64,
    events: &'a [ChainEvent],
    dex_pools: &'a HashMap<String, DexPool>,
    token_registry: &'a HashMap<String, Anrc20Token>,
}

#[derive(Debug, Clone, Serialize)]
struct BlockHashPayloadLegacy<'a> {
    block_height: u64,
    epoch_start: DateTime<Utc>,
    epoch_end: DateTime<Utc>,
    previous_hash: &'a str,
    transactions: &'a [Transaction],
    activated_supply_ants: u64,
    total_fees_ants: u64,
    miners: &'a [String],
    fee_per_miner: u64,
}

#[derive(Debug, Clone, Serialize)]
struct LegacyTransactionHashPayload {
    from: String,
    to: String,
    amount_ants: u64,
    fee_ants: u64,
    #[serde(skip_serializing_if = "String::is_empty")]
    memo: String,
    timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
struct BlockHashPayloadLegacyTx {
    block_height: u64,
    epoch_start: DateTime<Utc>,
    epoch_end: DateTime<Utc>,
    previous_hash: String,
    transactions: Vec<LegacyTransactionHashPayload>,
    activated_supply_ants: u64,
    total_fees_ants: u64,
    miners: Vec<String>,
    fee_per_miner: u64,
}

impl Block {
    pub fn new(
        block_height: u64,
        epoch_start: DateTime<Utc>,
        epoch_end: DateTime<Utc>,
        previous_hash: String,
        transactions: Vec<Transaction>,
        activated_supply_ants: u64,
        miners: Vec<String>,
        block_event: Option<String>,
        events: Vec<ChainEvent>,
        dex_pools: HashMap<String, DexPool>,
        token_registry: HashMap<String, Anrc20Token>,
        tpow_proofs: Vec<serde_json::Value>,
        validator_votes: Vec<serde_json::Value>,
        state_root: String,
        proof_root: String,
    ) -> Result<Self> {
        if epoch_end <= epoch_start {
            anyhow::bail!("block epoch_end must be greater than epoch_start");
        }

        for transaction in &transactions {
            transaction.validate()?;
        }

        let total_fees_ants = transactions.iter().map(|tx| tx.fee_ants).sum();
        let fee_per_miner = if miners.is_empty() {
            0
        } else {
            total_fees_ants / miners.len() as u64
        };

        let mut block = Self {
            block_height,
            epoch_start,
            epoch_end,
            previous_hash,
            hash: String::new(),
            transactions,
            activated_supply_ants,
            total_fees_ants,
            miners,
            fee_per_miner,
            block_event,
            events,
            dex_pools,
            token_registry,
            tpow_proofs,
            validator_votes,
            state_root,
            proof_root,
        };
        block.hash = block.compute_hash()?;
        Ok(block)
    }

    /// v3 (canonical): sorted-key, recursive JSON. New blocks use this format.
    pub fn compute_hash(&self) -> Result<String> {
        let payload = BlockHashPayload {
            block_height: self.block_height,
            epoch_start: self.epoch_start,
            epoch_end: self.epoch_end,
            previous_hash: &self.previous_hash,
            transactions: &self.transactions,
            activated_supply_ants: self.activated_supply_ants,
            total_fees_ants: self.total_fees_ants,
            miners: &self.miners,
            fee_per_miner: self.fee_per_miner,
            events: &self.events,
            dex_pools: &self.dex_pools,
            token_registry: &self.token_registry,
        };
        // S1-1: canonical (sorted-key, recursive) JSON encoding. The previous
        // `serde_json::to_vec(&payload)` produced struct-declaration-ordered
        // keys, which is *not* a stable interchange canonicalization — two
        // nodes running different serde patch versions, or two structs with
        // different field declaration order across a rename, could disagree on
        // the hash of byte-identical block contents. Using the same
        // canonicalization the transaction signing layer uses guarantees that
        // every node hashes the same preimage forever.
        let value = serde_json::to_value(&payload)?;
        let preimage = canonical_json_string(&value)?;
        let mut hasher = Sha256::new();
        hasher.update(preimage.as_bytes());
        Ok(hex::encode(hasher.finalize()))
    }

    /// v2 (pre-S1-1): `serde_json::to_vec` on the full payload with events,
    /// dex_pools, token_registry. Kept ONLY for validating blocks that were
    /// produced by previous node versions before canonical hashing landed.
    /// New blocks are never produced with this format.
    fn compute_hash_v2_noncanonical(&self) -> Result<String> {
        let payload = BlockHashPayload {
            block_height: self.block_height,
            epoch_start: self.epoch_start,
            epoch_end: self.epoch_end,
            previous_hash: &self.previous_hash,
            transactions: &self.transactions,
            activated_supply_ants: self.activated_supply_ants,
            total_fees_ants: self.total_fees_ants,
            miners: &self.miners,
            fee_per_miner: self.fee_per_miner,
            events: &self.events,
            dex_pools: &self.dex_pools,
            token_registry: &self.token_registry,
        };
        let encoded = serde_json::to_vec(&payload)?;
        let mut hasher = Sha256::new();
        hasher.update(encoded);
        Ok(hex::encode(hasher.finalize()))
    }

    fn compute_hash_legacy(&self) -> Result<String> {
        let payload = BlockHashPayloadLegacy {
            block_height: self.block_height,
            epoch_start: self.epoch_start,
            epoch_end: self.epoch_end,
            previous_hash: &self.previous_hash,
            transactions: &self.transactions,
            activated_supply_ants: self.activated_supply_ants,
            total_fees_ants: self.total_fees_ants,
            miners: &self.miners,
            fee_per_miner: self.fee_per_miner,
        };
        let encoded = serde_json::to_vec(&payload)?;
        let mut hasher = Sha256::new();
        hasher.update(encoded);
        Ok(hex::encode(hasher.finalize()))
    }

    fn compute_hash_legacy_tx_shape(&self) -> Result<String> {
        let legacy_transactions = self
            .transactions
            .iter()
            .map(|tx| LegacyTransactionHashPayload {
                from: tx.from.clone(),
                to: tx.to.clone(),
                amount_ants: tx.amount_ants,
                fee_ants: tx.fee_ants,
                memo: tx.memo.clone(),
                timestamp: tx.timestamp,
            })
            .collect::<Vec<_>>();

        let payload = BlockHashPayloadLegacyTx {
            block_height: self.block_height,
            epoch_start: self.epoch_start,
            epoch_end: self.epoch_end,
            previous_hash: self.previous_hash.clone(),
            transactions: legacy_transactions,
            activated_supply_ants: self.activated_supply_ants,
            total_fees_ants: self.total_fees_ants,
            miners: self.miners.clone(),
            fee_per_miner: self.fee_per_miner,
        };

        let encoded = serde_json::to_vec(&payload)?;
        let mut hasher = Sha256::new();
        hasher.update(encoded);
        Ok(hex::encode(hasher.finalize()))
    }

    pub fn validate_hash(&self) -> Result<bool> {
        let new_hash = self.compute_hash()?;
        if self.hash == new_hash {
            return Ok(true);
        }

        // S1-5: operators bringing up a fresh deployment can opt out of the
        // legacy hash-format fallbacks by setting ANET_REJECT_LEGACY_HASHES=true.
        // Existing chains keep the relaxed behavior (default) so historical
        // blocks continue to validate; new chains can lock down their preimage
        // surface from day one.
        if reject_legacy_hashes() {
            return Ok(false);
        }

        // v2 non-canonical: blocks produced before S1-1 used the same payload
        // shape but `serde_json::to_vec` (declaration-ordered keys). Accept
        // these so existing chains keep replaying.
        let v2_hash = self.compute_hash_v2_noncanonical()?;
        if self.hash == v2_hash {
            tracing::warn!(
                "block {} validated using pre-canonical v2 hash; new blocks use canonical v3",
                self.block_height
            );
            return Ok(true);
        }

        // Try legacy hash computation for backward compatibility with pre-events blocks
        let legacy_hash = self.compute_hash_legacy()?;
        if self.hash == legacy_hash {
            tracing::warn!(
                "block {} validated using legacy hash format (pre-events); new deployments will use v2 format",
                self.block_height
            );
            return Ok(true);
        }

        // Oldest chains hashed transactions before signed fields existed on Transaction.
        let legacy_tx_shape_hash = self.compute_hash_legacy_tx_shape()?;
        if self.hash == legacy_tx_shape_hash {
            tracing::warn!(
                "block {} validated using legacy transaction hash shape; new deployments will use v2 format",
                self.block_height
            );
            return Ok(true);
        }

        Ok(false)
    }

    pub fn validate_structure(&self) -> Result<()> {
        if self.epoch_end <= self.epoch_start {
            anyhow::bail!(
                "stored block {} has invalid epoch window",
                self.block_height
            );
        }

        for transaction in &self.transactions {
            transaction.validate()?;
        }

        Ok(())
    }
}

fn reject_legacy_hashes() -> bool {
    std::env::var("ANET_REJECT_LEGACY_HASHES")
        .ok()
        .map(|value| {
            let normalized = value.trim().to_ascii_lowercase();
            matches!(
                normalized.as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}
