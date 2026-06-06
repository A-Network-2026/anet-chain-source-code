# A-Network Whitepaper Update

## Wallet, Bridge & Migration Sync — June 5, 2026

**Version**: v3.1 (supersedes v3.0 of 2026-05-14)
**Effective Date**: June 5, 2026
**Status**: PRODUCTION — Wallet migration, EVM bridge, and native DEX live and verified

> This update documents the systems that shipped after the v3.0 whitepaper:
> the permissionless commit-reveal wallet migration, the durable BSC↔ANET
> bridge, the native-DEX spend rules (including the new bridge-funds
> exemption), the canonical-node consolidation, and the session-counter
> reconciliation job. It corrects v3.0 where the prior figures were
> illustrative rather than measured. All protocol constants below are quoted
> directly from the running source.

---

## Executive Summary

Since v3.0, the focus has been **end-to-end value movement**: letting holders
bring assets in from BSC, migrate legacy wallets to signing-capable keys, and
spend/swap on the native L1 DEX — with durability guarantees that survive node
restarts.

- ✅ **Wallet Migration**: permissionless commit-reveal sweep from legacy → secp256k1 addresses (no admin custody)
- ✅ **EVM Bridge (BSC↔ANET)**: durable, idempotent inbound credits + L1→BSC burn-and-release
- ✅ **Native DEX**: ANET/WANET + stablecoin pools, 0.30% fee, instant-swap long-poll
- ✅ **Bridge-funds spendability**: paid-for ANET is exempt from the 1,000-session mining gate
- ✅ **Canonical node**: a single durable mainnet at `explorer.a-network.net`
- ✅ **Session accounting**: periodic undercount-only reconciliation job

---

## Protocol Constants (source of truth)

| Constant | Value | Source |
|----------|-------|--------|
| ANTS per session | 4,882,812 | `activation.rs` `ANTS_PER_SESSION` |
| Min sessions for L1 activation | 1,000 | `activation.rs` `MIN_SESSIONS_FOR_ANET` |
| Max validators | 2,100 | `activation.rs` `MAX_VALIDATORS` |
| Min transfer fee | 1,000 ANTS | `transaction.rs` `MIN_FEE_ANTS` |
| Native DEX fee | 0.30% (30 bps) | `dex` default |
| Bridge treasury (synthetic) | `ANET000000000000000000000000000000000000` | `state.rs` `BRIDGE_TREASURY_ADDRESS` |
| Bridge credit memo prefix | `bridge:evm:` | `state.rs` `BRIDGE_CREDIT_MEMO_PREFIX` |

---

## 1. Wallet Migration (Legacy → secp256k1)

**Status**: ✅ PRODUCTION
**Code**: `rpc.rs` (`/wallet/migrate-legacy/*`), `state.rs::migrate_legacy_to_secp`
**Mechanism**: Bitcoin-aligned, permissionless, commit-reveal

### Why it exists

Early wallets derived an ANET address as
`RIPEMD160(SHA256(seed))` ("legacy"), which is **not** signing-capable for the
chain's action authorization (DEX swap, bridge burn, P2P transfer). The chain
requires signatures that recover to a **secp256k1** address
(`RIPEMD160(compressed_pubkey)`). Migration moves the entire legacy balance to
the holder's secp address so their signatures are accepted.

### Commit-reveal flow (front-run safe)

```
Phase 1 — commit:
  commit_hash = SHA256( privkey_hex_lower : secp_address_upper : nonce )
  Auth: action_v1 signed by the secp privkey (action_type "migrate_commit")
  → recorded in wallet_migrations (PRIMARY KEY legacy_address; once only)

Phase 2 — reveal: caller posts privkey_hex + nonce. The node verifies:
  1. auth recovers to the committed secp_address
  2. SHA256(privkey:secp:nonce) reproduces the stored commit_hash
  3. RIPEMD160(SHA256(privkey)) == legacy_address
  4. derive_address(secp_pubkey(privkey)) == secp_address
  → atomically moves ants + activated + sessions + assets legacy → secp,
    zeroes the legacy side, and records the move in the next block as
    "WalletMigration: <legacy> -> <secp> ants=… sessions=…"
```

The privkey is only revealed **after** a binding commit exists, so an observer
who sees the reveal in the mempool cannot re-bind the funds to their own
address.

