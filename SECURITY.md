# Security Policy — A-Network L1 (anet-chain)

This is the security policy for the Rust L1 binary at `anet-chain/`.
For the BNB Chain contracts and frontend, see the SECURITY.md in the
`A-Network-2026` repository.

## Scope

| Component | Path | Notes |
|---|---|---|
| L1 consensus | `src/consensus.rs` | Canonical-hash + low-s + strict nonce. |
| L1 RPC | `src/rpc.rs` | Public JSON-RPC. Rate-limited. |
| Bridge EIP-712 verifier | `src/bridge_vault.rs` | Verifies signatures over the BSC vault's domain. |
| State + DB | `src/state.rs`, `src/db.rs` | Block & account storage. |
| Mining / activation | `src/activation.rs`, `src/dex.rs`, `src/token.rs` | Issuance, lifecycle gates. |

Out of scope: theoretical attacks without a working PoC against a
local devnet; DoS via your own client against your own node; issues
in dependencies without a working exploit path against the binary.

## How to report

**Do not open a public GitHub issue.**

Email: **security@a-network.dev**.

Include:

1. A clear description of the vulnerability.
2. The exact file and, if possible, the commit SHA.
3. A reproduction recipe against `cargo run -- --dev` or an
   equivalent local setup.
4. Your severity assessment and the asset at risk.
5. Preferred attribution.

## Response timeline

- **48 hours** for acknowledgement.
- **5 business days** for triage and severity assignment.
- **Critical** (chain halt, signature forge, fund loss via bridge):
  fix within 14 days, coordinated disclosure.
- **High** (significant correctness issue, non-fund): fix within 30
  days.
- **Medium / Low**: next release cycle.

## Validator-specific issues

If you are a Phase 2 validator (see `VALIDATOR_RECRUITMENT_SPEC.md`)
and you observe an L1 anomaly in the field — equivocation, missing
blocks, suspicious gossip — use the validator-only channel disclosed
to you at onboarding, not this public email.

## Safe harbor

Good-faith research is welcome. We commit to:

- Not pursue legal action for accidental, good-faith violations.
- Work with you on a coordinated fix.
- Credit you publicly if you wish.

Good-faith means: you stop at proof of vulnerability, do not move
real funds, do not disclose publicly before patch.

## Audit history

The L1 binary has not yet had a formal external audit. An audit
package analogous to `contracts/AUDIT_PACKAGE.md` is planned once
the contracts audit is complete.
