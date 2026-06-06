# Production Smoke Checklist (10 Minutes)

Use this immediately before and after deployment of ANET Layer 1 node.

## 0) Preflight (1 minute)

From repo root:

```powershell
Set-Location "e:\A Network Production\anet-chain-main"
cargo check
cargo test -- --nocapture
```

Pass criteria:
- Build succeeds
- Tests pass

## 1) Production safety flags (1 minute)

Verify environment values used by startup guard:

```powershell
$env:ANET_ENV
$env:APP_ENV
$env:ENVIRONMENT
$env:ANET_ALLOW_ADMIN_NATIVE_MINT
$env:ADMIN_ENDPOINTS_ENABLED
```

Pass criteria:
- Effective environment is production
- ANET_ALLOW_ADMIN_NATIVE_MINT is not truthy
- ADMIN_ENDPOINTS_ENABLED is not truthy unless temporary emergency window

## 2) Start node (2 minutes)

Default bind is 0.0.0.0:8080 unless PORT is set.

```powershell
Set-Location "e:\A Network Production\anet-chain-main"
$env:PORT = if ($env:PORT) { $env:PORT } else { "8080" }
cargo run -- --start-node
```

If you deploy with bootstrap in same process:

```powershell
cargo run -- --bootstrap --start-node
```

Pass criteria:
- Process starts without panic
- No startup error about unsafe production config

## 3) Liveness and readiness (2 minutes)

In a second terminal:

```powershell
$port = if ($env:PORT) { $env:PORT } else { "8080" }
Invoke-RestMethod "http://127.0.0.1:$port/health"
Invoke-RestMethod "http://127.0.0.1:$port/ready"
Invoke-RestMethod "http://127.0.0.1:$port/blocks"
```

Pass criteria:
- /health responds 200 with valid JSON
- /ready responds 200
- /blocks responds and does not error

## 4) Signed transaction rail check (2 minutes)

Confirm unsigned or malformed payload is rejected (defensive check):

```powershell
$port = if ($env:PORT) { $env:PORT } else { "8080" }
$body = @{
  tx_type = "transfer"
  from = "ANET1234567890ABCDEF1234567890ABCDEF1234"
  to = "ANETFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF"
  amount_ants = 1000
  fee_ants = 1000
  nonce = 1
  timestamp = "2026-05-10T08:00:00Z"
  chain_id = "anet-mainnet"
  payload = @{ memo = "smoke" }
  signature = ""
  tx_hash = ""
} | ConvertTo-Json -Depth 5

try {
  Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:$port/transactions" -ContentType "application/json" -Body $body
  Write-Host "Unexpected: unsigned tx accepted"
} catch {
  Write-Host "Expected rejection observed"
}
```

Pass criteria:
- Request is rejected (non-2xx)

## 5) Block progression sanity (2 minutes)

Check chain remains responsive and consistent under normal polling:

```powershell
$port = if ($env:PORT) { $env:PORT } else { "8080" }
Invoke-RestMethod "http://127.0.0.1:$port/explorer/api"
Invoke-RestMethod "http://127.0.0.1:$port/explorer/health"
```

Pass criteria:
- Endpoints respond
- No repeated panic/restart in server logs

## Go / No-Go decision

Go:
- All sections above pass

Protocol invariants pass:
- PI-1: Eligibility/policy gates enforced at execution time
- PI-4: No duplicate settlement for same request_id
- PI-5: Settlement evidence includes BSC tx hash and ANET activity event
- PI-6: Queue transitions are deterministic (`pending -> paid/failed/duplicate`)
- PI-8: Risk controls (caps/reserve/retries) remain active in live mode

No-Go:
- Startup guard fails in production
- /health or /ready fails
- Signed-route rejection behavior is incorrect
- Repeated runtime errors/panics in logs
- Any PI invariant breach (PI-1..PI-8)

## Rollback trigger

Rollback immediately if any of the following occur in first 15 minutes:
- Node crashes or restart loop
- Health/readiness unstable
- Signature or nonce protections behave unexpectedly
- Blocks endpoint or explorer health returns persistent 5xx
- Duplicate payout execution, missing dual audit evidence, or bypassed risk controls
