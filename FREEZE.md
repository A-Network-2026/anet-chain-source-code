# FEATURE FREEZE IN EFFECT

**Start:** 2026-05-14  
**End:** 2026-05-28  
**Status:** ACTIVE

## Policy
Only the following commit types are permitted during the freeze window:
- `fix:` — bug fixes reducing measurable risk
- `security:` — security hardening (timing-safe comparisons, input validation, etc.)
- `hardening:` — reliability improvements (error handling, idempotency)
- `chore:` — dependency pinning, CI/CD, monitoring

## Exception Whitelist
The following were approved before/during freeze:
- `security: constant-time admin key comparison` (2026-05-14, commit fc4cd9c) — approved: timing oracle fix

## Rejected During Freeze
- New chain features (new tx types, new RPC endpoints)
- Governance or tokenomics changes
- Any schema migration not required for a security fix

## Freeze Owner
Any exception to this policy requires explicit approval and must be logged above.
