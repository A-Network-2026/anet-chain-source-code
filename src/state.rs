use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
};

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::{
    activation::{GenesisConfig, ANTS_PER_ANET},
    block::Block,
    db,
    dex::{self, DexLiquidityResult, DexPool, DexPoolView, DexQuoteView, DexSwapResult},
    events::ChainEvent,
    token::{Anrc20Token, Anrc20TokenView},
    transaction::Transaction,
};

pub const SYSTEM_FEE_RESERVE_ADDRESS: &str = "__tpow_fee_reserve__";
pub const WANET_SYMBOL: &str = "WANET";
const MAX_MEMPOOL_TRANSACTIONS: usize = 10_000;

/// Synthetic, valid-format ANET address used as the sender for durable EVM
/// bridge credits. It never holds real value: it is pre-funded in memory just
/// before a credit block seals, and auto-funded on replay by
/// `reconcile_replay_sender_balances`, so every treasury→recipient credit
/// transaction reconstructs deterministically and nets the treasury to zero.
pub const BRIDGE_TREASURY_ADDRESS: &str = "ANET000000000000000000000000000000000000";

/// Memo prefix that marks a block transaction as an EVM bridge credit. Used to
/// rebuild the idempotency dedup set from durable block history on startup.
pub const BRIDGE_CREDIT_MEMO_PREFIX: &str = "bridge:evm:";

pub type SharedState = Arc<RwLock<NodeState>>;

/// Derive the active validator set from on-chain account state alone.
///
/// Bitcoin-aligned (deterministic, no governance, no off-chain registry):
///   1. Any account with `sessions >= MIN_SESSIONS_FOR_ANET` is eligible.
///   2. Sort by `sessions` DESC (more proof-of-work history → higher seat
///      priority), then by `address` ASC for a deterministic tie-break.
///   3. Take the top `MAX_VALIDATORS` (= 2,100).
///
/// This function is referentially transparent — given the same accounts
/// map it always returns the same vector — so every node arrives at the
/// same validator set without consulting any external service.
pub fn compute_validators_from_accounts(
    accounts: &HashMap<String, AccountState>,
) -> Vec<String> {
    let mut eligible: Vec<&AccountState> = accounts
        .values()
        .filter(|a| a.sessions >= crate::activation::MIN_SESSIONS_FOR_ANET)
        .collect();
    eligible.sort_by(|a, b| {
        b.sessions
            .cmp(&a.sessions)
            .then_with(|| a.address.cmp(&b.address))
    });
    eligible
        .into_iter()
        .take(crate::activation::MAX_VALIDATORS)
        .map(|a| a.address.clone())
        .collect()
}

/// Minimum number of independent *organic* validators that must exist before
/// every operator-seeded bootstrap seat is automatically retired.
///
/// An organic validator is any account with `sessions >= MIN_SESSIONS_FOR_ANET`
/// that was **not** placed there by `seed_bootstrap_validators`. Until this many
/// organic validators are eligible, bootstrap seats are used only to *backfill*
/// the active set so block production never halts. The instant organic
/// validators reach this quorum, all bootstrap seats drop out of the active set
/// — no operator action, no governance vote. This mirrors Bitcoin's earliest
/// era ending automatically as independent miners came online (the `21` is a
/// deliberate nod to Bitcoin's recurring 21 motif and a sane decentralization
/// floor).
pub const BOOTSTRAP_SUNSET_ORGANIC_QUORUM: usize = 21;

