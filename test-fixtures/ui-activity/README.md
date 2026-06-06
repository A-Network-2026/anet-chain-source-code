# UI Activity Test Fixtures

Test fixtures for JSON Schema validation of UI activity payloads.

## Directory Structure

- `valid/`: Valid payloads that must pass schema validation
  - `web-login-minimal.json`: Minimal web source with required fields only
  - `inapp-transaction-with-auth.json`: InApp source with signed authorization
  - `web-complex-with-detail.json`: Web source with detailed object metadata
  - `inapp-logout.json`: InApp logout action

- `invalid/`: Invalid payloads that must be rejected by schema validation
  - `missing-source.json`: Missing required `source` field
  - `missing-action.json`: Missing required `action` field
  - `invalid-source-enum.json`: Source not in allowed enum [web, inapp]
  - `invalid-action-uppercase.json`: Action contains uppercase letters (must be lowercase)
  - `invalid-action-with-special-chars.json`: Action contains invalid special characters
  - `action-too-long.json`: Action exceeds 64 character limit

## Schema Validation

The CI workflow validates all fixtures using [ajv-cli](https://ajv.js.org/) against `schemas/ui-activity/v1.schema.json`.

Run locally:
```bash
# Validate all valid fixtures
ajv validate -s schemas/ui-activity/v1.schema.json -d test-fixtures/ui-activity/valid/*.json

# Test that invalid fixtures are rejected
ajv validate -s schemas/ui-activity/v1.schema.json -d test-fixtures/ui-activity/invalid/*.json
# Should return errors for each invalid fixture
```

## Adding New Fixtures

When adding new UI activities to the application:

1. Create a valid example fixture in `valid/` with the format `{source}_{action}.json`
2. If the fixture introduces edge cases, add corresponding invalid example in `invalid/`
3. Ensure the payload matches the schema at `schemas/ui-activity/v1.schema.json`
4. Push and let CI validate automatically

## Contract Reference

See [../../../UI_ACTIVITY_CONTRACT_2026-05-10.md](../../../UI_ACTIVITY_CONTRACT_2026-05-10.md) for the full UI/backend activity contract and payload examples.
