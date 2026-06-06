# ANET Payout Worker Deployment Guide (Render)

## Overview

The payout worker can run on Render as a background cron job instead of on your local machine.

This provides:
- ✅ 99.9% uptime (no local machine dependency)
- ✅ Automatic restarts on failure
- ✅ Centralized logs
- ✅ Scheduled runs via cron (every 5 minutes)

## Protocol Invariants v1 (Operator Contract)

The worker must remain compliant with these production invariants at all times:

- **PI-1 (Policy Gate):** No swap/transfer/cashout settlement executes unless eligibility checks pass at execution time.
- **PI-2 (Single Activation Trigger):** First-time NFT profile activation occurs only after at least one successful authenticated settlement event.
- **PI-3 (Execution Separation):** Public web interfaces expose market data and quotes; execute paths remain wallet-gated.
- **PI-4 (Idempotent Settlement):** Duplicate payout execution for the same request identifier is forbidden across retries/restarts.
- **PI-5 (Dual Audit Evidence):** Successful payout must emit both settlement-chain evidence (BSC tx hash) and ANET `payout_sent` activity.
- **PI-6 (Deterministic Queue State):** Records transition only through `pending -> paid/failed/duplicate` and stay replay-safe after crashes.
- **PI-7 (Key Isolation):** Seed/private key material is never transported in public API payloads.
- **PI-8 (Risk Envelope):** Hourly caps, per-user daily limits, reserve ratio checks, and retry limits are mandatory.

## Prerequisites

1. Render account with access to your existing services
2. Your anet-chain repo connected to Render
3. PAYOUT_EXECUTOR_API_KEY (from payout-executor service)

## Setup Instructions

### Option A: Add to Existing Render Project (Recommended)

1. **In Render Dashboard**:
   - Go to your A Network project
   - Click "New" → "Background Worker"
   - Connect your anet-chain repo
   - Branch: main

2. **Configure Worker**:
   - Name: `anet-payout-worker`
   - Root Directory: `scripts/payout-automation`
   - Build Command: `npm install`
   - Start Command: `node worker.js`
   - Instance Type: Starter
   - Plan: $7/month

3. **Set Environment Variables**:
   ```
   CONFIG_PATH=/var/data/config.json
   QUEUE_PATH=/var/data/queue.json
   STATE_PATH=/var/data/state.json
   BASE_URL=https://anet-private-mainnet.onrender.com
   PAYOUT_EXECUTOR_URL=https://anet-payout-executor.onrender.com/execute
   PAYOUT_EXECUTOR_API_KEY=<your-secret-key>
   SCHEDULE_INTERVAL_MINUTES=5
   AUTO_INGEST_ENABLED=true
   AUTO_INGEST_URL=<your-authorized-backend-feed>
   AUTO_INGEST_API_KEY=<optional-feed-key>
   ```

4. **Deploy** and watch logs for first run

### Option B: Use render-worker.yaml

1. In your Render dashboard, import `scripts/payout-automation/render-worker.yaml` as a service blueprint

2. Fill in secrets (PAYOUT_EXECUTOR_API_KEY)

3. Deploy

## Shared Storage (Required for Production)

The worker reads/writes:
- `config.json` (configuration)
- `queue.json` (pending payouts)
- `state.json` (processed tracking)

For these to persist across worker restarts, you have two options:

### Option 1: Use Render Persistent Disk (Required for Production)
- In Render dashboard, add a Persistent Disk to the worker
- Mount path: `/var/data`
- Set file paths to `/var/data/config.json`, `/var/data/queue.json`, `/var/data/state.json`

### Option 2: Use Remote State (Advanced)
- Store config/queue/state in a database or remote S3
- Worker fetches state on start, writes back after run

## Monitoring

1. **Logs**: Check Render dashboard "Logs" tab for worker output
2. **Status**: `GET https://anet-payout-executor.onrender.com/status` shows payout counts
3. **Failures**: Render sends alerts if worker crashes

4. **Invariant checks**:
   - Ensure no duplicate payout for same `request_id` in logs and status history (PI-4)
   - Ensure successful payout shows BSC tx hash and ANET `payout_sent` event (PI-5)
   - Ensure queue transitions only to valid states (`pending -> paid/failed/duplicate`) (PI-6)
   - Ensure reserve/cap checks are not bypassed in live mode (PI-8)

## Disabling Local Machine Task

Once worker is running reliably on Render:

```powershell
# Stop and remove Windows task
Unregister-ScheduledTask -TaskName ANET_AutoPayoutWorker -Confirm:$false
```

## Testing Before Production

1. Deploy worker with `PAYOUT_EXECUTOR_API_KEY` set
2. Confirm persistent disk is mounted at `/var/data`
3. Confirm `AUTO_INGEST_ENABLED=true` and feed returns valid candidates
4. Wait one cycle (5 minutes) or trigger a manual run
5. Check executor `/status` for payout count increase
6. Verify logs show idempotency checks and valid queue state transitions
7. Verify BSC tx hash and ANET `payout_sent` activity event are both present

## Rollback

If issues arise:
1. Keep local Windows task running as fallback
2. Pause the Render worker (don't delete)
3. Investigate logs
4. Re-enable when fixed

If any Protocol Invariant (PI-1..PI-8) is violated, classify as **No-Go**, pause worker execution, and restore last known safe config/state from persistent storage snapshot before resuming.

## Cost Impact

- Payout Worker (Starter): $7/month
- Payout Executor (already deployed): $7/month
- **Total**: $14/month for both services

---

**Status**: Ready for production deployment
