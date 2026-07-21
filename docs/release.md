# Release process

This checklist is the release contract for maintainers. A GitHub tag must not
be pushed until the candidate has passed its local preflight and the release
notes have been reviewed.

## 1. Prepare the release branch

1. Create `release/vX.Y.Z` from the approved development commit.
2. Update `CHANGELOG.md` with the release date and user-visible changes.
3. Update both package versions in lockstep:

   - `Cargo.toml` → `[package].version`
   - `rt/Cargo.toml` → `[package].version`

   The CLI version is automatically derived from `Cargo.toml`. Do not add a
   separate hand-maintained version constant.
4. Update migration notes, support matrix, and documentation if behavior or
   platform support changed.

## 2. Verify locally

Use a clean checkout with the supported stable Rust toolchain:

```sh
cargo fmt --all -- --check
cargo check --workspace --locked
cargo test --workspace --locked
cargo build --release --locked
bash tests/release_preflight.sh ./target/release/plix
# Final candidate: larger differential test sample
bash tests/fuzz_parity.sh 150 ./target/release/plix
```

Inspect failures rather than weakening tests. The preflight checks package and
CLI version consistency, Plix test suites, interpreter/native parity, and a
deterministic fuzz sample.

## 3. Review release evidence

Before tagging, confirm:

- the working tree contains only intended release changes;
- the changelog and release notes describe compatibility impact and known
  limitations honestly;
- required CI checks are green;
- supported-target artifacts and checksums can be produced; and
- any security, audit, performance, or compatibility claim links to its scope
  and evidence.

## 4. Tag and publish

```sh
git add -A
git commit -m "release: vX.Y.Z"
git tag -a vX.Y.Z -m "Plix vX.Y.Z"
git push origin release/vX.Y.Z --follow-tags
```

After maintainer review/merge as appropriate, the GitHub Actions release
workflow packages supported targets and creates the GitHub release from the
tag. Review the draft release, its generated notes, archives, checksums, and
installation smoke-test output before publishing it.

## 5. Post-release

- Verify downloads and checksums from the public release page.
- Smoke-test each supported archive on its target platform.
- Record any discovered release issue and publish an advisory when necessary.
- Start the next development cycle with a documented version plan; do not
  silently change a released tag or replace release artifacts.
