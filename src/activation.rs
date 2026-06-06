use std::{fs, path::Path};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::db;

pub const CHAIN_ID: &str = "anet-private-mainnet-1";
pub const ANTS_PER_SESSION: u64 = 4_882_812;
pub const ANTS_PER_ANET: u64 = 100_000_000;
pub const MIN_SESSIONS_FOR_ANET: u64 = 1_000;

/// Hard cap on the size of the active validator set.
///
/// Bitcoin-aligned design choice: small, deterministic, ungovernable.
/// 2,100 is chosen as a nod to Bitcoin's "21" magic number (21M supply
/// cap; 210,000 blocks per halving era). It is large enough for global
/// geographic decentralization while keeping per-block fee distribution
/// economically meaningful for each seated validator.
///
/// Selection when more than `MAX_VALIDATORS` accounts are eligible is
/// deterministic: sessions DESC, then address ASC (see
/// `state::compute_validators_from_accounts`).
pub const MAX_VALIDATORS: usize = 2_100;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenesisAccount {
    pub address: String,
    pub ants_balance: u64,
    pub sessions: u64,
    #[serde(default)]
    pub total_activated_ants: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenesisConfig {
    pub chain_id: String,
    pub genesis_time: DateTime<Utc>,
    pub accounts: Vec<GenesisAccount>,
}

pub async fn activate_genesis(path: &Path) -> Result<GenesisConfig> {
    let client = db::connect().await?;
    let ant_ledger_accounts = db::load_genesis_accounts(&client).await?;

    let accounts = ant_ledger_accounts
        .into_iter()
        .map(|row| GenesisAccount {
            address: row.wallet_address,
            ants_balance: row
                .ants_balance
                .max(row.sessions.saturating_mul(ANTS_PER_SESSION)),
            sessions: row.sessions,
            total_activated_ants: row
                .ants_balance
                .max(row.sessions.saturating_mul(ANTS_PER_SESSION)),
        })
        .collect();

    let genesis = GenesisConfig {
        chain_id: CHAIN_ID.to_owned(),
        genesis_time: Utc::now(),
        accounts,
    };

    write_genesis(path, &genesis)?;
    Ok(genesis)
}

pub fn load_genesis(path: &Path) -> Result<GenesisConfig> {
    let contents = fs::read_to_string(path)
        .with_context(|| format!("failed to read genesis file at {}", path.display()))?;
    let genesis = serde_json::from_str(&contents)
        .with_context(|| format!("failed to parse genesis file at {}", path.display()))?;
    Ok(genesis)
}

pub fn write_genesis(path: &Path, genesis: &GenesisConfig) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config directory {}", parent.display()))?;
    }

    let encoded = serde_json::to_string_pretty(genesis)?;
    fs::write(path, encoded)
        .with_context(|| format!("failed to write genesis file at {}", path.display()))?;
    Ok(())
}
