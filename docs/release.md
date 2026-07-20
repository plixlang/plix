# Release checklist

Publishing a Plix release (all four OS artifacts are produced by CI —
`.github/workflows/release.yml` — on clean GitHub runners: Ubuntu 22.04,
Windows 2022 MSVC, macOS 13 Intel, macOS 14 Apple Silicon).

```bash
# 1. green local battery
cargo build --release
bash tests/run_all.sh ./target/release/plix

# 2. version bump (keep in sync!)
#    - Cargo.toml            [package] version
#    - rt/Cargo.toml         [package] version
#    - src/main.rs           const VERSION
git add -A && git commit -m "release: v0.3.0"

# 3. tag & publish — CI builds all archives and drafts the GitHub release
git tag v0.3.0
git push origin main --tags

# 4. after CI finishes: review the drafted release notes, attach nothing
#    manually (the pipeline uploads the four archives automatically)

# optional: manual local Linux artifact the way dist/ was produced
PKG=plix-$(grep '^version' Cargo.toml | head -1 | cut -d'"' -f2)-x86_64-unknown-linux-gnu
mkdir -p dist/$PKG/bin
cp target/release/plix dist/$PKG/bin/ && cp README.md dist/$PKG/
cp -r docs examples dist/$PKG/
mkdir -p dist/$PKG/tests && cp -r tests/guards tests/run_all.sh dist/$PKG/tests/
tar czf dist/$PKG.tar.gz -C dist $PKG && rm -rf dist/$PKG
```

Post-release: bump versions to the next `-dev` number and carry on.
