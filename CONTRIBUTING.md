# Contributing to anet-chain

The A-Network L1 chain is open source under the MIT License (see
`LICENSE`). Anyone can run a node, run a validator (see
`VALIDATOR_RECRUITMENT_SPEC.md`), and submit improvements.

## Before you start

- **Security issues** go through `SECURITY.md`, not pull requests.
- **Consensus changes** require a written design doc and project
  sign-off before any code is written. Do not surprise us with a
  consensus PR.
- **Validator recruitment** is a separate process documented in
  `VALIDATOR_RECRUITMENT_SPEC.md`.

## What we accept

- Bug fixes with a regression test.
- Performance improvements that preserve consensus byte-for-byte and
  include a benchmark.
- New RPC tests, fuzz harnesses, property tests.
- Documentation, especially clarifications to `DEPLOY.md` and the
  validator spec.
- Tooling improvements (build scripts, CI, observability).

## What we do not accept

- Consensus changes without a design doc + sign-off.
- New external dependencies without justification.
- Refactors that are not byte-for-byte behaviour-preserving for the
  consensus surface.
- Speculative features that anticipate a Phase 3+ validator set we
  have not yet recruited.

## Process

1. **Open an issue first.** Describe the problem, the proposed fix,
   and the test plan.
2. **One logical change per PR.**
3. **Tests are required** for code changes. `cargo test` must pass.
   Consensus-touching changes additionally require a property test
   or fuzz target.
4. **Commit messages** follow `<scope>(<area>): <summary>` with a
   body explaining *why*.
5. **Sign off** with a real name or stable handle.

## Style

- `cargo fmt --check` and `cargo clippy -- -D warnings` must pass.
- Match the existing module structure. New top-level modules need
  justification in the PR description.
- No `unsafe` without a written safety comment and reviewer
  approval.
- Prefer `thiserror` for error types in line with current usage.

## Review

- Consensus or bridge-verifier PRs require **two** project
  reviewers.
- Everything else requires one reviewer.
- We will tell you clearly when something is good and merge it. We
  will also be explicit about why something is rejected.

## CLA

No CLA. By submitting a PR you confirm you have the right to
license your contribution under MIT.

## Questions

Public discussion: project Discord / X.
Anything sensitive: per `SECURITY.md`.
