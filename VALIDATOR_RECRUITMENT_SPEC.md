# A-Network L1 — Validator Recruitment & Technical Spec

**Status:** Open call for independent validator operators.
**Tracker milestone:** #8 (L1 multi-validator set, 3+ independent operators).
**Prepared:** May 28, 2026.

This document is both the **technical specification** for running an
A-Network L1 validator and the **recruitment terms** under which the
project will federate the chain across at least three independent
operators. Closing milestone #8 requires three independent humans (or
organisations) running this binary, each from infrastructure the
A-Network founder does not control.

---

## 1. Why this milestone matters

The A-Network L1 chain is currently operator-run from a single
infrastructure footprint. That means:

- The chain's liveness depends on one provider.
- The chain's history is signed by one key.
- An L1 rewind would not be detectable by any third party that has
  not independently followed the chain.

This is acceptable for a small bootstrap network, but it is the
single largest **structural** decentralization gap in the
Decentralization Status Tracker. Every other "operator-run" item on
the tracker is a smart contract that can be progressively
decentralized by code. L1 federation requires **other people**.

Closing #8 is therefore a recruitment problem, not a code problem.

---

## 2. Target validator set

| Phase | Validators | BFT threshold | Quorum | Notes |
|---|---:|---|---|---|
| **Phase 1 (today)** | 1 | n/a | 1 | Operator-run. Liveness = single point of failure. |
| **Phase 2 (target)** | 3 | f=0, can survive 0 byzantine | 2-of-3 | Hits scorecard #8. **No byzantine fault tolerance yet** — three honest validators required. |
| **Phase 3** | 4 | f=1 | 3-of-4 | First BFT-capable configuration. Can survive 1 byzantine. |
| **Phase 4** | 7+ | f=2 | 5-of-7 | Production BFT. Geographic + jurisdictional diversity. |

Phase 2 (3 validators) closes the scorecard but is **not yet
byzantine-fault-tolerant**. We document this honestly in the tracker
when #8 flips to ✅; we do not claim BFT until Phase 3.

---

## 3. Who we are looking for (Phase 2 recruitment)

We need **two** independent validator operators in addition to the
A-Network founding operator. Ideal profile per operator:

- Experience running L1 / L2 validators (Ethereum, Cosmos, Polkadot,
  BNB Chain BSC validator, Solana, or comparable).
- Independent infrastructure: not on the same cloud account, not in
  the same datacenter region, not behind the same upstream ISP as the
  founding operator (currently Render, US-East).
- A real-name or pseudonymous-but-doxxable-to-counsel identity. We
  will publish the validator's name or handle on the tracker; we will
  hold legal identity confidentially with project counsel.
- Commitment to a **minimum 12-month** operating term with 30-day
  written notice on exit. This protects the network from a sudden
  drop below 2-of-3 quorum.
- Read this document and run a 7-day testnet validator successfully
  before signing the mainnet commitment.

We are **not** looking for:

- Stake-for-yield validators. There is no validator reward yet.
- Anonymous Discord handles without infrastructure history. Phase 4
  may relax this; Phase 2 cannot.
- Operators who insist on managed/custodial signing services. Each
  validator must hold its own keys on hardware it controls.

---

## 4. Compensation (Phase 2)

**There is no Phase 2 compensation.** Phase 2 validators run the
network as believers in the project. A-Network is a zero-budget
project: it has no cash treasury, and the founder wANET allocation
is reserved for its published economic functions and will **not** be
issued as discretionary validator grants. We will not promise what
we cannot honestly deliver.

What Phase 2 validators receive:

- **Public attribution** as a Phase 2 validator on the
  Decentralization Status Tracker, on a validator dashboard
  (`/validators.html`, planned), in the project README, and in the
  whitepaper's contributors section.
- **A signed letter of acknowledgement** on project letterhead if
  needed for a portfolio, conference CV, or employment.
- **Priority eligibility** for the Phase 3+ protocol-level block
  reward when it ships. Phase 2 operators who are still in the
  active set at the time of the Phase 3 launch will be the first
  three validators to receive protocol rewards by default.
- **Governance seat** on protocol upgrade decisions during Phase 2
  (each active validator has equal voice; founding operator does
  not have a casting vote).

If protocol-reward compensation in Phase 3+ is a hard requirement
for you, Phase 2 is not the right entry point. Wait for Phase 3.

---

## 5. Technical specification

### 5.1 Binary

The validator binary is the standard `anet-chain` Rust build:

```bash
git clone https://github.com/A-Network-2026/anet-chain
cd anet-chain
cargo build --release
./target/release/anet-chain --help
```

Source: `anet-chain` repository, MIT-licensed. Reproducible build
via `Cargo.lock` pin and `rust-toolchain.toml`. Auditable per
upcoming separate L1 audit track.

### 5.2 Hardware

Minimum for Phase 2:

