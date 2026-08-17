# Portals Lore release notes

## Supported distribution

This fork supplies the only Lore CLI and server supported by Nap. Local and
Portals Cloud modes use the same `portalshq/lore` release. Do not add an
automatic or silent fallback to `EpicGames/lore`; upstream remains configured
only for reviewing and rebasing future upstream changes.

## Version and tag policy

- Use a distinct SemVer prerelease such as `0.8.4-portals.1` in
  `[workspace.package].version`.
- Release tags are exactly `v<workspace-version>`.
- Tags are immutable. Never move, force-update, or reuse a release tag. If a
  release needs any change, increment the `portals.N` suffix.
- A branch name is not a release pin. Production consumers pin the immutable
  release tag, its source commit, and the signed artifact manifest.

## Local release gate

From a clean checkout on the release branch, run:

```bash
scripts/release-local.sh v0.8.4-portals.1
```

The gate verifies that the tag matches the workspace version, builds `lore`
and `loreserver` from the same commit, runs the security-sensitive Rust tests,
and runs the CLI/server smoke integration suite against those release
binaries. It requires at least 12 GiB of free disk space and does not create or
push a tag. It fetches `origin`, requires the matching base branch (for example
`0.8.4`), and refuses a branch that does not contain the latest remote commit.

After reviewing the result, publish explicitly:

```bash
scripts/release-local.sh v0.8.4-portals.1 --publish
```

Publishing reruns every gate, pushes the current branch, creates a new
annotated tag, and pushes that tag. It refuses an existing local or remote tag.
Pushing the tag starts `.github/workflows/release.yml`, which reruns tests,
builds platform artifacts, creates `SHA256SUMS`, signs that manifest through
GitHub OIDC/Sigstore, and creates the GitHub Release. The installer consumes
the GitHub Release assets, not a branch archive.

When the workflow finishes, promote the release from the parent repository:

```bash
infra/pulumi/scripts/verify-and-promote-lore-client-release.sh \
  v0.8.4-portals.1
```

That verification is the only supported way to populate the Lore client
release fields in `infra/lore/versions.yaml`: it resolves the immutable tag to
its commit and records `source_commit`, tag, checksums, and signature URLs
automatically. Commit the resulting parent gitlink and release
bill-of-materials update together. Then update Nap's pinned Lore version and
provenance, run Nap's local and cloud integration suites, and publish a new Nap
release.

## Required checks

- `cargo test --locked --release -p lore-credential -p lore-server`
- `scripts/run-smoke-tests.sh` (all smoke tests, split into bounded-disk batches)
- The release workflow's cross-platform artifact builds and signed manifest
- The parent repository's Lore release verification script

Do not describe a tag by itself as proof of compatibility. Compatibility is
established by the immutable tag, the two local binaries built from the same
commit, passing tests, and the verified signed release manifest together.