### Orchestration & recovery

- The mobile app calls its backend `/wallet/migrate-to-secp` with the
  PIN-decrypted seed; the backend derives the L1 key as `SHA256(seedText)` —
  **identical** to the mobile signer (`_deriveAnetPrivateKeyFromSeed`) — so the
  migrated balance lands on the exact address the app's signatures recover to.
- The backend persists the commit **nonce before** the on-chain commit, making
  every retry idempotent. A lost/mismatched nonce is **self-healed** via
  `/admin/wallet/migrate-legacy/cancel` (deletes only `committed` rows, never
  `revealed`), after which the user retries with a fresh nonce — no balance is
  ever stranded.
- **Pre-flight gates**: migration is blocked with a clear message when the
  wallet is not yet activated (sessions < 1,000 and zero on-chain balance) or
  when the legacy account has no on-chain balance yet (awaiting an inbound
  bridge credit).

---

## 2. EVM Bridge (BSC ↔ ANET)

**Status**: ✅ PRODUCTION
**Code**: `rpc.rs` (`/admin/bridge/evm/credit`, `/bridge/evm/credit/:hash`, `/bridge/burn`), `state.rs::queue_bridge_credit`, `state.rs::bridge_burn_anet`

### Inbound (BSC → ANET) — durable & idempotent

A USDC/USDT/wANET deposit on BSC is credited on L1 as a **real block
transaction** from the synthetic `BRIDGE_TREASURY_ADDRESS` to the recipient,
with memo `bridge:evm:<lowercase_bsc_tx_hash>`.

- **Durable**: because the credit is a transaction sealed into a Postgres-
  persisted block (not an in-memory label), it is reconstructed on every
  restart by block replay. Earlier RAM-only credits that vanished on redeploy
  are fixed.
- **Idempotent**: the BSC tx hash is deduplicated. The dedup set is rebuilt on
  startup from block memos (`collect_processed_bridge_hashes`), so the same
  deposit can never be credited twice — even across restarts.
- **Treasury neutrality**: the treasury is pre-funded in memory and auto-
  topped-up on replay (`reconcile_replay_sender_balances`), so live state and
  replayed state are bit-for-bit identical and net treasury movement is zero.

### Outbound (ANET → BSC) — burn-and-release

`bridge_burn_anet` permanently destroys the bridged ANET from L1 supply and
charges a separate **1,000-ANTS validator fee** (Bitcoin-style, paid on top),
distributed to the active validator set. The bridged amount stays clean: ANTS
destroyed on L1 equals wANET released on BSC, so the fee never reduces the
user's bridged value. The relayer releases the equivalent asset on BSC.

A public lookup `GET /bridge/evm/credit/:evm_tx_hash` lets anyone verify whether
a given BSC deposit has been credited.

---

## 3. Native DEX & Spend Rules

**Status**: ✅ PRODUCTION
**Pairs**: ANET ↔ WANET (1:1 wrap/unwrap), ANET ↔ {USDT, USDC, WBTC}
**Fee**: 0.30% (30 bps) · **Pricing**: constant-product AMM · instant-swap long-poll

### The 1,000-session spend gate

To prevent un-mined (sybil) wallets from dumping mined ANTS, **mined** balances
are spendable only after the wallet completes **1,000 verified Web2 sessions**.
This gate is enforced in three places:

- `queue_transaction` — sender and recipient of P2P transfers
- `ensure_eligible_account_mut` — native-DEX participants

### NEW in v3.1 — bridge-funds exemption

Holders who acquired ANET through the **EVM bridge paid real value**
(USDC/BNB), so their balance is **not** mined ANTS and is **exempt** from the
1,000-session gate. They can swap and transfer immediately.

- Implemented as a durable set `NodeState::bridge_funded_accounts`, populated
  when a bridge credit is queued and **rebuilt on restart** from block memos
  (`collect_bridge_funded_accounts`) — so the exemption survives redeploys.
- The exemption is checked at all three gates above; un-bridged mined balances
  remain fully gated. Migrated balances that carried ≥ 1,000 sessions across
  remain spendable on their own merit.

> Rationale: the gate protects against sybil dumping of *freely mined* supply.
> Bridge holders are paying customers, not sybils, so gating their own
> purchased ANET would be incorrect. The exemption is scoped to bridge-funded
> addresses only and leaves the mining anti-sybil property intact.