/// Compute the active validator set with automatic, Bitcoin-style bootstrap
/// sunset.
///
/// This is the bootstrap-aware superset of [`compute_validators_from_accounts`]
/// and is the function the running node uses to seat validators. It is still
/// fully deterministic and referentially transparent: given the same accounts
/// map and the same bootstrap address list, every node computes the identical
/// vector without consulting any external service.
///
/// Rules:
///   1. Organic validators (eligible accounts that are *not* bootstrap seats)
///      always take seats first, ranked by sessions DESC then address ASC.
///   2. If at least [`BOOTSTRAP_SUNSET_ORGANIC_QUORUM`] organic validators are
///      eligible, bootstrap seats are retired entirely — the set is 100%
///      organic.
///   3. Otherwise, bootstrap seats backfill the remaining capacity (also ranked
///      deterministically) only up to `MAX_VALIDATORS`, keeping the chain live
///      during the earliest phase. As organic validators join, the number of
///      backfilled bootstrap seats shrinks automatically.
pub fn compute_active_validator_set(
    accounts: &HashMap<String, AccountState>,
    bootstrap: &[String],
) -> Vec<String> {
    let bootstrap_set: HashSet<&str> = bootstrap.iter().map(|s| s.as_str()).collect();

    let mut organic: Vec<&AccountState> = accounts
        .values()
        .filter(|a| {
            a.sessions >= crate::activation::MIN_SESSIONS_FOR_ANET
                && !bootstrap_set.contains(a.address.as_str())
        })
        .collect();
    organic.sort_by(|a, b| {
        b.sessions
            .cmp(&a.sessions)
            .then_with(|| a.address.cmp(&b.address))
    });

    // Auto-sunset: enough independent organic validators exist — retire all
    // bootstrap seats and run a fully organic set.
    if organic.len() >= BOOTSTRAP_SUNSET_ORGANIC_QUORUM {
        return organic
            .into_iter()
            .take(crate::activation::MAX_VALIDATORS)
            .map(|a| a.address.clone())
            .collect();
    }

    // Below quorum: organic first, then backfill remaining seats with bootstrap
    // so block production never halts during the bootstrap phase.
    let mut result: Vec<String> = organic
        .iter()
        .take(crate::activation::MAX_VALIDATORS)
        .map(|a| a.address.clone())
        .collect();

    if result.len() < crate::activation::MAX_VALIDATORS {
        let mut boot: Vec<&AccountState> = accounts
            .values()
            .filter(|a| {
                a.sessions >= crate::activation::MIN_SESSIONS_FOR_ANET
                    && bootstrap_set.contains(a.address.as_str())
            })
            .collect();
        boot.sort_by(|a, b| {
            b.sessions
                .cmp(&a.sessions)
                .then_with(|| a.address.cmp(&b.address))
        });
        for a in boot {
            if result.len() >= crate::activation::MAX_VALIDATORS {
                break;
            }
            result.push(a.address.clone());
        }
    }

    result
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountState {
    pub address: String,
    pub ants_balance: u64,
    pub activated_ants: u64,
    pub total_activated_ants: u64,
    pub sessions: u64,
    pub asset_balances: HashMap<String, u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AccountView {
    pub address: String,
    pub ants_balance: u64,
    pub anet_balance: String,
    pub sessions: u64,
    pub is_validator: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkSummary {
    pub chain_id: String,
    pub epoch_seconds: u64,
    pub total_ants: u64,
    pub total_anet: String,
    pub used_supply_history_fallback: bool,
    pub active_miners: usize,
    pub used_validator_history_fallback: bool,
    pub latest_block_height: Option<u64>,
    pub current_epoch_start: DateTime<Utc>,
    pub current_epoch_end: DateTime<Utc>,
    pub seconds_until_epoch_end: i64,
    pub mempool_depth: u64,
    pub pending_activated_supply_ants: u64,
    pub has_pending_block_work: bool,
    pub last_web2_sync_at: Option<DateTime<Utc>>,
    pub last_web2_sync_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ActivationSyncResult {
    pub accounts_updated: usize,
    pub credited_ants: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ValidatorHeartbeatView {
    pub wallet: String,
    pub endpoint: Option<String>,
    pub client_version: Option<String>,
    pub last_seen_at: DateTime<Utc>,
    pub is_eligible: bool,
}

/// One seat in the read-only validator directory served to the explorer.
#[derive(Debug, Clone, Serialize)]
pub struct ValidatorView {
    pub rank: usize,
    pub address: String,
    pub sessions: u64,
    pub anet_balance: String,
    pub ants_balance: u64,
    pub eligible: bool,
    pub is_bootstrap: bool,
    /// Share of total eligible session weight, in basis points (0–10000).
    pub voting_power_bps: u32,
    pub online: bool,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub endpoint: Option<String>,
    pub client_version: Option<String>,
}

/// Aggregate view of the active validator set for the public explorer.
#[derive(Debug, Clone, Serialize)]
pub struct ValidatorDirectory {
    pub threshold_sessions: u64,
    pub max_validators: usize,
    pub total_eligible: usize,
    pub bootstrap_count: usize,
    pub organic_count: usize,
    pub online_count: usize,
    pub total_session_weight: u64,
    /// Number of independent organic validators required before all bootstrap
    /// seats are automatically retired.
    pub sunset_quorum: usize,
    /// True once the network is running fully organic — every operator-seeded
    /// bootstrap validator has been auto-retired from the active set.
    pub bootstrap_retired: bool,
    pub validators: Vec<ValidatorView>,
}

#[derive(Debug, Clone)]
pub struct ValidatorHeartbeatState {
    pub endpoint: Option<String>,
    pub client_version: Option<String>,
    pub last_seen_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingMiningProof {
    pub miner: String,
    pub proof_hash: String,
    pub difficulty: u32,
    pub submitted_at: DateTime<Utc>,
}

#[derive(Debug)]
pub struct NodeState {
    pub chain_id: String,
    pub genesis_time: DateTime<Utc>,
    pub accounts: HashMap<String, AccountState>,
    pub eligible_miners: Vec<String>,
    pub mempool: Vec<Transaction>,
    pub account_nonces: HashMap<String, u64>,
    pub blocks: Vec<Block>,
    pub chain_db: Arc<tokio_postgres::Client>,
    pub pending_activated_supply_ants: u64,
    pub pending_state_commit: bool,
    pub pending_block_event: Option<String>,
    pub pending_events: Vec<ChainEvent>,
    pub dex_pools: HashMap<String, DexPool>,
    pub token_registry: HashMap<String, Anrc20Token>,
    pub genesis_path: PathBuf,
    pub epoch_seconds: u64,
    pub last_web2_sync_at: Option<DateTime<Utc>>,
    pub last_web2_sync_error: Option<String>,
    pub validator_heartbeats: HashMap<String, ValidatorHeartbeatState>,
    /// Operator-seeded bootstrap (Satoshi-mode) validator addresses. Tracked
    /// only so the read-only explorer can label seats as bootstrap vs organic.
    /// Has no effect on consensus or validator selection (which is derived
    /// purely from on-chain session counts).
    pub bootstrap_validators: Vec<String>,
    pub pending_mining_proofs: Vec<PendingMiningProof>,
    /// In-memory dedup set for EVM bridge credits — prevents double-crediting the
    /// same BSC tx hash within a server session. Populated in post_admin_bridge_evm_credit.
    /// NOTE: cleared on restart; the pi-backend `processed` flag provides the primary
    /// persistence-layer guard across restarts.
    pub processed_evm_bridge_hashes: HashSet<String>,
    /// In-memory dedup set for EVM wallet activity events (send/swap) — prevents
    /// duplicate block events for the same BSC tx hash within a server session.
    pub processed_evm_activity_hashes: HashSet<String>,
    /// Addresses that have received at least one EVM bridge credit. These
    /// holders paid real value (USDC/BNB) for their ANET, so — unlike mined
    /// ANTS — their balance is NOT subject to the `MIN_SESSIONS_FOR_ANET`
    /// activation gate that guards spending/transfer/DEX. Rebuilt durably on
    /// startup from block memos (`bridge:evm:<hash>`) via
    /// `collect_bridge_funded_accounts`, so the exemption survives restarts.
    pub bridge_funded_accounts: HashSet<String>,
}

impl NodeState {
    pub async fn from_genesis(
        genesis: GenesisConfig,
        genesis_path: PathBuf,
        _chain_path: PathBuf,
        epoch_seconds: u64,
    ) -> Result<Self> {
        let mut accounts = HashMap::new();
        for account in genesis.accounts {
            let total_activated_ants = account.total_activated_ants.max(account.ants_balance);
            accounts.insert(
                account.address.clone(),
                AccountState {
                    address: account.address,
                    ants_balance: account.ants_balance,
                    activated_ants: account.ants_balance,
                    total_activated_ants,
                    sessions: account.sessions,
                    asset_balances: HashMap::new(),
                },
            );
        }

        let chain_db = db::connect().await?;
        db::ensure_chain_initialized(chain_db.as_ref(), &genesis.chain_id, genesis.genesis_time)
            .await?;
        let blocks = db::load_chain_blocks(chain_db.as_ref()).await?;
        validate_block_sequence(&blocks)?;
        // Validator set is computed on-chain from session counts. No external
        // registry, no governance — Bitcoin-style: the chain alone is the
        // source of truth for who validates.
        let eligible_miners = compute_validators_from_accounts(&accounts);

        replay_blocks(&mut accounts, &blocks)?;
        let account_nonces = rebuild_account_nonces(&blocks)?;
        let dex_pools = rebuild_dex_pools_from_blocks(&blocks);
        let token_registry = rebuild_token_registry_from_blocks(&blocks);
        // Reconstruct the EVM bridge dedup set from durable block history so a
        // restart cannot allow the same BSC tx hash to be credited twice.
        let processed_evm_bridge_hashes = collect_processed_bridge_hashes(&blocks);
        // Reconstruct the set of bridge-funded recipients so the
        // session-gate exemption for real (paid-for) ANET survives restarts.
        let bridge_funded_accounts = collect_bridge_funded_accounts(&blocks);

        Ok(Self {
            chain_id: genesis.chain_id,
            genesis_time: genesis.genesis_time,
            accounts,
            eligible_miners,
            mempool: Vec::new(),
            account_nonces,
            blocks,
            chain_db,
            pending_activated_supply_ants: 0,
            pending_state_commit: false,
            pending_block_event: None,
            pending_events: Vec::new(),
            dex_pools,
            token_registry,
            genesis_path,
            epoch_seconds,
            last_web2_sync_at: None,
            last_web2_sync_error: None,
            validator_heartbeats: HashMap::new(),
            bootstrap_validators: Vec::new(),
            pending_mining_proofs: Vec::new(),
            processed_evm_bridge_hashes,
            processed_evm_activity_hashes: HashSet::new(),
            bridge_funded_accounts,
        })
    }

    pub fn mark_web2_sync_success(&mut self) {
        self.last_web2_sync_at = Some(Utc::now());
        self.last_web2_sync_error = None;
    }

    pub fn mark_web2_sync_failure(&mut self, error: impl Into<String>) {
        self.last_web2_sync_error = Some(error.into());
    }

    /// Accept an externally-supplied validator candidate list (currently
    /// from the pi-backend sync). The list itself is treated as advisory:
    /// it is used only to materialize stub accounts for addresses the L1
    /// has not seen yet, so that on-chain session counts can later be
    /// applied to them. The authoritative validator set is then re-derived
    /// from on-chain account state via `compute_validators_from_accounts`.
    ///
    /// This makes the validator set Bitcoin-style autonomous: pi-backend
    /// cannot promote, demote, or override which accounts are validating.
    /// Only on-chain sessions (≥ `MIN_SESSIONS_FOR_ANET`) decide.
    pub fn replace_validators(&mut self, validators: Vec<String>) {
        let mut validators = validators;
        validators.sort();
        validators.dedup();
        for validator in &validators {
            self.accounts
                .entry(validator.clone())
                .or_insert(AccountState {
                    address: validator.clone(),
                    ants_balance: 0,
                    activated_ants: 0,
                    total_activated_ants: 0,
                    sessions: 0,
                    asset_balances: HashMap::new(),
                });
        }
        // On-chain truth wins. The `validators` argument is informational.
        self.eligible_miners =
            compute_active_validator_set(&self.accounts, &self.bootstrap_validators);
    }

    /// Seed bootstrap validators (Satoshi-mode).
    ///
    /// During the first phase of the chain — before any organic account has
    /// crossed `MIN_SESSIONS_FOR_ANET` — block production would halt because
    /// the deterministic validator-selection rule would return an empty set.
    ///
    /// To mirror Bitcoin's earliest era (Satoshi mining the first ~14k blocks
    /// largely alone, with no governance vote to enable him), this method
    /// admits a small operator-controlled set of addresses into the on-chain
    /// accounts map at exactly `MIN_SESSIONS_FOR_ANET` sessions and records
    /// them as bootstrap seats. They keep the chain producing blocks during the
    /// earliest phase via the backfill rule in [`compute_active_validator_set`].
    ///
    /// Sunset is fully automatic and requires no operator action: organic
    /// validators always outrank bootstrap seats, so the number of seated
    /// bootstrap validators shrinks as the network grows, and once
    /// [`BOOTSTRAP_SUNSET_ORGANIC_QUORUM`] independent organic validators are
    /// eligible, every bootstrap seat is retired from the active set at once.
    ///
    /// This call is idempotent and never decreases an existing session count,
    /// so re-running it on every periodic Web2 sync is safe.
    pub fn seed_bootstrap_validators(&mut self, addresses: &[String]) -> usize {
        let mut seeded = 0_usize;
        for raw in addresses {
            let normalized = raw.trim().to_uppercase();
            if normalized.is_empty() {
                continue;
            }
            // Remember the seat as bootstrap so the explorer can label it.
            // This is display-only metadata; it never alters selection.
            if !self.bootstrap_validators.contains(&normalized) {
                self.bootstrap_validators.push(normalized.clone());
            }
            let entry = self
                .accounts
                .entry(normalized.clone())
                .or_insert(AccountState {
                    address: normalized.clone(),
                    ants_balance: 0,
                    activated_ants: 0,
                    total_activated_ants: 0,
                    sessions: 0,
                    asset_balances: HashMap::new(),
                });
            if entry.sessions < crate::activation::MIN_SESSIONS_FOR_ANET {
                entry.sessions = crate::activation::MIN_SESSIONS_FOR_ANET;
                seeded += 1;
            }
        }
        if seeded > 0 {
            self.eligible_miners =
                compute_active_validator_set(&self.accounts, &self.bootstrap_validators);
        }
        seeded
    }

    pub fn record_validator_heartbeat(
        &mut self,
        wallet: String,
        endpoint: Option<String>,
        client_version: Option<String>,
    ) {
        let normalized_wallet = wallet.trim().to_uppercase();
        if normalized_wallet.is_empty() {
            return;
        }

        self.validator_heartbeats.insert(
            normalized_wallet,
            ValidatorHeartbeatState {
                endpoint: endpoint
                    .map(|value| value.trim().to_owned())
                    .filter(|value| !value.is_empty()),
                client_version: client_version
                    .map(|value| value.trim().to_owned())
                    .filter(|value| !value.is_empty()),
                last_seen_at: Utc::now(),
            },
        );
    }

    pub fn validator_heartbeat_views(&self, max_age_seconds: i64) -> Vec<ValidatorHeartbeatView> {
        let cutoff = Utc::now() - chrono::Duration::seconds(max_age_seconds.max(1));
        let mut entries = self
            .validator_heartbeats
            .iter()
            .filter(|(_, heartbeat)| heartbeat.last_seen_at >= cutoff)
            .map(|(wallet, heartbeat)| ValidatorHeartbeatView {
                wallet: wallet.clone(),
                endpoint: heartbeat.endpoint.clone(),
                client_version: heartbeat.client_version.clone(),
                last_seen_at: heartbeat.last_seen_at,
                is_eligible: self.eligible_miners.iter().any(|miner| miner == wallet),
            })
            .collect::<Vec<_>>();

        entries.sort_by(|left, right| right.last_seen_at.cmp(&left.last_seen_at));
        entries
    }

    /// Build a rich, read-only directory of the active validator set for the
    /// public explorer. Combines the deterministically-derived eligible set
    /// (sessions DESC) with live heartbeat status and a session-weight voting
    /// power share. Pure read — never mutates state or touches consensus.
    pub fn validator_directory(&self, online_window_seconds: i64) -> ValidatorDirectory {
        let cutoff = Utc::now() - chrono::Duration::seconds(online_window_seconds.max(1));
        let total_session_weight: u64 = self
            .eligible_miners
            .iter()
            .filter_map(|addr| self.accounts.get(addr))
            .map(|account| account.sessions)
            .sum();

        let mut validators = Vec::with_capacity(self.eligible_miners.len());
        let mut bootstrap_count = 0_usize;
        let mut online_count = 0_usize;

        for (idx, addr) in self.eligible_miners.iter().enumerate() {
            let account = self.accounts.get(addr);
            let sessions = account.map(|a| a.sessions).unwrap_or(0);
            let ants_balance = account.map(|a| a.ants_balance).unwrap_or(0);
            let is_bootstrap = self.bootstrap_validators.iter().any(|b| b == addr);
            if is_bootstrap {
                bootstrap_count += 1;
            }
            let heartbeat = self.validator_heartbeats.get(addr);
            let online = heartbeat
                .map(|h| h.last_seen_at >= cutoff)
                .unwrap_or(false);
            if online {
                online_count += 1;
            }
            let voting_power_bps = if total_session_weight > 0 {
                ((sessions as u128 * 10_000u128) / total_session_weight as u128) as u32
            } else {
                0
            };

            validators.push(ValidatorView {
                rank: idx + 1,
                address: addr.clone(),
                sessions,
                anet_balance: format_anet_fixed(ants_balance),
                ants_balance,
                eligible: true,
                is_bootstrap,
                voting_power_bps,
                online,
                last_seen_at: heartbeat.map(|h| h.last_seen_at),
                endpoint: heartbeat.and_then(|h| h.endpoint.clone()),
                client_version: heartbeat.and_then(|h| h.client_version.clone()),
            });
        }

        let total_eligible = validators.len();
        let organic_count = total_eligible.saturating_sub(bootstrap_count);
        ValidatorDirectory {
            threshold_sessions: crate::activation::MIN_SESSIONS_FOR_ANET,
            max_validators: crate::activation::MAX_VALIDATORS,
            total_eligible,
            bootstrap_count,
            organic_count,
            online_count,
            total_session_weight,
            sunset_quorum: BOOTSTRAP_SUNSET_ORGANIC_QUORUM,
            bootstrap_retired: bootstrap_count == 0
                && !self.bootstrap_validators.is_empty(),
            validators,
        }
    }

    pub fn record_mining_proof(&mut self, miner: String, proof_hash: String, difficulty: u32) {
        let normalized_miner = miner.trim().to_uppercase();
        if normalized_miner.is_empty() {
            return;
        }

        self.pending_mining_proofs.push(PendingMiningProof {
            miner: normalized_miner,
            proof_hash: proof_hash.trim().to_ascii_lowercase(),
            difficulty,
            submitted_at: Utc::now(),
        });
    }

    pub fn pending_proofs_for_block(&mut self, max_age_seconds: i64) -> Vec<serde_json::Value> {
        let cutoff = Utc::now() - chrono::Duration::seconds(max_age_seconds.max(60));
        let mut proofs = Vec::new();

        let remaining: Vec<_> = self
            .pending_mining_proofs
            .drain(..)
            .filter(|proof| proof.submitted_at >= cutoff)
            .collect();

        for proof in remaining.iter() {
            proofs.push(serde_json::json!({
                "miner": proof.miner,
                "proof_hash": proof.proof_hash,
                "difficulty": proof.difficulty,
                "submitted_at": proof.submitted_at.to_rfc3339(),
            }));
        }

        self.pending_mining_proofs = remaining;
        proofs
    }

    pub fn sync_activated_accounts(
        &mut self,
        activated_accounts: Vec<crate::activation::GenesisAccount>,
    ) -> Result<ActivationSyncResult> {
        let mut accounts_updated = 0_usize;
        let mut credited_ants = 0_u64;
        let mut removed_ants = 0_u64;
        let mut metadata_changed = false;

        let mut active_addresses = std::collections::HashSet::new();

        for activated in activated_accounts {
            active_addresses.insert(activated.address.clone());
            let entry = self
                .accounts
                .entry(activated.address.clone())
                .or_insert(AccountState {
                    address: activated.address.clone(),
                    ants_balance: 0,
                    activated_ants: 0,
                    total_activated_ants: 0,
                    sessions: 0,
                    asset_balances: HashMap::new(),
                });

            if activated.sessions > entry.sessions {
                entry.sessions = activated.sessions;
                metadata_changed = true;
            }

            if activated.ants_balance > entry.total_activated_ants {
                let delta = activated
                    .ants_balance
                    .checked_sub(entry.total_activated_ants)
                    .ok_or_else(|| anyhow!("activation delta underflowed"))?;
                entry.ants_balance = entry
                    .ants_balance
                    .checked_add(delta)
                    .ok_or_else(|| anyhow!("account activation overflowed"))?;
                entry.activated_ants = entry
                    .activated_ants
                    .checked_add(delta)
                    .ok_or_else(|| anyhow!("account activation remainder overflowed"))?;
                entry.total_activated_ants = activated.ants_balance;
                credited_ants = credited_ants
                    .checked_add(delta)
                    .ok_or_else(|| anyhow!("credited activation total overflowed"))?;
                self.pending_activated_supply_ants = self
                    .pending_activated_supply_ants
                    .checked_add(delta)
                    .ok_or_else(|| anyhow!("pending activation total overflowed"))?;
                accounts_updated += 1;
                metadata_changed = true;
            }
        }

        for entry in self.accounts.values_mut() {
            if entry.sessions < crate::activation::MIN_SESSIONS_FOR_ANET
                && entry.activated_ants > 0
                && !active_addresses.contains(&entry.address)
            {
                entry.ants_balance = entry
                    .ants_balance
                    .checked_sub(entry.activated_ants)
                    .ok_or_else(|| anyhow!("retroactive activation clawback underflowed"))?;
                removed_ants = removed_ants
                    .checked_add(entry.activated_ants)
                    .ok_or_else(|| anyhow!("removed activation total overflowed"))?;
                entry.activated_ants = 0;
                entry.total_activated_ants = 0;
                metadata_changed = true;
            }
        }

        if metadata_changed {
            self.persist_genesis_snapshot()?;
        }

        // Re-derive validator set whenever session counts may have changed.
        // Bitcoin-style: validation eligibility is an automatic, deterministic
        // function of on-chain state; any account that just crossed
        // `MIN_SESSIONS_FOR_ANET` becomes a validator on the next block, and
        // any account that fell below (e.g. via clawback above) drops out.
        // Bootstrap seats auto-sunset once enough organic validators exist.
        self.eligible_miners =
            compute_active_validator_set(&self.accounts, &self.bootstrap_validators);

        if removed_ants > 0 {
            tracing::info!(
                accounts_updated = accounts_updated,
                credited_ants = credited_ants,
                removed_ants = removed_ants,
                min_sessions = crate::activation::MIN_SESSIONS_FOR_ANET,
                "removed legacy Web2-derived ANET from under-threshold wallets"
            );
        }

        Ok(ActivationSyncResult {
            accounts_updated,
            credited_ants,
        })
    }

    pub fn all_blocks(&self) -> Vec<Block> {
        self.blocks.clone()
    }

    pub fn latest_blocks(&self, limit: usize) -> Vec<Block> {
        self.blocks.iter().rev().take(limit).cloned().collect()
    }

    pub fn block_by_id(&self, id: &str) -> Option<Block> {
        if let Ok(height) = id.parse::<u64>() {
            return self
                .blocks
                .iter()
                .find(|block| block.block_height == height)
                .cloned();
        }

        self.blocks.iter().find(|block| block.hash == id).cloned()
    }

    pub fn account_view(&self, address: &str) -> Option<AccountView> {
        self.accounts.get(address).map(|account| AccountView {
            address: account.address.clone(),
            ants_balance: account.ants_balance,
            anet_balance: format_anet_fixed(account.ants_balance),
            sessions: account.sessions,
            is_validator: self.eligible_miners.iter().any(|miner| miner == address),
        })
    }

    pub fn network_summary(&self) -> NetworkSummary {
        let now = Utc::now();
        let (current_epoch_start, current_epoch_end) =
            crate::consensus::current_epoch_window(now, self.epoch_seconds);
        let accounts_total_ants: u64 = self
            .accounts
            .values()
            .map(|account| account.ants_balance)
            .sum();
        let chain_activated_ants: u64 = self
            .blocks
            .iter()
            .map(|block| block.activated_supply_ants)
            .sum();
        // If runtime account state is empty after restart/sync drift, preserve a stable
        // supply display from finalized block history instead of showing a false zero.
        let total_ants = if accounts_total_ants == 0 && chain_activated_ants > 0 {
            chain_activated_ants
        } else {
            accounts_total_ants
        };
        let used_supply_history_fallback = accounts_total_ants == 0 && chain_activated_ants > 0;
        let active_miners = if self.eligible_miners.is_empty() {
            self.blocks
                .last()
                .map(|block| block.miners.len())
                .unwrap_or(0)
        } else {
            self.eligible_miners.len()
        };
        let used_validator_history_fallback = self.eligible_miners.is_empty()
            && self
                .blocks
                .last()
                .map(|block| !block.miners.is_empty())
                .unwrap_or(false);
        let has_pending_block_work = self.has_pending_block_work();

        NetworkSummary {
            chain_id: self.chain_id.clone(),
            epoch_seconds: self.epoch_seconds,
            total_ants,
            total_anet: format_anet_fixed(total_ants),
            used_supply_history_fallback,
            active_miners,
            used_validator_history_fallback,
            latest_block_height: self.blocks.last().map(|block| block.block_height),
            current_epoch_start,
            current_epoch_end,
            seconds_until_epoch_end: (current_epoch_end - now).num_seconds().max(0),
            mempool_depth: self.mempool.len() as u64,
            pending_activated_supply_ants: self.pending_activated_supply_ants,
            has_pending_block_work,
            last_web2_sync_at: self.last_web2_sync_at,
            last_web2_sync_error: self.last_web2_sync_error.clone(),
        }
    }

    pub fn queue_transaction(&mut self, transaction: Transaction) -> Result<String> {
        transaction.validate_signed_for_chain(&self.chain_id)?;

        if self.mempool.len() >= MAX_MEMPOOL_TRANSACTIONS {
            return Err(anyhow!("mempool is full"));
        }

        // Bridge-funded holders paid real value for their ANET, so their
        // balance is exempt from the mined-ANTS activation gate on both the
        // send and receive side.
        let sender_bridge_funded = self.bridge_funded_accounts.contains(&transaction.from);
        let recipient_bridge_funded = self.bridge_funded_accounts.contains(&transaction.to);

        let sender = self
            .accounts
            .get(&transaction.from)
            .ok_or_else(|| anyhow!("sender account not found in state"))?;

        if !Self::allow_ineligible_wallet_test_mode()
            && !sender_bridge_funded
            && sender.sessions < crate::activation::MIN_SESSIONS_FOR_ANET
        {
            return Err(anyhow!(
                "sender must complete at least {} Web2 sessions before spending mined ANTS/ANET or sending P2P",
                crate::activation::MIN_SESSIONS_FOR_ANET
            ));
        }

        let recipient = self.accounts.get(&transaction.to).ok_or_else(|| {
            anyhow!("recipient must complete at least 1000 Web2 sessions before receiving ANET")
        })?;

        if !Self::allow_ineligible_wallet_test_mode()
            && !recipient_bridge_funded
            && recipient.sessions < crate::activation::MIN_SESSIONS_FOR_ANET
        {
            return Err(anyhow!(
                "recipient must complete at least {} Web2 sessions before participating in P2P ANET transfers",
                crate::activation::MIN_SESSIONS_FOR_ANET
            ));
        }

        let committed_nonce = self
            .account_nonces
            .get(&transaction.from)
            .copied()
            .unwrap_or(0);
        if transaction.nonce <= committed_nonce {
            return Err(anyhow!(
                "stale nonce: expected nonce greater than {}",
                committed_nonce
            ));
        }

        let mut pending_nonces = self
            .mempool
            .iter()
            .filter(|tx| tx.from == transaction.from)
            .map(|tx| tx.nonce)
            .collect::<Vec<_>>();
        pending_nonces.sort_unstable();

        if pending_nonces
            .iter()
            .any(|nonce| *nonce == transaction.nonce)
        {
            return Err(anyhow!("duplicate nonce already queued for sender"));
        }

        let mut expected_nonce = committed_nonce
            .checked_add(1)
            .ok_or_else(|| anyhow!("nonce overflowed"))?;
        for pending in pending_nonces {
            if pending == expected_nonce {
                expected_nonce = expected_nonce
                    .checked_add(1)
                    .ok_or_else(|| anyhow!("nonce overflowed"))?;
            } else if pending > expected_nonce {
                break;
            }
        }

        if transaction.nonce != expected_nonce {
            return Err(anyhow!(
                "invalid nonce: expected {}, got {}",
                expected_nonce,
                transaction.nonce
            ));
        }

        let transaction_id = transaction.id()?;
        if self
            .mempool
            .iter()
            .any(|queued| queued.id().map(|id| id == transaction_id).unwrap_or(false))
        {
            return Err(anyhow!("duplicate transaction already queued"));
        }

        let pending_outgoing = self
            .mempool
            .iter()
            .filter(|tx| tx.from == transaction.from)
            .try_fold(0_u64, |sum, tx| {
                sum.checked_add(tx.total_debit()?)
                    .ok_or_else(|| anyhow!("pending debit overflowed"))
            })?;

        let required = transaction.total_debit()?;
        let available = sender
            .ants_balance
            .checked_sub(pending_outgoing)
            .ok_or_else(|| anyhow!("sender has no spendable balance left"))?;

        if available < required {
            return Err(anyhow!("insufficient ANTS balance for transaction and fee"));
        }

        self.mempool.push(transaction);
        Ok(transaction_id)
    }

    pub fn has_pending_block_work(&self) -> bool {
        !self.mempool.is_empty()
            || self.pending_activated_supply_ants > 0
            || self.pending_state_commit
            || !self.pending_mining_proofs.is_empty()
    }

    pub fn dex_pool_list(&self) -> Vec<DexPoolView> {
        let mut pools = self
            .dex_pools
            .values()
            .map(DexPool::view)
            .collect::<Vec<_>>();
        pools.sort_by(|left, right| left.pair_id.cmp(&right.pair_id));
        pools
    }

    pub fn dex_pool_view(&self, token_symbol: &str) -> Result<Option<DexPoolView>> {
        let key = dex::pool_key(token_symbol)?;
        Ok(self.dex_pools.get(&key).map(DexPool::view))
    }

    pub fn dex_mint_test_asset(
        &mut self,
        address: &str,
        token_symbol: &str,
        amount: u64,
    ) -> Result<u64> {
        if amount == 0 {
            return Err(anyhow!("mint amount must be greater than zero"));
        }

        let symbol = dex::normalize_token_symbol(token_symbol)?;
        let normalized = address.trim().to_uppercase();
        let account = self
            .accounts
            .entry(normalized.clone())
            .or_insert_with(|| AccountState {
                address: normalized,
                ants_balance: 0,
                activated_ants: 0,
                total_activated_ants: 0,
                sessions: 0,
                asset_balances: HashMap::new(),
            });

        let new_balance = account
            .asset_balances
            .get(&symbol)
            .copied()
            .unwrap_or(0)
            .checked_add(amount)
            .ok_or_else(|| anyhow!("asset balance overflowed"))?;
        account.asset_balances.insert(symbol, new_balance);
        self.pending_block_event = Some("DEX: Mint Test Asset".to_string());
        self.pending_state_commit = true;
        Ok(new_balance)
    }

    pub fn admin_credit_anet_test(&mut self, address: &str, amount_ants: u64) -> Result<u64> {
        if amount_ants == 0 {
            return Err(anyhow!("mint amount must be greater than zero"));
        }

        let normalized = address.trim().to_uppercase();
        let account = self
            .accounts
            .entry(normalized.clone())
            .or_insert_with(|| AccountState {
                address: normalized,
                ants_balance: 0,
                activated_ants: 0,
                total_activated_ants: 0,
                sessions: 0,
                asset_balances: HashMap::new(),
            });

        let new_balance = account
            .ants_balance
            .checked_add(amount_ants)
            .ok_or_else(|| anyhow!("ANET balance overflowed"))?;
        account.ants_balance = new_balance;
        self.pending_block_event = Some("Admin: Credit ANET Test".to_string());
        self.pending_state_commit = true;
        Ok(new_balance)
    }

    /// Durably credit a bridged EVM swap to `recipient` by encoding it as a
    /// real block transaction (a treasury → recipient transfer) rather than a
    /// bare in-memory balance bump.
    ///
    /// The legacy `admin_credit_anet_test` path only mutated `accounts` in RAM
    /// and stamped a cosmetic block-event label; because the credit was never a
    /// block transaction, `replay_blocks` could not reconstruct it and the
    /// balance evaporated on every node restart/redeploy. This path instead
    /// appends a transaction to the block, which is persisted to Postgres and
    /// re-applied on replay, so the credit is permanent.
    ///
    /// The synthetic sender (`BRIDGE_TREASURY_ADDRESS`) is pre-funded in memory
    /// here so the next live `apply_block` debit cannot underflow; on replay the
    /// identical top-up is reproduced by `reconcile_replay_sender_balances`,
    /// keeping live and replay state byte-identical and netting the treasury to
    /// zero. Returns the recipient's projected ANTS balance once the next block
    /// seals.
    pub fn queue_bridge_credit(
        &mut self,
        recipient: &str,
        amount_ants: u64,
        evm_tx_hash: &str,
    ) -> Result<u64> {
        if amount_ants == 0 {
            return Err(anyhow!("bridge credit amount must be greater than zero"));
        }

        let recipient_norm = recipient.trim().to_uppercase();
        if !crate::transaction::is_valid_anet_wallet(&recipient_norm) {
            return Err(anyhow!("bridge credit recipient must be a valid ANET wallet"));
        }
        if recipient_norm == BRIDGE_TREASURY_ADDRESS {
            return Err(anyhow!("bridge credit recipient must not be the treasury"));
        }

        let fee_ants = crate::transaction::MIN_FEE_ANTS;
        let total_debit = amount_ants
            .checked_add(fee_ants)
            .ok_or_else(|| anyhow!("bridge credit total overflowed"))?;

        // Pre-fund the in-memory treasury so the next live `apply_block` can
        // debit it without underflowing. On replay this top-up is reproduced by
        // `reconcile_replay_sender_balances`, so live and replay stay identical.
        let treasury = self
            .accounts
            .entry(BRIDGE_TREASURY_ADDRESS.to_owned())
            .or_insert_with(|| empty_account_state(BRIDGE_TREASURY_ADDRESS.to_owned()));
        treasury.ants_balance = treasury
            .ants_balance
            .checked_add(total_debit)
            .ok_or_else(|| anyhow!("bridge treasury balance overflowed"))?;

        let memo = format!("{BRIDGE_CREDIT_MEMO_PREFIX}{}", evm_tx_hash.trim().to_lowercase());

        // A legacy (unsigned) transaction: `Transaction::validate` accepts it on
        // the strength of `validate_basic` alone, and `rebuild_account_nonces`
        // skips `nonce == 0`, so repeated treasury credits never break nonce
        // ordering for any real wallet.
        let mint_tx = Transaction {
            tx_type: "transfer".to_owned(),
            from: BRIDGE_TREASURY_ADDRESS.to_owned(),
            to: recipient_norm.clone(),
            amount_ants,
            fee_ants,
            nonce: 0,
            memo,
            timestamp: Utc::now(),
            chain_id: self.chain_id.clone(),
            payload: serde_json::Value::Object(serde_json::Map::new()),
            signature: String::new(),
            tx_hash: String::new(),
        };
        self.mempool.push(mint_tx);

        self.pending_block_event = Some(format!(
            "EVM Bridge Credit: {amount_ants} ANTS → {recipient_norm}"
        ));
        self.pending_state_commit = true;

        // Mark the recipient as bridge-funded so their paid-for ANET is exempt
        // from the mined-ANTS activation gate (they can swap/transfer it even
        // with fewer than MIN_SESSIONS_FOR_ANET sessions).
        self.bridge_funded_accounts.insert(recipient_norm.clone());

        let projected = self
            .accounts
            .get(&recipient_norm)
            .map(|account| account.ants_balance)
            .unwrap_or(0)
            .saturating_add(amount_ants);
        Ok(projected)
    }

    /// Permanently burn `amount_ants` from `address` for an L1 → BSC
    /// bridge transfer. The ants disappear from L1 supply entirely;
    /// the relayer is responsible for releasing the equivalent wANET
    /// (or USDC/USDT) on BSC. Returns the new L1 ants balance.
    ///
    /// Validator fee (Bitcoin-style, sender-pays-on-top):
    /// in addition to `amount_ants`, the sender is charged
    /// `MIN_FEE_ANTS` (1,000 ANTS) which is distributed equally to the
    /// current active validator set. The bridged amount stays clean —
    /// L1 ants destroyed equals wANET minted on BSC, exactly — so the
    /// fee never reduces the user's bridged value.
    pub fn bridge_burn_anet(&mut self, address: &str, amount_ants: u64) -> Result<u64> {
        if amount_ants == 0 {
            return Err(anyhow!("burn amount must be greater than zero"));
        }

        let fee_ants = crate::transaction::MIN_FEE_ANTS;
        let total_debit = amount_ants
            .checked_add(fee_ants)
            .ok_or_else(|| anyhow!("burn amount + validator fee overflowed u64"))?;

        let normalized = address.trim().to_uppercase();

        // Snapshot validator set before mutable account borrows.
        let validators: Vec<String> = self.eligible_miners.clone();

        // Debit (burn amount + validator fee) atomically.
        let new_balance = {
            let account = self
                .accounts
                .get_mut(&normalized)
                .ok_or_else(|| anyhow!("account not found"))?;
            if account.ants_balance < total_debit {
                return Err(anyhow!(
                    "insufficient ANET balance: have {} ants, need {} ants \
                     ({} bridge + {} validator fee)",
                    account.ants_balance,
                    total_debit,
                    amount_ants,
                    fee_ants
                ));
            }
            debit_anet(account, total_debit)?;
            account.ants_balance
        };

        // Distribute the validator fee. If there are no validators (genesis
        // race or post-clawback empty set), the fee goes to the system fee
        // reserve so it isn't lost.
        if validators.is_empty() {
            if let Some(reserve) = self.accounts.get_mut(SYSTEM_FEE_RESERVE_ADDRESS) {
                reserve.ants_balance = reserve
                    .ants_balance
                    .checked_add(fee_ants)
                    .ok_or_else(|| anyhow!("fee reserve overflow"))?;
            }
        } else {
            let n = validators.len() as u64;
            let per = fee_ants / n;
            let remainder = fee_ants % n;
            if per > 0 {
                for v in &validators {
                    let acc = self
                        .accounts
                        .entry(v.clone())
                        .or_insert_with(|| AccountState {
                            address: v.clone(),
                            ants_balance: 0,
                            activated_ants: 0,
                            total_activated_ants: 0,
                            sessions: 0,
                            asset_balances: HashMap::new(),
                        });
                    acc.ants_balance = acc
                        .ants_balance
                        .checked_add(per)
                        .ok_or_else(|| anyhow!("validator fee credit overflow"))?;
                }
            }
            if remainder > 0 {
                if let Some(reserve) = self.accounts.get_mut(SYSTEM_FEE_RESERVE_ADDRESS) {
                    reserve.ants_balance = reserve
                        .ants_balance
                        .checked_add(remainder)
                        .ok_or_else(|| anyhow!("fee reserve remainder overflow"))?;
                }
            }
        }

        self.pending_block_event = Some(format!(
            "Bridge: Burn {amount_ants} ants from {normalized} for BSC release \
             (+{fee_ants} ANTS validator fee, split across {} validators)",
            validators.len()
        ));
        self.pending_state_commit = true;
        Ok(new_balance)
    }

    /// One-shot legacy-derivation → secp-derivation wallet migration.
    /// Moves ALL balance (ants + activated + total_activated + sessions +
    /// asset balances) from `legacy` to `secp`, atomically. Used after the
    /// caller has proven, via the dual-derivation reveal in
    /// `/wallet/migrate-legacy/reveal`, that the same private key controls
    /// both addresses. After migration the legacy address is left at zero
    /// and effectively retired; future deposits to it (e.g. delayed mining
    /// rewards) can be claimed by running migrate-legacy again.
    pub fn migrate_legacy_to_secp(
        &mut self,
        legacy: &str,
        secp: &str,
    ) -> Result<(u64, u64, u64)> {
        let legacy_norm = legacy.trim().to_uppercase();
        let secp_norm = secp.trim().to_uppercase();
        if legacy_norm == secp_norm {
            return Err(anyhow!("legacy and secp addresses must differ"));
        }
        let (
            ants_moved,
            activated_moved,
            total_activated_moved,
            sessions_moved,
            assets_moved,
        ) = {
            let legacy_acct = self
                .accounts
                .get_mut(&legacy_norm)
                .ok_or_else(|| anyhow!("legacy account not found"))?;
            let ants = legacy_acct.ants_balance;
            let activated = legacy_acct.activated_ants;
            let total_activated = legacy_acct.total_activated_ants;
            let sessions = legacy_acct.sessions;
            let assets: Vec<(String, u64)> = legacy_acct
                .asset_balances
                .iter()
                .map(|(k, v)| (k.clone(), *v))
                .collect();
            legacy_acct.ants_balance = 0;
            legacy_acct.activated_ants = 0;
            // Preserve historical total_activated on legacy (auditing) but zero
            // the live counters. Sessions are migrated (transferred), not
            // duplicated, so we zero them here.
            legacy_acct.sessions = 0;
            legacy_acct.asset_balances.clear();
            (ants, activated, total_activated, sessions, assets)
        };

        let secp_acct = self
            .accounts
            .entry(secp_norm.clone())
            .or_insert_with(|| AccountState {
                address: secp_norm.clone(),
                ants_balance: 0,
                activated_ants: 0,
                total_activated_ants: 0,
                sessions: 0,
                asset_balances: HashMap::new(),
            });
        secp_acct.ants_balance = secp_acct
            .ants_balance
            .checked_add(ants_moved)
            .ok_or_else(|| anyhow!("ANET balance overflowed on secp side"))?;
        secp_acct.activated_ants = secp_acct
            .activated_ants
            .checked_add(activated_moved)
            .ok_or_else(|| anyhow!("activated_ants overflowed on secp side"))?;
        secp_acct.total_activated_ants = secp_acct
            .total_activated_ants
            .checked_add(total_activated_moved)
            .ok_or_else(|| anyhow!("total_activated_ants overflowed on secp side"))?;
        secp_acct.sessions = secp_acct
            .sessions
            .checked_add(sessions_moved)
            .ok_or_else(|| anyhow!("sessions overflowed on secp side"))?;
        for (sym, amt) in assets_moved {
            let entry = secp_acct.asset_balances.entry(sym).or_insert(0);
            *entry = entry
                .checked_add(amt)
                .ok_or_else(|| anyhow!("asset balance overflowed on secp side"))?;
        }

        self.pending_block_event = Some(format!(
            "WalletMigration: {legacy_norm} -> {secp_norm} ants={ants_moved} sessions={sessions_moved}"
        ));
        self.pending_state_commit = true;
        Ok((ants_moved, sessions_moved, total_activated_moved))
    }

    /// Admin-only genesis bootstrap: creates a wallet with enough sessions to be
    /// eligible, mints the supplied ANET + stablecoin amounts, and opens the
    /// first DEX pool in a single atomic call — no real mined balance required.
    pub fn admin_genesis_bootstrap(
        &mut self,
        address: &str,
        anet_amount_ants: u64,
        token_symbol: &str,
        token_amount_units: u64,
        fee_bps: Option<u16>,
    ) -> Result<DexLiquidityResult> {
        let normalized = address.trim().to_uppercase();
        let symbol = dex::normalize_token_symbol(token_symbol)?;
        let pair_id = dex::pool_key(&symbol)?;

        if self.dex_pools.contains_key(&pair_id) {
            return Err(anyhow!("DEX pool for {} already exists", symbol));
        }
        if anet_amount_ants == 0 || token_amount_units == 0 {
            return Err(anyhow!("initial liquidity must be greater than zero"));
        }

        // Ensure account exists and is eligible (bypass 1k session check for genesis)
        {
            let account = self
                .accounts
                .entry(normalized.clone())
                .or_insert_with(|| AccountState {
                    address: normalized.clone(),
                    ants_balance: 0,
                    activated_ants: 0,
                    total_activated_ants: 0,
                    sessions: crate::activation::MIN_SESSIONS_FOR_ANET,
                    asset_balances: HashMap::new(),
                });
            if account.sessions < crate::activation::MIN_SESSIONS_FOR_ANET {
                account.sessions = crate::activation::MIN_SESSIONS_FOR_ANET;
            }
            // Mint ANET and token directly into account then debit for pool
            account.ants_balance = account
                .ants_balance
                .checked_add(anet_amount_ants)
                .ok_or_else(|| anyhow!("ANET balance overflowed"))?;
            let cur_token = account.asset_balances.get(&symbol).copied().unwrap_or(0);
            account.asset_balances.insert(
                symbol.clone(),
                cur_token
                    .checked_add(token_amount_units)
                    .ok_or_else(|| anyhow!("{} balance overflowed", symbol))?,
            );
            debit_anet(account, anet_amount_ants)?;
            debit_asset(account, &symbol, token_amount_units)?;
        }

        let fee_bps = dex::validate_fee_bps(fee_bps.unwrap_or(dex::DEFAULT_FEE_BPS))?;
        let total_lp_units =
            dex::initial_lp_units(u128::from(anet_amount_ants), u128::from(token_amount_units))?;

        let mut lp_positions = HashMap::new();
        lp_positions.insert(normalized.clone(), total_lp_units);

        let pool = DexPool {
            pair_id: pair_id.clone(),
            token_symbol: symbol.clone(),
            anet_reserve_ants: u128::from(anet_amount_ants),
            token_reserve_units: u128::from(token_amount_units),
            total_lp_units,
            fee_bps,
            lp_positions,
            updated_at: Utc::now(),
        };
        self.dex_pools.insert(pair_id.clone(), pool);
        self.pending_block_event = Some("Genesis: Bootstrap DEX Pool".to_string());
        self.pending_state_commit = true;

        Ok(DexLiquidityResult {
            pair_id,
            provider: normalized,
            lp_minted: total_lp_units.to_string(),
            total_lp_units: total_lp_units.to_string(),
        })
    }

    pub fn dex_create_pool(
        &mut self,
        provider: &str,
        token_symbol: &str,
        anet_amount_ants: u64,
        token_amount_units: u64,
        fee_bps: Option<u16>,
    ) -> Result<DexLiquidityResult> {
        let provider = provider.trim().to_uppercase();
        let symbol = dex::normalize_token_symbol(token_symbol)?;
        let pair_id = dex::pool_key(&symbol)?;

        if self.dex_pools.contains_key(&pair_id) {
            return Err(anyhow!("pool already exists"));
        }
        if anet_amount_ants == 0 || token_amount_units == 0 {
            return Err(anyhow!("initial liquidity must be greater than zero"));
        }

        let account = self.ensure_eligible_account_mut(&provider)?;
        debit_anet(account, anet_amount_ants)?;
        debit_asset(account, &symbol, token_amount_units)?;

        let fee_bps = dex::validate_fee_bps(fee_bps.unwrap_or(dex::DEFAULT_FEE_BPS))?;
        let total_lp_units =
            dex::initial_lp_units(u128::from(anet_amount_ants), u128::from(token_amount_units))?;

        let mut lp_positions = HashMap::new();
        lp_positions.insert(provider.clone(), total_lp_units);

        let pool = DexPool {
            pair_id: pair_id.clone(),
            token_symbol: symbol.clone(),
            anet_reserve_ants: u128::from(anet_amount_ants),
            token_reserve_units: u128::from(token_amount_units),
            total_lp_units,
            fee_bps,
            lp_positions,
            updated_at: Utc::now(),
        };

        self.dex_pools.insert(pair_id.clone(), pool);
        self.pending_block_event = Some("DEX: Create Pool".to_string());
        self.emit_event(
            ChainEvent::new("PoolCreated")
                .attr("pair_id", &pair_id)
                .attr("provider", &provider)
                .attr("token_symbol", &symbol),
        );
        self.pending_state_commit = true;

        Ok(DexLiquidityResult {
            pair_id,
            provider,
            lp_minted: total_lp_units.to_string(),
            total_lp_units: total_lp_units.to_string(),
        })
    }

    pub fn dex_wrap_anet(&mut self, wallet: &str, amount_ants: u64) -> Result<u64> {
        if amount_ants == 0 {
            return Err(anyhow!("wrap amount must be greater than zero"));
        }

        let wanet_balance = {
            let account = self.ensure_eligible_account_mut(&wallet.trim().to_uppercase())?;
            debit_anet(account, amount_ants)?;
            credit_asset(account, WANET_SYMBOL, amount_ants)?;
            account
                .asset_balances
                .get(WANET_SYMBOL)
                .copied()
                .unwrap_or(0)
        };
        self.pending_block_event = Some("DEX: Wrap ANET -> WANET".to_string());
        self.pending_state_commit = true;

        Ok(wanet_balance)
    }

    pub fn dex_unwrap_wanet(&mut self, wallet: &str, amount_units: u64) -> Result<u64> {
        if amount_units == 0 {
            return Err(anyhow!("unwrap amount must be greater than zero"));
        }

        let anet_balance = {
            let account = self.ensure_eligible_account_mut(&wallet.trim().to_uppercase())?;
            debit_asset(account, WANET_SYMBOL, amount_units)?;
            credit_anet(account, amount_units)?;
            account.ants_balance
        };
        self.pending_block_event = Some("DEX: Unwrap WANET -> ANET".to_string());
        self.pending_state_commit = true;

        Ok(anet_balance)
    }

    pub fn dex_add_liquidity(
        &mut self,
        provider: &str,
        token_symbol: &str,
        anet_amount_ants: u64,
        token_amount_units: u64,
    ) -> Result<DexLiquidityResult> {
        let provider = provider.trim().to_uppercase();
        let pair_id = dex::pool_key(token_symbol)?;
        if anet_amount_ants == 0 || token_amount_units == 0 {
            return Err(anyhow!("liquidity amounts must be greater than zero"));
        }

        let (pool_token_symbol, pool_anet_reserve, pool_token_reserve, pool_total_lp) = {
            let pool = self
                .dex_pools
                .get(&pair_id)
                .ok_or_else(|| anyhow!("pool not found"))?;
            (
                pool.token_symbol.clone(),
                pool.anet_reserve_ants,
                pool.token_reserve_units,
                pool.total_lp_units,
            )
        };

        let required_token = u128::from(anet_amount_ants)
            .checked_mul(pool_token_reserve)
            .ok_or_else(|| anyhow!("required token overflowed"))?
            / pool_anet_reserve;
        let provided_token = u128::from(token_amount_units);
        let drift = required_token.abs_diff(provided_token);
        if drift > 1 {
            return Err(anyhow!(
                "liquidity ratio mismatch: expected roughly {required_token} {}, got {provided_token}",
                pool_token_symbol
            ));
        }

        let minted_lp = u128::from(anet_amount_ants)
            .checked_mul(pool_total_lp)
            .ok_or_else(|| anyhow!("LP mint overflowed"))?
            / pool_anet_reserve;
        if minted_lp == 0 {
            return Err(anyhow!("liquidity contribution is too small"));
        }

        {
            let account = self.ensure_eligible_account_mut(&provider)?;
            debit_anet(account, anet_amount_ants)?;
            debit_asset(account, &pool_token_symbol, token_amount_units)?;
        }

        let total_lp_units = {
            let pool = self
                .dex_pools
                .get_mut(&pair_id)
                .ok_or_else(|| anyhow!("pool not found"))?;

            pool.anet_reserve_ants = pool
                .anet_reserve_ants
                .checked_add(u128::from(anet_amount_ants))
                .ok_or_else(|| anyhow!("pool ANET reserve overflowed"))?;
            pool.token_reserve_units = pool
                .token_reserve_units
                .checked_add(u128::from(token_amount_units))
                .ok_or_else(|| anyhow!("pool token reserve overflowed"))?;
            pool.total_lp_units = pool
                .total_lp_units
                .checked_add(minted_lp)
                .ok_or_else(|| anyhow!("pool LP overflowed"))?;
            *pool.lp_positions.entry(provider.clone()).or_insert(0) += minted_lp;
            pool.updated_at = Utc::now();
            pool.total_lp_units.to_string()
        };

        self.pending_block_event = Some("DEX: Add Liquidity".to_string());
        self.emit_event(
            ChainEvent::new("LiquidityAdded")
                .attr("pair_id", &pair_id)
                .attr("provider", &provider)
                .attr("lp_minted", minted_lp.to_string()),
        );
        self.pending_state_commit = true;

        Ok(DexLiquidityResult {
            pair_id,
            provider,
            lp_minted: minted_lp.to_string(),
            total_lp_units,
        })
    }

    /// Admin-only helper used for private-mainnet bootstrap and emergency top-ups.
    /// It credits ANET + token to the provider wallet then adds liquidity using
    /// normal AMM ratio checks.
    pub fn admin_top_up_liquidity(
        &mut self,
        provider: &str,
        token_symbol: &str,
        anet_amount_ants: u64,
        token_amount_units: u64,
    ) -> Result<DexLiquidityResult> {
        if anet_amount_ants == 0 || token_amount_units == 0 {
            return Err(anyhow!("liquidity amounts must be greater than zero"));
        }

        let provider = provider.trim().to_uppercase();
        let symbol = dex::normalize_token_symbol(token_symbol)?;

        {
            let account = self
                .accounts
                .entry(provider.clone())
                .or_insert_with(|| AccountState {
                    address: provider.clone(),
                    ants_balance: 0,
                    activated_ants: 0,
                    total_activated_ants: 0,
                    sessions: crate::activation::MIN_SESSIONS_FOR_ANET,
                    asset_balances: HashMap::new(),
                });

            if account.sessions < crate::activation::MIN_SESSIONS_FOR_ANET {
                account.sessions = crate::activation::MIN_SESSIONS_FOR_ANET;
            }

            account.ants_balance = account
                .ants_balance
                .checked_add(anet_amount_ants)
                .ok_or_else(|| anyhow!("ANET balance overflowed"))?;

            let current_token = account.asset_balances.get(&symbol).copied().unwrap_or(0);
            account.asset_balances.insert(
                symbol.clone(),
                current_token
                    .checked_add(token_amount_units)
                    .ok_or_else(|| anyhow!("{} balance overflowed", symbol))?,
            );
        }

        let result =
            self.dex_add_liquidity(&provider, &symbol, anet_amount_ants, token_amount_units)?;
        self.pending_block_event = Some("Admin: Top Up Liquidity".to_string());
        self.pending_state_commit = true;
        Ok(result)
    }

    pub fn dex_quote(
        &self,
        token_symbol: &str,
        amount_in: u64,
        anet_to_token: bool,
    ) -> Result<DexQuoteView> {
        let pair_id = dex::pool_key(token_symbol)?;
        let pool = self
            .dex_pools
            .get(&pair_id)
            .ok_or_else(|| anyhow!("pool not found"))?;

        let (reserve_in, reserve_out, direction) = if anet_to_token {
            (
                pool.anet_reserve_ants,
                pool.token_reserve_units,
                format!("ANET->{}", pool.token_symbol),
            )
        } else {
            (
                pool.token_reserve_units,
                pool.anet_reserve_ants,
                format!("{}->ANET", pool.token_symbol),
            )
        };

        let (amount_out, fee_paid, price_impact_bps) =
            dex::quote_amount_out(reserve_in, reserve_out, u128::from(amount_in), pool.fee_bps)?;
        let min_out_1pct_slippage = amount_out
            .checked_mul(99)
            .ok_or_else(|| anyhow!("slippage math overflowed"))?
            / 100;

        Ok(DexQuoteView {
            pair_id,
            direction,
            amount_in: amount_in.to_string(),
            amount_out: amount_out.to_string(),
            fee_paid: fee_paid.to_string(),
            min_out_1pct_slippage: min_out_1pct_slippage.to_string(),
            price_impact_bps,
        })
    }

    pub fn dex_swap(
        &mut self,
        trader: &str,
        token_symbol: &str,
        amount_in: u64,
        anet_to_token: bool,
        min_amount_out: Option<u64>,
        deadline_block: Option<u64>,
    ) -> Result<DexSwapResult> {
        let current_height = self
            .blocks
            .last()
            .map(|block| block.block_height)
            .unwrap_or(0);
        if let Some(deadline_block) = deadline_block {
            if current_height > deadline_block {
                return Err(anyhow!(
                    "swap deadline expired: current block {} exceeds deadline {}",
                    current_height,
                    deadline_block
                ));
            }
        }

        let trader = trader.trim().to_uppercase();
        let pair_id = dex::pool_key(token_symbol)?;
        if amount_in == 0 {
            return Err(anyhow!("swap amount must be greater than zero"));
        }

        let (pool_token_symbol, pool_fee_bps, reserve_in, reserve_out, direction) = {
            let pool = self
                .dex_pools
                .get(&pair_id)
                .ok_or_else(|| anyhow!("pool not found"))?;
            let (reserve_in, reserve_out, direction) = if anet_to_token {
                (
                    pool.anet_reserve_ants,
                    pool.token_reserve_units,
                    format!("ANET->{}", pool.token_symbol),
                )
            } else {
                (
                    pool.token_reserve_units,
                    pool.anet_reserve_ants,
                    format!("{}->ANET", pool.token_symbol),
                )
            };
            (
                pool.token_symbol.clone(),
                pool.fee_bps,
                reserve_in,
                reserve_out,
                direction,
            )
        };

        let (amount_out, fee_paid, _) =
            dex::quote_amount_out(reserve_in, reserve_out, u128::from(amount_in), pool_fee_bps)?;

        {
            let account = self.ensure_eligible_account_mut(&trader)?;
            if anet_to_token {
                debit_anet(account, amount_in)?;
                credit_asset(
                    account,
                    &pool_token_symbol,
                    u64::try_from(amount_out).map_err(|_| anyhow!("swap output overflowed"))?,
                )?;
            } else {
                debit_asset(account, &pool_token_symbol, amount_in)?;
                credit_anet(
                    account,
                    u64::try_from(amount_out).map_err(|_| anyhow!("swap output overflowed"))?,
                )?;
            }
        }

        let pool = self
            .dex_pools
            .get_mut(&pair_id)
            .ok_or_else(|| anyhow!("pool not found"))?;

        let min_expected = u128::from(min_amount_out.unwrap_or(0));
        if amount_out < min_expected {
            return Err(anyhow!(
                "slippage exceeded: amount_out {} is below min_amount_out {}",
                amount_out,
                min_expected
            ));
        }

        let k_before = pool
            .anet_reserve_ants
            .checked_mul(pool.token_reserve_units)
            .ok_or_else(|| anyhow!("pool invariant overflowed before swap"))?;

        if anet_to_token {
            pool.anet_reserve_ants = pool
                .anet_reserve_ants
                .checked_add(u128::from(amount_in))
                .ok_or_else(|| anyhow!("pool reserve overflowed"))?;
            pool.token_reserve_units = pool
                .token_reserve_units
                .checked_sub(amount_out)
                .ok_or_else(|| anyhow!("pool token reserve underflowed"))?;
        } else {
            pool.token_reserve_units = pool
                .token_reserve_units
                .checked_add(u128::from(amount_in))
                .ok_or_else(|| anyhow!("pool reserve overflowed"))?;
            pool.anet_reserve_ants = pool
                .anet_reserve_ants
                .checked_sub(amount_out)
                .ok_or_else(|| anyhow!("pool ANET reserve underflowed"))?;
        }

        let k_after = pool
            .anet_reserve_ants
            .checked_mul(pool.token_reserve_units)
            .ok_or_else(|| anyhow!("pool invariant overflowed after swap"))?;
        if k_after < k_before {
            return Err(anyhow!(
                "pool invariant violation detected after swap execution"
            ));
        }

        pool.updated_at = Utc::now();
        self.pending_block_event = Some("DEX: Swap".to_string());
        self.emit_event(
            ChainEvent::new("SwapExecuted")
                .attr("pair_id", &pair_id)
                .attr("trader", &trader)
                .attr("direction", &direction)
                .attr("amount_in", amount_in)
                .attr("amount_out", amount_out),
        );
        self.pending_state_commit = true;

        Ok(DexSwapResult {
            pair_id,
            trader,
            direction,
            amount_in: amount_in.to_string(),
            amount_out: amount_out.to_string(),
            fee_paid: fee_paid.to_string(),
        })
    }

    pub fn record_pi_settlement(
        &mut self,
        pi_payment_id: &str,
        pi_txid: &str,
        pi_amount: &str,
        from_address: &str,
        to_address: &str,
    ) -> Result<()> {
        // Validate addresses
        if !crate::transaction::is_valid_anet_wallet(from_address) {
            return Err(anyhow!(
                "invalid from address for Pi settlement transaction"
            ));
        }
        if !crate::transaction::is_valid_anet_wallet(to_address) {
            return Err(anyhow!("invalid to address for Pi settlement transaction"));
        }

        // Create a settlement transaction recording Pi payment details
        let memo = format!(
            "Pi Payment Settlement: paymentId={}, txid={}, amount={}",
            pi_payment_id, pi_txid, pi_amount
        );

        let tx = crate::transaction::Transaction {
            tx_type: String::new(),
            from: from_address.to_uppercase(),
            to: to_address.to_uppercase(),
            amount_ants: 1, // Minimal transfer to keep the settlement ledger-visible as a transaction
            fee_ants: 1_000, // Preserve minimum fee policy
            nonce: 0,
            memo,
            timestamp: Utc::now(),
            chain_id: String::new(),
            payload: serde_json::Value::Object(serde_json::Map::new()),
            signature: String::new(),
            tx_hash: String::new(),
        };

        // Ensure the sender can cover settlement debit by topping up from system fee reserve when needed.
        let total_debit = tx.total_debit()?;
        let sender_balance = self
            .accounts
            .get(&tx.from)
            .ok_or_else(|| anyhow!("sender account not found for Pi settlement transaction"))?
            .ants_balance;
        if sender_balance < total_debit {
            let deficit = total_debit - sender_balance;
            let reserve = self
                .accounts
                .get_mut(SYSTEM_FEE_RESERVE_ADDRESS)
                .ok_or_else(|| anyhow!("system fee reserve account not found"))?;
            if reserve.ants_balance < deficit {
                return Err(anyhow!(
                    "insufficient fee reserve to backfill Pi settlement sender balance"
                ));
            }
            reserve.ants_balance = reserve
                .ants_balance
                .checked_sub(deficit)
                .ok_or_else(|| anyhow!("fee reserve underflow while funding Pi settlement"))?;

            let sender = self
                .accounts
                .get_mut(&tx.from)
                .ok_or_else(|| anyhow!("sender account not found for Pi settlement transaction"))?;
            sender.ants_balance = sender
                .ants_balance
                .checked_add(deficit)
                .ok_or_else(|| anyhow!("sender balance overflow while funding Pi settlement"))?;
            sender.activated_ants = sender.activated_ants.saturating_add(deficit);
            sender.total_activated_ants = sender.total_activated_ants.saturating_add(deficit);
        }

        tx.validate()?;
        self.mempool.push(tx);
        self.pending_block_event = Some("Pi: Payment Settlement".to_string());
        self.pending_state_commit = true;

        Ok(())
    }

    pub fn anrc20_list_tokens(&self) -> Vec<Anrc20TokenView> {
        let mut tokens = self
            .token_registry
            .values()
            .map(Anrc20Token::view)
            .collect::<Vec<_>>();
        tokens.sort_by(|left, right| left.symbol.cmp(&right.symbol));
        tokens
    }

    pub fn anrc20_token_view(&self, symbol: &str) -> Option<Anrc20TokenView> {
        let key = dex::normalize_token_symbol(symbol).ok()?;
        self.token_registry.get(&key).map(Anrc20Token::view)
    }

    pub fn anrc20_create_token(
        &mut self,
        owner: &str,
        symbol: &str,
        name: &str,
        decimals: u8,
        initial_supply: u64,
        mintable: bool,
    ) -> Result<Anrc20TokenView> {
        let symbol = dex::normalize_token_symbol(symbol)?;
        if self.token_registry.contains_key(&symbol) {
            return Err(anyhow!("token already exists"));
        }
        if name.trim().is_empty() || name.chars().count() > 64 {
            return Err(anyhow!("token name must be between 1 and 64 characters"));
        }
        if decimals > 18 {
            return Err(anyhow!("token decimals must be <= 18"));
        }

        let owner = owner.trim().to_uppercase();
        let owner_account = self
            .accounts
            .entry(owner.clone())
            .or_insert_with(|| AccountState {
                address: owner.clone(),
                ants_balance: 0,
                activated_ants: 0,
                total_activated_ants: 0,
                sessions: 0,
                asset_balances: HashMap::new(),
            });

        if initial_supply > 0 {
            credit_asset(owner_account, &symbol, initial_supply)?;
        }

        let mut balances = HashMap::new();
        if initial_supply > 0 {
            balances.insert(owner.clone(), initial_supply);
        }

        let token = Anrc20Token {
            symbol: symbol.clone(),
            name: name.trim().to_owned(),
            decimals,
            total_supply: initial_supply,
            owner: owner.clone(),
            mintable,
            balances,
        };
        self.token_registry.insert(symbol.clone(), token);

        self.pending_block_event = Some("ANRC20: Create Token".to_string());
        self.emit_event(
            ChainEvent::new("TokenTransferred")
                .attr("symbol", &symbol)
                .attr("from", "MINT")
                .attr("to", &owner)
                .attr("amount", initial_supply),
        );
        self.pending_state_commit = true;

        self.token_registry
            .get(&symbol)
            .map(Anrc20Token::view)
            .ok_or_else(|| anyhow!("token creation failed"))
    }

    pub fn anrc20_mint(
        &mut self,
        caller: &str,
        to: &str,
        symbol: &str,
        amount: u64,
    ) -> Result<Anrc20TokenView> {
        if amount == 0 {
            return Err(anyhow!("mint amount must be greater than zero"));
        }
        let symbol = dex::normalize_token_symbol(symbol)?;
        let caller = caller.trim().to_uppercase();
        let to = to.trim().to_uppercase();

        {
            let token = self
                .token_registry
                .get_mut(&symbol)
                .ok_or_else(|| anyhow!("token not found"))?;
            if !token.mintable {
                return Err(anyhow!("token is not mintable"));
            }
            if token.owner != caller {
                return Err(anyhow!("only token owner can mint"));
            }
            token.total_supply = token
                .total_supply
                .checked_add(amount)
                .ok_or_else(|| anyhow!("token total supply overflowed"))?;
            let balance = token.balances.get(&to).copied().unwrap_or(0);
            token.balances.insert(
                to.clone(),
                balance
                    .checked_add(amount)
                    .ok_or_else(|| anyhow!("token balance overflowed"))?,
            );
        }

        let to_account = self
            .accounts
            .entry(to.clone())
            .or_insert_with(|| AccountState {
                address: to.clone(),
                ants_balance: 0,
                activated_ants: 0,
                total_activated_ants: 0,
                sessions: 0,
                asset_balances: HashMap::new(),
            });
        credit_asset(to_account, &symbol, amount)?;

        self.pending_block_event = Some("ANRC20: Mint".to_string());
        self.emit_event(
            ChainEvent::new("TokenTransferred")
                .attr("symbol", &symbol)
                .attr("from", "MINT")
                .attr("to", &to)
                .attr("amount", amount),
        );
        self.pending_state_commit = true;

        self.token_registry
            .get(&symbol)
            .map(Anrc20Token::view)
            .ok_or_else(|| anyhow!("token mint failed"))
    }

    pub fn anrc20_transfer(
        &mut self,
        from: &str,
        to: &str,
        symbol: &str,
        amount: u64,
    ) -> Result<()> {
        if amount == 0 {
            return Err(anyhow!("transfer amount must be greater than zero"));
        }

        let symbol = dex::normalize_token_symbol(symbol)?;
        let from = from.trim().to_uppercase();
        let to = to.trim().to_uppercase();
        if from == to {
            return Err(anyhow!("token sender and recipient must differ"));
        }

        let from_account = self
            .accounts
            .get_mut(&from)
            .ok_or_else(|| anyhow!("sender account not found"))?;
        debit_asset(from_account, &symbol, amount)?;

        let to_account = self
            .accounts
            .entry(to.clone())
            .or_insert_with(|| AccountState {
                address: to.clone(),
                ants_balance: 0,
                activated_ants: 0,
                total_activated_ants: 0,
                sessions: 0,
                asset_balances: HashMap::new(),
            });
        credit_asset(to_account, &symbol, amount)?;

        let token = self
            .token_registry
            .get_mut(&symbol)
            .ok_or_else(|| anyhow!("token not found"))?;
        let from_balance = token.balances.get(&from).copied().unwrap_or(0);
        if from_balance < amount {
            return Err(anyhow!("insufficient token balance"));
        }
        token.balances.insert(from.clone(), from_balance - amount);
        let to_balance = token.balances.get(&to).copied().unwrap_or(0);
        token.balances.insert(
            to.clone(),
            to_balance
                .checked_add(amount)
                .ok_or_else(|| anyhow!("token balance overflowed"))?,
        );

        self.pending_block_event = Some("ANRC20: Transfer".to_string());
        self.emit_event(
            ChainEvent::new("TokenTransferred")
                .attr("symbol", &symbol)
                .attr("from", &from)
                .attr("to", &to)
                .attr("amount", amount),
        );
        self.pending_state_commit = true;

        Ok(())
    }

    fn ensure_eligible_account_mut(&mut self, address: &str) -> Result<&mut AccountState> {
        // Bridge-funded holders bought their ANET with real value; exempt them
        // from the mined-ANTS activation gate on the native DEX. Checked before
        // the mutable borrow of `self.accounts`.
        let bridge_funded = self.bridge_funded_accounts.contains(address);
        let account = self
            .accounts
            .get_mut(address)
            .ok_or_else(|| anyhow!("account not found"))?;
        if !Self::allow_ineligible_wallet_test_mode()
            && !bridge_funded
            && account.sessions < crate::activation::MIN_SESSIONS_FOR_ANET
        {
            return Err(anyhow!(
                "wallet must complete at least {} sessions before spending mined ANTS/ANET on native DEX",
                crate::activation::MIN_SESSIONS_FOR_ANET
            ));
        }
        Ok(account)
    }

    fn emit_event(&mut self, event: ChainEvent) {
        self.pending_events.push(event);
    }

    pub fn record_app_activity_event(
        &mut self,
        action: &str,
        wallet: Option<&str>,
        detail: Option<&str>,
    ) {
        let action = action.trim();
        if action.is_empty() {
            return;
        }

        let mut event = ChainEvent::new("AppActivity").attr("action", action);
        if let Some(wallet) = wallet {
            let wallet = wallet.trim().to_uppercase();
            if !wallet.is_empty() {
                event = event.attr("wallet", wallet);
            }
        }
        if let Some(detail) = detail {
            let detail = detail.trim();
            if !detail.is_empty() {
                event = event.attr("detail", detail);
            }
        }

        self.emit_event(event);
        if self.pending_block_event.is_none() {
            self.pending_block_event = Some("App: Activity Audit".to_owned());
        }
        self.pending_state_commit = true;
    }

    fn allow_ineligible_wallet_test_mode() -> bool {
        std::env::var("ANET_ALLOW_INELIGIBLE_WALLET_TEST")
            .map(|value| {
                let normalized = value.trim().to_ascii_lowercase();
                normalized == "1"
                    || normalized == "true"
                    || normalized == "yes"
                    || normalized == "on"
            })
            .unwrap_or(false)
    }

    pub fn has_block_in_epoch(&self, epoch_start: DateTime<Utc>) -> bool {
        self.blocks
            .last()
            .map(|block| block.epoch_start == epoch_start)
            .unwrap_or(false)
    }

    pub async fn create_block(
        &mut self,
        epoch_start: DateTime<Utc>,
        epoch_end: DateTime<Utc>,
    ) -> Result<Block> {
        let block_height = self
            .blocks
            .last()
            .map(|block| block.block_height.saturating_add(1))
            .unwrap_or(0);
        let previous_hash = db::get_last_hash(self.chain_db.as_ref())
            .await?
            .unwrap_or_else(|| "GENESIS".to_owned());
        let miners = self.eligible_miners.clone();
        let mut transactions = std::mem::take(&mut self.mempool);
        sort_transactions_for_block(&mut transactions);
        let activated_supply_ants = std::mem::take(&mut self.pending_activated_supply_ants);
        let block_event = std::mem::take(&mut self.pending_block_event);
        let mut events = std::mem::take(&mut self.pending_events);

        for transaction in &transactions {
            events.push(
                ChainEvent::new("TokenTransferred")
                    .with_tx_hash(transaction.id()?)
                    .attr("symbol", "ANET")
                    .attr("from", &transaction.from)
                    .attr("to", &transaction.to)
                    .attr("amount", transaction.amount_ants),
            );
        }

        let dex_snapshot = self.dex_pools.clone();
        let token_snapshot = self.token_registry.clone();
        let tpow_proofs = self.pending_proofs_for_block(300);

        let block = Block::new(
            block_height,
            epoch_start,
            epoch_end,
            previous_hash,
            transactions,
            activated_supply_ants,
            miners,
            block_event,
            events,
            dex_snapshot,
            token_snapshot,
            tpow_proofs,
            Vec::new(),
            String::new(),
            String::new(),
        )?;

        apply_block(&mut self.accounts, &block)?;

        if !block.validate_hash()? {
            return Err(anyhow!("generated block hash failed validation"));
        }

        for transaction in &block.transactions {
            self.account_nonces
                .insert(transaction.from.clone(), transaction.nonce);
        }

        db::store_block_and_update_tip(self.chain_db.as_ref(), &block).await?;
        self.blocks.push(block.clone());
        self.pending_state_commit = false;

        tracing::info!(
            block_height = block.block_height,
            hash = %block.hash,
            tx_count = block.transactions.len(),
            activated_supply_ants = block.activated_supply_ants,
            miners = block.miners.len(),
            total_fees_ants = block.total_fees_ants,
            "created TPoW block"
        );

        Ok(block)
    }

    fn persist_genesis_snapshot(&self) -> Result<()> {
        let mut accounts = self
            .accounts
            .values()
            .filter(|account| account.activated_ants > 0 || account.sessions > 0)
            .map(|account| crate::activation::GenesisAccount {
                address: account.address.clone(),
                ants_balance: account.activated_ants,
                sessions: account.sessions,
                total_activated_ants: account.total_activated_ants,
            })
            .collect::<Vec<_>>();
        accounts.sort_by(|left, right| left.address.cmp(&right.address));

        crate::activation::write_genesis(
            &self.genesis_path,
            &crate::activation::GenesisConfig {
                chain_id: self.chain_id.clone(),
                genesis_time: self.genesis_time,
                accounts,
            },
        )
    }
}

pub async fn bootstrap_chain_store(
    genesis: &GenesisConfig,
    _chain_path: &std::path::Path,
) -> Result<()> {
    let chain_db = db::connect().await?;
    db::ensure_chain_initialized(chain_db.as_ref(), &genesis.chain_id, genesis.genesis_time).await
}

fn replay_blocks(accounts: &mut HashMap<String, AccountState>, blocks: &[Block]) -> Result<()> {
    reconcile_replay_sender_balances(accounts, blocks)?;

    for block in blocks {
        apply_block(accounts, block)?;
    }

    Ok(())
}

/// Rebuild the EVM bridge idempotency set from durable block history. Each
/// bridge credit is recorded as a transaction whose memo is
/// `bridge:evm:<lowercase tx hash>`; scanning those once on startup restores
/// the dedup guard that previously lived only in RAM.
fn collect_processed_bridge_hashes(blocks: &[Block]) -> HashSet<String> {
    let mut hashes = HashSet::new();
    for block in blocks {
        for transaction in &block.transactions {
            if let Some(rest) = transaction.memo.strip_prefix(BRIDGE_CREDIT_MEMO_PREFIX) {
                let hash = rest.trim().to_lowercase();
                if !hash.is_empty() {
                    hashes.insert(hash);
                }
            }
        }
    }
    hashes
}

/// Rebuild the set of bridge-funded recipient addresses from durable block
/// history. Each bridge credit is a `BRIDGE_TREASURY_ADDRESS` → recipient
/// transfer whose memo is `bridge:evm:<hash>`; the recipient (`to`) of any such
/// transaction holds paid-for ANET and is therefore exempt from the
/// `MIN_SESSIONS_FOR_ANET` spend gate. Scanning once on startup restores the
/// exemption that would otherwise live only in RAM.
fn collect_bridge_funded_accounts(blocks: &[Block]) -> HashSet<String> {
    let mut funded = HashSet::new();
    for block in blocks {
        for transaction in &block.transactions {
            if transaction.memo.starts_with(BRIDGE_CREDIT_MEMO_PREFIX) {
                let recipient = transaction.to.trim().to_uppercase();
                if !recipient.is_empty() {
                    funded.insert(recipient);
                }
            }
        }
    }
    funded
}

fn rebuild_account_nonces(blocks: &[Block]) -> Result<HashMap<String, u64>> {
    let mut nonces = HashMap::new();
    for block in blocks {
        for transaction in &block.transactions {
            if transaction.nonce == 0 {
                continue;
            }

            let expected = nonces
                .get(&transaction.from)
                .copied()
                .unwrap_or(0_u64)
                .checked_add(1)
                .ok_or_else(|| anyhow!("nonce overflow while rebuilding history"))?;
            if transaction.nonce != expected {
                return Err(anyhow!(
                    "invalid nonce sequence for {} in stored block {}: expected {}, found {}",
                    transaction.from,
                    block.block_height,
                    expected,
                    transaction.nonce
                ));
            }
            nonces.insert(transaction.from.clone(), transaction.nonce);
        }
    }
    Ok(nonces)
}

fn rebuild_dex_pools_from_blocks(blocks: &[Block]) -> HashMap<String, DexPool> {
    blocks
        .last()
        .map(|block| block.dex_pools.clone())
        .unwrap_or_default()
}

fn rebuild_token_registry_from_blocks(blocks: &[Block]) -> HashMap<String, Anrc20Token> {
    blocks
        .last()
        .map(|block| block.token_registry.clone())
        .unwrap_or_default()
}

fn sort_transactions_for_block(transactions: &mut [Transaction]) {
    transactions.sort_by(|left, right| {
        left.timestamp
            .cmp(&right.timestamp)
            .then_with(|| left.from.cmp(&right.from))
            .then_with(|| left.nonce.cmp(&right.nonce))
            .then_with(|| left.tx_hash.cmp(&right.tx_hash))
    });
}

fn reconcile_replay_sender_balances(
    accounts: &mut HashMap<String, AccountState>,
    blocks: &[Block],
) -> Result<()> {
    let mut simulation_accounts = accounts.clone();
    let mut sender_topups: HashMap<String, u64> = HashMap::new();

    for block in blocks {
        for transaction in &block.transactions {
            let sender = simulation_accounts
                .entry(transaction.from.clone())
                .or_insert_with(|| empty_account_state(transaction.from.clone()));
            let debit = transaction.total_debit()?;

            if sender.ants_balance < debit {
                let deficit = debit - sender.ants_balance;
                let topup = sender_topups.entry(transaction.from.clone()).or_insert(0);
                *topup = topup
                    .checked_add(deficit)
                    .ok_or_else(|| anyhow!("replay sender topup overflowed"))?;
                sender.ants_balance = sender
                    .ants_balance
                    .checked_add(deficit)
                    .ok_or_else(|| anyhow!("replay sender balance overflowed"))?;
                sender.activated_ants = sender.activated_ants.saturating_add(deficit);
                sender.total_activated_ants = sender.total_activated_ants.saturating_add(deficit);
            }

            sender.ants_balance = sender
                .ants_balance
                .checked_sub(debit)
                .ok_or_else(|| anyhow!("sender balance underflow while reconciling replay"))?;
            sender.activated_ants = sender.activated_ants.saturating_sub(debit);

            let recipient = simulation_accounts
                .entry(transaction.to.clone())
                .or_insert_with(|| empty_account_state(transaction.to.clone()));
            recipient.ants_balance = recipient
                .ants_balance
                .checked_add(transaction.amount_ants)
                .ok_or_else(|| anyhow!("recipient balance overflow while reconciling replay"))?;
        }

        distribute_fees(&mut simulation_accounts, block)?;
    }

    for (address, amount) in sender_topups {
        let account = accounts
            .entry(address.clone())
            .or_insert_with(|| empty_account_state(address.clone()));
        account.ants_balance = account
            .ants_balance
            .checked_add(amount)
            .ok_or_else(|| anyhow!("sender replay topup overflowed"))?;
        account.activated_ants = account.activated_ants.saturating_add(amount);
        account.total_activated_ants = account.total_activated_ants.saturating_add(amount);

        tracing::warn!(
            wallet = %address,
            amount_ants = amount,
            "replay reconciliation credited sender balance to satisfy historical block debits"
        );
    }

    Ok(())
}

fn empty_account_state(address: String) -> AccountState {
    AccountState {
        address,
        ants_balance: 0,
        activated_ants: 0,
        total_activated_ants: 0,
        sessions: 0,
        asset_balances: HashMap::new(),
    }
}

fn validate_block_sequence(blocks: &[Block]) -> Result<()> {
    let mut previous_hash = "GENESIS".to_owned();
    let mut previous_epoch_end = None;
    let mut label_mismatch_count = 0_usize;
    let mut first_label_mismatch: Option<(u64, usize)> = None;
    let mut last_label_mismatch: Option<(u64, usize)> = None;

    for (index, block) in blocks.iter().enumerate() {
        block.validate_structure()?;
        if !block.validate_hash()? {
            return Err(anyhow!(
                "stored block {} failed hash validation",
                block.block_height
            ));
        }
        // The cryptographic chain (each block's hash bound to the previous
        // block's hash via `previous_hash`) is the authoritative integrity
        // check. The `block_height` field is just a label; in early production
        // history a small number of blocks were minted with non-contiguous
        // height labels. We aggregate any mismatches into a single summary
        // line at the end so logs stay readable, but we do NOT refuse to load
        // the chain over a label mismatch — that would take a fully-intact,
        // hash-linked history offline for a cosmetic field.
        if block.block_height != index as u64 {
            label_mismatch_count += 1;
            if first_label_mismatch.is_none() {
                first_label_mismatch = Some((block.block_height, index));
            }
            last_label_mismatch = Some((block.block_height, index));
        }
        if block.previous_hash != previous_hash {
            return Err(anyhow!(
                "stored block {} has invalid previous hash linkage",
                block.block_height
            ));
        }
        if let Some(previous_epoch_end) = previous_epoch_end {
            if block.epoch_start < previous_epoch_end {
                return Err(anyhow!(
                    "stored block {} overlaps an earlier epoch",
                    block.block_height
                ));
            }
        }

        previous_hash = block.hash.clone();
        previous_epoch_end = Some(block.epoch_end);
    }

    if label_mismatch_count > 0 {
        let (first_stored, first_pos) = first_label_mismatch.unwrap_or((0, 0));
        let (last_stored, last_pos) = last_label_mismatch.unwrap_or((0, 0));
        tracing::warn!(
            count = label_mismatch_count,
            first_stored_height = first_stored,
            first_position = first_pos,
            last_stored_height = last_stored,
            last_position = last_pos,
            "block height labels diverge from linked-list position on {} legacy block(s); cryptographic chain is intact, continuing",
            label_mismatch_count
        );
    }

    Ok(())
}

fn apply_block(accounts: &mut HashMap<String, AccountState>, block: &Block) -> Result<()> {
    for transaction in &block.transactions {
        let sender = accounts
            .get_mut(&transaction.from)
            .ok_or_else(|| anyhow!("sender account not found while applying block"))?;
        let debit = transaction.total_debit()?;
        sender.ants_balance = sender
            .ants_balance
            .checked_sub(debit)
            .ok_or_else(|| anyhow!("sender balance underflow while applying block"))?;
        sender.activated_ants = sender.activated_ants.saturating_sub(debit);

        let recipient = accounts
            .entry(transaction.to.clone())
            .or_insert(AccountState {
                address: transaction.to.clone(),
                ants_balance: 0,
                activated_ants: 0,
                total_activated_ants: 0,
                sessions: 0,
                asset_balances: HashMap::new(),
            });
        recipient.ants_balance = recipient
            .ants_balance
            .checked_add(transaction.amount_ants)
            .ok_or_else(|| anyhow!("recipient balance overflow while applying block"))?;
    }

    distribute_fees(accounts, block)
}

fn distribute_fees(accounts: &mut HashMap<String, AccountState>, block: &Block) -> Result<()> {
    if block.total_fees_ants == 0 {
        return Ok(());
    }

    if block.miners.is_empty() {
        let reserve = accounts
            .entry(SYSTEM_FEE_RESERVE_ADDRESS.to_owned())
            .or_insert(AccountState {
                address: SYSTEM_FEE_RESERVE_ADDRESS.to_owned(),
                ants_balance: 0,
                activated_ants: 0,
                total_activated_ants: 0,
                sessions: 0,
                asset_balances: HashMap::new(),
            });
        reserve.ants_balance = reserve
            .ants_balance
            .checked_add(block.total_fees_ants)
            .ok_or_else(|| anyhow!("fee reserve overflowed"))?;
        return Ok(());
    }

    let remainder = block.total_fees_ants % block.miners.len() as u64;
    for miner in &block.miners {
        let account = accounts.entry(miner.clone()).or_insert(AccountState {
            address: miner.clone(),
            ants_balance: 0,
            activated_ants: 0,
            total_activated_ants: 0,
            sessions: 0,
            asset_balances: HashMap::new(),
        });
        account.ants_balance = account
            .ants_balance
            .checked_add(block.fee_per_miner)
            .ok_or_else(|| anyhow!("miner fee distribution overflowed"))?;
    }

    if remainder > 0 {
        let reserve = accounts
            .entry(SYSTEM_FEE_RESERVE_ADDRESS.to_owned())
            .or_insert(AccountState {
                address: SYSTEM_FEE_RESERVE_ADDRESS.to_owned(),
                ants_balance: 0,
                activated_ants: 0,
                total_activated_ants: 0,
                sessions: 0,
                asset_balances: HashMap::new(),
            });
        reserve.ants_balance = reserve
            .ants_balance
            .checked_add(remainder)
            .ok_or_else(|| anyhow!("fee reserve overflowed"))?;
    }

    Ok(())
}

pub fn format_anet_fixed(ants: u64) -> String {
    let whole = ants / ANTS_PER_ANET;
    let fractional = ants % ANTS_PER_ANET;
    format!("{whole}.{fractional:08}")
}

fn debit_anet(account: &mut AccountState, amount: u64) -> Result<()> {
    account.ants_balance = account
        .ants_balance
        .checked_sub(amount)
        .ok_or_else(|| anyhow!("insufficient ANET balance"))?;
    account.activated_ants = account.activated_ants.saturating_sub(amount);
    Ok(())
}

fn credit_anet(account: &mut AccountState, amount: u64) -> Result<()> {
    account.ants_balance = account
        .ants_balance
        .checked_add(amount)
        .ok_or_else(|| anyhow!("ANET balance overflowed"))?;
    Ok(())
}

fn debit_asset(account: &mut AccountState, symbol: &str, amount: u64) -> Result<()> {
    let balance = account.asset_balances.get(symbol).copied().unwrap_or(0);
    if balance < amount {
        return Err(anyhow!("insufficient {symbol} balance"));
    }
    let next = balance
        .checked_sub(amount)
        .ok_or_else(|| anyhow!("asset balance underflowed"))?;
    account.asset_balances.insert(symbol.to_owned(), next);
    Ok(())
}

fn credit_asset(account: &mut AccountState, symbol: &str, amount: u64) -> Result<()> {
    let balance = account.asset_balances.get(symbol).copied().unwrap_or(0);
    let next = balance
        .checked_add(amount)
        .ok_or_else(|| anyhow!("asset balance overflowed"))?;
    account.asset_balances.insert(symbol.to_owned(), next);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use chrono::{TimeZone, Utc};
    use serde_json::Value;

    use super::{rebuild_account_nonces, sort_transactions_for_block};
    use crate::block::Block;
    use crate::transaction::Transaction;

    fn tx(from: &str, nonce: u64, ts_seconds: i64, tx_hash: &str) -> Transaction {
        Transaction {
            tx_type: "transfer".to_owned(),
            from: from.to_owned(),
            to: "ANETFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF".to_owned(),
            amount_ants: 1,
            fee_ants: 1_000,
            nonce,
            memo: String::new(),
            timestamp: Utc
                .timestamp_opt(ts_seconds, 0)
                .single()
                .expect("valid timestamp"),
            chain_id: "anet-mainnet".to_owned(),
            payload: Value::Object(serde_json::Map::new()),
            signature: "sig".to_owned(),
            tx_hash: tx_hash.to_owned(),
        }
    }

    fn historical_block(
        block_height: u64,
        previous_hash: &str,
        transactions: Vec<Transaction>,
    ) -> Block {
        Block {
            block_height,
            epoch_start: Utc
                .timestamp_opt(block_height as i64 * 60, 0)
                .single()
                .expect("valid timestamp"),
            epoch_end: Utc
                .timestamp_opt(block_height as i64 * 60 + 30, 0)
                .single()
                .expect("valid timestamp"),
            previous_hash: previous_hash.to_owned(),
            hash: format!("block-hash-{block_height}"),
            transactions,
            activated_supply_ants: 0,
            total_fees_ants: 0,
            miners: Vec::new(),
            fee_per_miner: 0,
            block_event: None,
            events: Vec::new(),
            dex_pools: HashMap::new(),
            token_registry: HashMap::new(),
            tpow_proofs: Vec::new(),
            validator_votes: Vec::new(),
            state_root: String::new(),
            proof_root: String::new(),
        }
    }

    #[test]
    fn deterministic_ordering_uses_timestamp_sender_nonce_hash() {
        let mut transactions = vec![
            tx("ANETBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB", 2, 10, "bb"),
            tx("ANETAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", 2, 10, "cc"),
            tx("ANETAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", 1, 10, "dd"),
            tx("ANETAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", 1, 9, "ee"),
            tx("ANETAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", 2, 10, "aa"),
        ];

        sort_transactions_for_block(&mut transactions);

        let ordered = transactions
            .iter()
            .map(|item| {
                format!(
                    "{}:{}:{}:{}",
                    item.timestamp.timestamp(),
                    item.from,
                    item.nonce,
                    item.tx_hash
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            ordered,
            vec![
                "9:ANETAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA:1:ee",
                "10:ANETAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA:1:dd",
                "10:ANETAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA:2:aa",
                "10:ANETAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA:2:cc",
                "10:ANETBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB:2:bb",
            ]
        );
    }

    #[test]
    fn rebuild_account_nonces_from_history_tracks_latest_nonce_per_sender() {
        let blocks = vec![
            historical_block(
                0,
                "GENESIS",
                vec![
                    tx("ANETAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", 1, 1, "h1"),
                    tx("ANETBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB", 1, 1, "h2"),
                ],
            ),
            historical_block(
                1,
                "block-hash-0",
                vec![
                    tx("ANETAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", 2, 2, "h3"),
                    tx("ANETBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB", 2, 2, "h4"),
                    tx("ANETAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", 3, 2, "h5"),
                ],
            ),
        ];

        let nonces = rebuild_account_nonces(&blocks).expect("nonce rebuild succeeds");

        assert_eq!(
            nonces
                .get("ANETAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA")
                .copied(),
            Some(3)
        );
        assert_eq!(
            nonces
                .get("ANETBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB")
                .copied(),
            Some(2)
        );
    }

    #[test]
    fn rebuild_account_nonces_rejects_out_of_sequence_history() {
        let blocks = vec![historical_block(
            0,
            "GENESIS",
            vec![
                tx("ANETAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", 1, 1, "h1"),
                tx("ANETAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", 3, 1, "h2"),
            ],
        )];

        let error = rebuild_account_nonces(&blocks).expect_err("nonce gap must fail");
        assert!(error
            .to_string()
            .contains("invalid nonce sequence for ANETAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"));
    }

    fn account(addr: &str, sessions: u64) -> super::AccountState {
        super::AccountState {
            address: addr.to_owned(),
            ants_balance: 0,
            activated_ants: 0,
            total_activated_ants: 0,
            sessions,
            asset_balances: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn validator_compute_filters_below_min_sessions() {
        let mut accounts = std::collections::HashMap::new();
        accounts.insert("A".to_owned(), account("A", 1_000));
        accounts.insert("B".to_owned(), account("B", 999));
        accounts.insert("C".to_owned(), account("C", 5_000));

        let validators = super::compute_validators_from_accounts(&accounts);
        // B excluded (< MIN_SESSIONS_FOR_ANET); C first (more sessions), then A.
        assert_eq!(validators, vec!["C".to_owned(), "A".to_owned()]);
    }

    #[test]
    fn validator_compute_ties_broken_by_address_asc() {
        let mut accounts = std::collections::HashMap::new();
        accounts.insert("Z".to_owned(), account("Z", 2_000));
        accounts.insert("A".to_owned(), account("A", 2_000));
        accounts.insert("M".to_owned(), account("M", 2_000));

        let validators = super::compute_validators_from_accounts(&accounts);
        assert_eq!(
            validators,
            vec!["A".to_owned(), "M".to_owned(), "Z".to_owned()]
        );
    }

    #[test]
    fn validator_compute_caps_at_max_validators() {
        let mut accounts = std::collections::HashMap::new();
        // Insert MAX_VALIDATORS + 50 eligible accounts.
        let total = crate::activation::MAX_VALIDATORS + 50;
        for i in 0..total {
            let addr = format!("ANET{:040}", i);
            accounts.insert(addr.clone(), account(&addr, 1_000 + i as u64));
        }
        let validators = super::compute_validators_from_accounts(&accounts);
        assert_eq!(validators.len(), crate::activation::MAX_VALIDATORS);
    }

    #[test]
    fn validator_compute_is_deterministic() {
        let mut accounts = std::collections::HashMap::new();
        accounts.insert("X".to_owned(), account("X", 1_500));
        accounts.insert("Y".to_owned(), account("Y", 1_500));
        accounts.insert("W".to_owned(), account("W", 3_000));

        let a = super::compute_validators_from_accounts(&accounts);
        let b = super::compute_validators_from_accounts(&accounts);
        assert_eq!(a, b);
        assert_eq!(a[0], "W"); // highest sessions
    }

    #[test]
    fn bootstrap_backfills_when_no_organic_validators() {
        // Only bootstrap seats are eligible — they must keep the chain live.
        let mut accounts = std::collections::HashMap::new();
        accounts.insert("BOOT1".to_owned(), account("BOOT1", 1_000));
        accounts.insert("BOOT2".to_owned(), account("BOOT2", 1_000));
        let bootstrap = vec!["BOOT1".to_owned(), "BOOT2".to_owned()];

        let validators = super::compute_active_validator_set(&accounts, &bootstrap);
        assert_eq!(validators.len(), 2);
        assert!(validators.contains(&"BOOT1".to_owned()));
        assert!(validators.contains(&"BOOT2".to_owned()));
    }

    #[test]
    fn organic_validators_outrank_bootstrap_below_quorum() {
        // One organic validator + bootstrap seats, still below the sunset
        // quorum: organic is seated first, bootstrap backfills the rest.
        let mut accounts = std::collections::HashMap::new();
        accounts.insert("ORGANIC".to_owned(), account("ORGANIC", 5_000));
        accounts.insert("BOOT1".to_owned(), account("BOOT1", 1_000));
        let bootstrap = vec!["BOOT1".to_owned()];

        let validators = super::compute_active_validator_set(&accounts, &bootstrap);
        assert_eq!(validators[0], "ORGANIC"); // organic ranked first
        assert!(validators.contains(&"BOOT1".to_owned())); // bootstrap backfills
    }

    #[test]
    fn bootstrap_fully_retires_at_organic_quorum() {
        // Exactly the quorum of organic validators exists — every bootstrap
        // seat must drop out of the active set automatically.
        let mut accounts = std::collections::HashMap::new();
        for i in 0..super::BOOTSTRAP_SUNSET_ORGANIC_QUORUM {
            let addr = format!("ORG{:040}", i);
            accounts.insert(addr.clone(), account(&addr, 2_000));
        }
        accounts.insert("BOOT1".to_owned(), account("BOOT1", 1_000));
        accounts.insert("BOOT2".to_owned(), account("BOOT2", 1_000));
        let bootstrap = vec!["BOOT1".to_owned(), "BOOT2".to_owned()];

        let validators = super::compute_active_validator_set(&accounts, &bootstrap);
        assert_eq!(validators.len(), super::BOOTSTRAP_SUNSET_ORGANIC_QUORUM);
        assert!(!validators.contains(&"BOOT1".to_owned()));
        assert!(!validators.contains(&"BOOT2".to_owned()));
    }
}
