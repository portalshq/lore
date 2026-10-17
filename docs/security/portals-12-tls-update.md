# Portals 12 TLS dependency update

This release candidate raises the rustls minimum to 0.23.46 and locks that version.
The 0.8.4 branch already contains h2 0.4.16. Together these address the vulnerabilities
reported by Portals Cloud's pinned-source audit: RUSTSEC-2026-0258,
RUSTSEC-2026-0285, RUSTSEC-2026-0346, RUSTSEC-2026-0348, and RUSTSEC-2026-0349.
No security audit exception is added.

`cargo audit --file Cargo.lock` verifies the dependency graph. Non-fatal unsound/yanked
warnings for other dependencies are reported separately and are not claimed fixed here.

Before publishing v0.8.4-portals.12, complete the release gate in AGENTS.md, including
the locked release builds, security-sensitive Rust tests, and CLI/server smoke tests.
The local gate requires at least 12 GiB of free disk. A lockfile audit alone is not
release acceptance. Do not reuse the portals.11 tag or deploy this candidate by branch.
After the new immutable release and signed manifests are available, Portals Cloud must
use its verify-and-promote flow to update the source gitlink and release metadata together.

The `Validate release candidate` pull-request workflow runs the same non-publishing
release gate on a clean hosted runner. It creates only a local release-branch name,
checks ancestry against the actual remote branch, and never pushes a branch or tag.
This provides the required build/test capacity when a developer machine lacks disk.
Publication and parent promotion remain explicit subsequent operations.

Release and lint builds use the pinned Rust 1.97.0 toolchain. This keeps candidate
validation and release artifacts on the same compiler rather than changing when
`stable` advances. Rust 1.99 introduces a Clippy diagnostic in tonic/async-trait
generated bindings; upgrading the compiler requires validating that generator
compatibility first. No Clippy or security-audit rule is disabled.

For hosted publication, dispatch the existing Release workflow on the release branch
with `publish=true`. Its publication job reruns `release-local.sh --publish`, then
explicitly dispatches the canonical artifact build on the new immutable tag. This
explicit dispatch is necessary because tags pushed by `GITHUB_TOKEN` do not start
push-triggered workflows. Normal tag runs and dispatches with `publish=false` only
build, test, sign, and publish artifacts; they cannot create tags.