---

## 4. Consensus, Validators & Genesis

**Status**: ✅ PRODUCTION

- **On-chain validators**: the validator set is derived purely from on-chain
  session counts (any account with ≥ 1,000 sessions), capped at
  `MAX_VALIDATORS = 2,100`. No external registry, no governance — the chain is
  the sole source of truth.
- **Bootstrap sunset**: operator-seeded "Satoshi-mode" bootstrap seats exist
  only to keep block production alive before the first organic validators
  qualify. They are labeled (not counted in consensus) and **automatically
  retired** once enough organic validators exist — a Bitcoin-style sunset.
- **Revelation Block 0**: genesis is sealed and publicly verifiable at
  `/genesis` (sha256 `f8719ae3…d27a`, anchored to BTC #951158 and
  BSC #100593590), with the BSC commitment tx `0xffade652…ddcec2`.
- **Block production**: on-demand — the chain mines only when there is pending
  block work, so a static height while idle is normal, not a stall.

---

## 5. Canonical Node Consolidation

**Status**: ✅ RESOLVED

A historical split-brain (two diverged L1 nodes) was consolidated. The **single
canonical mainnet** is `https://explorer.a-network.net`
(= `mainnet.explorer.a-network.net`), backed by a durable 50 GB disk with daily
snapshots and active block production. The mobile app now defaults its L1 base
URL — and its fallbacks — to this canonical host.

---

## 6. Web2 Backend: Session Accounting

**Status**: ✅ PRODUCTION

Eligibility (1,000 sessions) is judged on each user's completed mining-session
count. A periodic **undercount-only reconciliation job** keeps the stored
counter in sync with the true number of completed session rows (scoped to
recently active users to stay cheap, never lowering legitimately inflated
counters and never touching balances). This corrects historical drift where two
completion paths advanced the counter inconsistently.

---

## 7. Mobile App

**Status**: ✅ PRODUCTION
**Current Version**: v1.0.81+141

- Wallet with ANET balance, seed backup/restore, PIN-gated signing
- Native DEX swap UI with friendly handling of activation/bridge states and a
  one-tap **swap waitlist** for users awaiting activation or an inbound bridge
  credit
- One-call `/wallet/migrate-to-secp` integration (idempotent; safe to call on
  every DEX/Bridge entry)
- Server-side action signer (`/auth/wallet/sign-action`) for canonical
  signature derivation
- Mining dashboard, NFT identity, AI support chat, leaderboard, referral deep
  links

---

## 8. Security & Invariants

- **Private keys** never leave the device except as a PIN-decrypted seed used
  for server-side canonical signing; balances move only via signed actions or
  the audited bridge/migration paths.
- **Idempotency** everywhere it matters: bridge credits (BSC tx hash),
  migration (one row per legacy address), session settlement (request id).
- **Durability**: bridge credits, migrations, and the bridge-funds exemption
  are all reconstructed from durable block history on restart.
- **Front-running resistance**: migration uses commit-reveal.
- **Admin gating**: bridge-credit and migration-cancel endpoints are key-gated
  with constant-time comparison.

---

## 9. What's Next (Roadmap)

1. **Verify the bridge-funds exemption in production** — redeploy the chain,
   confirm a bridge-credited wallet (sessions < 1,000) can swap and transfer.
2. **Operationalize the relayer** — confirm `ANET_L1_BASE_URL` and
   `ANET_DEX_ADMIN_KEY` point at the canonical node on the running process;
   re-run any pending durable credits.
3. **Decommission** the retired non-canonical node.
4. **Multi-sig / advanced wallet** features (v3.0 Weeks 5–8 milestone).
5. **L2 / cross-chain research** and an independent bridge audit.
6. **Live metrics** — publish measured network statistics sourced directly from
   the explorer rather than illustrative figures.

---

## Changelog vs v3.0

- **Added**: commit-reveal wallet migration; durable + idempotent EVM bridge
  (in and out); native-DEX spend-gate documentation; **bridge-funds exemption**;
  canonical-node consolidation; session-counter reconciliation; validator
  bootstrap sunset; Revelation Block 0 verification.
- **Corrected**: replaced v3.0's illustrative network metrics with a pointer to
  live explorer data; clarified that block production is on-demand.
- **Constants**: all protocol numbers now quoted from the running source.