- **CPU:** 4 vCPU (modern x86_64, AVX2 required).
- **RAM:** 8 GB.
- **Disk:** 200 GB NVMe SSD with sustained 500 MB/s.
- **Network:** 100 Mbps symmetric, < 100 ms latency to the other
  validators, public static IP.
- **OS:** Linux (Ubuntu 22.04 LTS or Debian 12 reference).

Recommended for production:

- 8 vCPU / 16 GB RAM / 1 TB NVMe / 1 Gbps with BGP-diverse upstream.

### 5.3 Key custody

Each validator holds **two** keys:

- **Block-signing key** — online, on the validator node. Rotatable.
  Loss = liveness incident, not a safety incident.
- **Stake / identity key** — offline, hardware wallet, controls the
  validator's on-chain registration and the vesting grant address.
  Loss = recoverable via the vault's 2-of-3 admin path.

The signing key **must not** be on the same machine as the
operator's day-to-day shell. Use a dedicated VM or bare-metal box.

### 5.4 Network ports

| Port | Direction | Purpose |
|---|---|---|
| 26656/tcp | inbound | P2P (gossip, block propagation) |
| 26657/tcp | local-only | RPC (do not expose publicly) |
| 9090/tcp | inbound (peers only) | Validator-set comms |
| 9100/tcp | local-only | Prometheus metrics scrape |

Firewall everything else. RPC and metrics must not be reachable from
the public internet.

### 5.5 Monitoring & SLO

Per-validator SLOs:

- **Uptime:** ≥ 99.5% per calendar month.
- **Time to incident response:** ≤ 30 minutes on PagerDuty-equivalent.
- **Missed-block rate:** ≤ 1% over a 24h sliding window.

Operators expose Prometheus on `:9100` (local-only) and forward
metrics to the A-Network observability pool (read-only credentials
issued at onboarding).

### 5.6 Slashing & faults

Phase 2 does not enforce on-chain slashing. Faults are handled by
governance:

- 3 missed-block-rate breaches in 30 days → written notice.
- 5 breaches in 30 days → removal from the active set, grant vesting
  paused until cured.
- Equivocation (double-sign) → immediate removal, grant forfeited,
  evidence published.

Phase 3 introduces protocol-level slashing.

### 5.7 Upgrades

- All validators run the same release tag at any point in time.
- Coordinated upgrades on a 14-day public notice.
- Emergency upgrades via 2-of-3 admin sig + immediate notice.
- Each release tag is signed by the founding operator and the
  release SHA-256 is published in the GitHub release notes.

---

## 6. Onboarding sequence

| Step | Owner | Duration |
|---|---|---|
| 1. Read this spec, file an intent-to-validate via the SECURITY.md disclosed channel. | Candidate | open |
| 2. KYC-equivalent identity exchange with project counsel. | Candidate + counsel | ~1 week |
| 3. Sign the Phase 2 validator agreement (12-month term, SLOs, slashing-by-governance). | Candidate + project | ~1 week |
| 4. Run testnet validator for 7 days with green SLOs. | Candidate | 1 week |
| 5. Receive mainnet bootstrap config + peer list. | Project | same-day |
| 6. Bring up mainnet validator, attest first block signed. | Candidate | 1 day |
| 7. Tracker entry created. Grant vesting starts. | Project | same-day |

Total clock from intent to active mainnet validator: **3–4 weeks**.

---

## 7. Closing milestone #8

Milestone #8 flips from ⏳ PLANNED to ✅ DONE when:

1. Three independent operators are signing blocks on mainnet.
2. Each operator is publicly listed on the tracker with attribution.
3. The chain has produced **at least 30 consecutive days** of blocks
   with the 2-of-3 quorum, with no quorum-loss incident.

The honest accompanying note in the tracker will read: "3 independent
operators, 2-of-3 quorum. Not yet byzantine-fault-tolerant; BFT
requires Phase 3 (4 validators, 3-of-4). Phase 3 is the next planned
milestone after #8."

---

## 8. Contact

- Intent-to-validate and all sensitive operator communication: via
  the disclosed channel in `SECURITY.md`.
- Public discussion: project Discord / X.
- Testnet credentials and mainnet onboarding config: issued only
  after step 3 of §6.

---

## 9. Open questions (community input welcome)

- Should the Phase 3 protocol-level block reward be paid in L1 ANET
  (newly minted under the 21M cap) or in wANET (from existing
  founder allocation, requires a separate vault release)? Default:
  L1 ANET so it is genuinely protocol-native and does not draw from
  the founder allocation.
- Should validator-set changes require a 48h timelock at the L1
  level analogous to the vault contract? Default: yes for Phase 3,
  not enforced at Phase 2 because the founder still operates the
  rotation manually.
- Should we require validators to also run a wANET signer daemon for
  the BSC vault? Default: encouraged but not required, to keep the
  operator skill profile small in Phase 2.
