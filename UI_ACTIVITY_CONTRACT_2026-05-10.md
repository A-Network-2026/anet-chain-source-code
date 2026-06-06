# UI Activity Contract (Web + Inapp)

Purpose: keep frontend behavior and backend blockchain audit recording fully aligned.

## Endpoint

Primary endpoint:
POST /app/activity

Alias endpoint (same handler):
POST /ui/activity

## Versioned Schema Artifact

Authoritative schema file for CI and release validation:
- schemas/ui-activity/v1.schema.json

Versioning policy:
- Existing schema versions are immutable.
- Breaking changes require a new version file (for example: v2.schema.json).
- Non-breaking additions should still be reviewed and released with backend + UI together.

## Request Schema

Fields:
- source: required, string, one of: web, inapp
- action: required, string, lowercase snake_case, max 64 chars, allowed chars: a-z, 0-9, _
- wallet: optional, ANET wallet string
- screen: optional, string
- status: optional, string
- client_version: optional, string
- detail: optional, string
- auth: optional, SignedActionAuthorization

Validation rules:
- source must be exactly web or inapp
- action must match [a-z0-9_]+ and length 1..64
- if wallet is provided, request must include either:
  - valid signed auth for action type app_activity with matching wallet, or
  - matching authenticated explorer web session cookie

## On-Chain Recording Behavior

Every accepted request is recorded as one AppActivity event in block events.

Event mapping:
- event_type: AppActivity
- action attribute: ui_<action>
- wallet attribute: included when wallet is provided
- detail attribute: source plus optional metadata

Detail format example:
source=web;screen=swap;status=success;client_version=1.0.17;detail=quote_loaded

## Response

Accepted response:
- HTTP 202
- JSON:
  - status: accepted
  - source: normalized source (web or inapp)
  - action: normalized action

Example:
{
  "status": "accepted",
  "source": "inapp",
  "action": "swap_form_submit"
}

## Error Cases

- HTTP 400 if source or action is invalid
- HTTP 401 if wallet-scoped activity does not satisfy signed auth or matching session requirements

## Recommended Shared Action Names

Use these exact action names across web and inapp:
- page_open
- wallet_connect
- transfer_form_open
- transfer_form_submit
- swap_form_open
- swap_form_submit
- quote_request
- quote_success
- quote_failure
- tx_submit
- tx_success
- tx_failure
- login_submit
- login_success
- login_failure

## Web Example (session-auth wallet)

POST /app/activity
{
  "source": "web",
  "action": "swap_form_submit",
  "wallet": "ANET1234567890ABCDEF1234567890ABCDEF1234",
  "screen": "dex_swap",
  "status": "success",
  "client_version": "web-2026.05.10",
  "detail": "amount_in=10000000;pair=ANET_USDT"
}

## Inapp Example (signed auth wallet)

POST /app/activity
{
  "source": "inapp",
  "action": "tx_submit",
  "wallet": "ANET1234567890ABCDEF1234567890ABCDEF1234",
  "screen": "transfer",
  "status": "pending",
  "client_version": "ios-1.0.17",
  "detail": "kind=transfer",
  "auth": {
    "wallet": "ANET1234567890ABCDEF1234567890ABCDEF1234",
    "nonce": 27,
    "timestamp": "2026-05-10T09:30:00Z",
    "chain_id": "anet-mainnet",
    "payload": {
      "route": "app_activity"
    },
    "signature": "<65-byte-recoverable-signature-hex>",
    "action_hash": "<sha256-canonical-action-hash-hex>"
  }
}

## Consistency Policy

- Frontend teams must not invent ad-hoc action names in production.
- New action names should be reviewed once, then reused in both web and inapp.
- Keep source/action naming stable so analytics and chain audit queries remain deterministic.
- CI should validate payload fixtures against schemas/ui-activity/v1.schema.json before each release.

## CI Validation (Example)

If your UI CI uses Ajv CLI:

npm install --save-dev ajv-cli
npx ajv validate -s schemas/ui-activity/v1.schema.json -d path/to/ui-activity-payloads/*.json

Use any equivalent JSON Schema validator if your pipeline does not use Node.
