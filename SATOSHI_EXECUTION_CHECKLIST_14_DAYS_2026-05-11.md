# Satoshi-Style Execution Checklist (14 Days)

Date: 2026-05-11  
Owner: A Network Core Team  
Mode: Reliability and trust-minimization sprint (no net-new product features)

## Sprint Activation

- Status: Active
- Activated on: 2026-05-11
- Current phase: Day 1
- Execution baseline:
  - SATOSHI_SPRINT_DAY1_KICKOFF_2026-05-11.md
  - PI_RELEASE_EVIDENCE_TEMPLATE.md

## Mission

Lock protocol behavior into enforceable operations, remove single-operator risk, publish verifiable evidence, and prove failure handling before scaling.

## Rules Of The Sprint

- Freeze net-new user features for 14 days unless required for critical security fixes.
- Every deploy must pass protocol gates (PI checks) before release.
- No single operator controls payout trigger, signer, and state mutation in one step.
- Every payout success must produce dual evidence: settlement tx hash and ANET activity event.
- Any PI breach is immediate No-Go and triggers incident flow.

## Protocol Gates (Must Stay True)

- PI-1: Eligibility and policy checks must pass at execution time.
- PI-2: First NFT profile activation only after successful authenticated settlement event.
- PI-3: Public web remains data/quote oriented; execute paths are wallet-gated.
- PI-4: Idempotency forbids duplicate settlement for same request identity.
- PI-5: Dual audit evidence required for successful payout.
- PI-6: Queue transitions only pending -> paid/failed/duplicate.
- PI-7: Signed-only production flow; no public transmission of seed/private key material.
- PI-8: Risk envelope (caps, reserve ratio, retries) is mandatory.

## Team Roles (Assign Named People)

- Incident Commander (IC): owns incident timeline and Go/No-Go decisions.
- Protocol Owner: owns PI gate implementation and release acceptance.
- Payout Ops Owner: owns worker config, queue health, reserve/cap checks.
- Signer Custodian: owns signer controls and rotation process.
- Auditor: independently verifies payout evidence and publishes daily report.

### Initial Role Assignment Sheet

| Role | Primary | Backup | Status |
|---|---|---|---|
| Incident Commander (IC) | UNASSIGNED | UNASSIGNED | ACTION REQUIRED |
| Protocol Owner | UNASSIGNED | UNASSIGNED | ACTION REQUIRED |
| Payout Ops Owner | UNASSIGNED | UNASSIGNED | ACTION REQUIRED |
| Signer Custodian | UNASSIGNED | UNASSIGNED | ACTION REQUIRED |
| Auditor | UNASSIGNED | UNASSIGNED | ACTION REQUIRED |

## 14-Day Plan

### Day 1-2: Lock Invariants Into Release Gates

- Add release checklist item requiring proof for PI-1..PI-8.
- Define mandatory artifacts for each release:
  - smoke output
  - payout status snapshot
  - dual-evidence sample
  - incident rollback readiness check
- Set No-Go condition: missing evidence for any PI gate.

Exit criteria:

- Release template updated and used once in a dry run.
- IC, Protocol Owner, Auditor sign off.

### Day 3-4: Remove Single-Operator Risk

- Split responsibilities so one person cannot execute full payout lifecycle alone.
- Introduce approval step for production config changes.
- Require two-party review for signer/config/limit changes.

Exit criteria:

- Access matrix documented.
- At least one simulated change executed with two-party control.

### Day 5-6: Verifiable Daily Transparency

- Publish machine-readable daily operations report with:
  - payouts_attempted
  - payouts_paid
  - payouts_failed
  - payouts_duplicate_blocked
  - reserve_ratio
  - cap_usage_hourly
  - cap_usage_daily_top_users
  - dual_audit_coverage_percent
- Store report immutably (append-only path) and keep 30-day history.

Exit criteria:

- Two consecutive days of generated reports.
- Auditor cross-checks one random payout from report.

### Day 7-8: Adversarial Replay And Failure Drills

- Run replay tests for duplicate request_id and swap_reference.
- Simulate worker restart during pending payout.
- Simulate delayed upstream feed and malformed candidate payload.
- Simulate reserve-ratio breach and verify automatic block behavior.

Exit criteria:

- All drills have expected outcomes with logs.
- Any unexpected behavior is opened as Sev issue.

### Day 9-10: Emergency Policy And Pause/Resume Runbook

- Define explicit pause triggers:
  - PI breach
  - unexplained duplicate settlement
  - dual-audit evidence missing
  - reserve/cap control bypass
- Define resume requirements:
  - root cause identified
  - fix deployed
  - backfill audit completed
  - IC and Protocol Owner approval

Exit criteria:

- Emergency runbook approved.
- Tabletop incident executed end-to-end.

### Day 11-12: Signer Hardening

- Move toward multi-party signing control (or staged approval before live execution).
- Rotate API keys and signer secrets under change control.
- Verify signer source and key lineage in status outputs.

Exit criteria:

- Rotation drill completed in non-disruptive window.
- Post-rotation payouts validated with dual evidence.

### Day 13-14: Go-Live Readiness Review

- Run full production smoke using updated PI gates.
- Review 14-day evidence bundle.
- Decide Go/No-Go for scaling payout volume.

Exit criteria:

- All PI gates passed for last 72 hours.
- No unresolved Sev incidents.
- IC signs readiness statement.

## Daily Command Cadence (Operations)

- 00:00 UTC: Generate daily report and archive evidence.
- Every cycle: Validate queue transition integrity and idempotency state.
- Every 4 hours: Check reserve ratio and payout cap envelope.
- End of day: Auditor verifies sample payouts and signs log.

## No-Go Matrix

Immediate No-Go if any condition is true:

- PI gate breach (PI-1..PI-8)
- Duplicate settlement confirmed
- Missing tx hash or missing payout_sent audit event on successful payout
- Reserve ratio below configured minimum with execution still continuing
- Unauthorized config/signer change without two-party control

## Evidence Bundle (Keep Daily)

- Worker logs for all cycles
- Executor status snapshot
- Queue/state checksums
- Sample tx hashes and matching payout_sent activity IDs
- Incident notes and remediation entries

## Success Definition After 14 Days

- Protocol behavior is enforced operationally, not only documented.
- No single person can unilaterally trigger unsafe production actions.
- Every payout has independently verifiable evidence.
- Team can pause, diagnose, recover, and resume safely under pressure.
