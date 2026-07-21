#!/usr/bin/env bash
# ============================================================================
# Plix v0.9.6 Release Script
# ============================================================================
# This script prepares the repository for a GitHub release.
# Run it from the project root: bash scripts/release.sh
#
# Prerequisites:
#   - Git configured with push access to https://github.com/plixlang/plix
#   - All changes committed and pushed
#   - Cargo.lock up to date
# ============================================================================

set -euo pipefail

VERSION="0.9.6"
TAG="v${VERSION}"
REPO="plixlang/plix"

echo "╔══════════════════════════════════════════════╗"
echo "║   Plix ${VERSION} Release Preparation          ║"
echo "╚══════════════════════════════════════════════╝"
echo ""

# --- Step 1: Verify git repo ---
echo "📋 Step 1: Verifying git repository..."
if [ ! -d .git ]; then
    echo "❌ Not a git repository. Initialize one first:"
    echo "   git init"
    echo "   git remote add origin https://github.com/${REPO}.git"
    exit 1
fi
echo "   ✅ Git repository found"

# --- Step 2: Verify version consistency ---
echo ""
echo "📋 Step 2: Verifying version consistency..."
CARGO_VER=$(awk -F '"' '/^version =/ { print $2; exit }' Cargo.toml)
RT_VER=$(awk -F '"' '/^version =/ { print $2; exit }' rt/Cargo.toml)

if [ "$CARGO_VER" != "$VERSION" ]; then
    echo "❌ Cargo.toml version is ${CARGO_VER}, expected ${VERSION}"
    exit 1
fi
if [ "$RT_VER" != "$VERSION" ]; then
    echo "❌ rt/Cargo.toml version is ${RT_VER}, expected ${VERSION}"
    exit 1
fi
echo "   ✅ Version ${VERSION} consistent across Cargo.toml and rt/Cargo.toml"

# --- Step 3: Verify Cargo.lock exists ---
echo ""
echo "📋 Step 3: Verifying Cargo.lock..."
if [ ! -f Cargo.lock ]; then
    echo "❌ Cargo.lock not found. Run: cargo generate-lockfile"
    exit 1
fi
echo "   ✅ Cargo.lock exists"

# --- Step 4: Build and test ---
echo ""
echo "📋 Step 4: Building and testing..."
cargo build --release --locked 2>&1
echo "   ✅ Release build successful"

cargo test --workspace --locked 2>&1
echo "   ✅ Unit tests passed"

cargo build --locked 2>&1
echo "   ✅ Debug build successful"

bash tests/run_all.sh 2>&1
echo "   ✅ Integration tests passed"

bash tests/fuzz_parity.sh 2>&1
echo "   ✅ Fuzz parity passed"

# --- Step 5: Verify CHANGELOG ---
echo ""
echo "📋 Step 5: Verifying CHANGELOG.md..."
if grep -q "## \[${VERSION}\]" CHANGELOG.md; then
    echo "   ✅ CHANGELOG.md has entry for ${VERSION}"
else
    echo "❌ CHANGELOG.md missing entry for ${VERSION}"
    exit 1
fi

# --- Step 6: Verify no uncommitted changes ---
echo ""
echo "📋 Step 6: Checking for uncommitted changes..."
if [ -n "$(git status --porcelain)" ]; then
    echo "⚠️  Uncommitted changes detected:"
    git status --short
    echo ""
    echo "   Commit all changes before releasing. Example:"
    echo "   git add -A"
    echo "   git commit -m 'release v${VERSION}: docker, security, docs, lsp, wasm, ffi modules + WASM codegen'"
    exit 1
fi
echo "   ✅ No uncommitted changes"

# --- Step 7: Create and push tag ---
echo ""
echo "📋 Step 7: Creating git tag ${TAG}..."
if git tag -l "${TAG}" | grep -q "${TAG}"; then
    echo "⚠️  Tag ${TAG} already exists. Delete it first if you want to re-release:"
    echo "   git tag -d ${TAG}"
    echo "   git push origin :refs/tags/${TAG}"
    exit 1
fi

git tag -a "${TAG}" -m "Plix ${VERSION}

New stdlib modules: docker, security, docs, lsp, wasm, ffi
WASM codegen backend (plix build --target wasm)
FFI with dlopen/dlsym and zero-copy buffers
LSP server with completion, diagnostics, hover, formatting
Bug fixes: WASM say() digit order, negative numbers, LEB128 encoding
"

echo "   ✅ Tag ${TAG} created"

echo ""
echo "╔══════════════════════════════════════════════╗"
echo "║   Ready to publish!                          ║"
echo "╚══════════════════════════════════════════════╝"
echo ""
echo "Run these commands to publish:"
echo ""
echo "  # 1. Push the commit and tag to GitHub"
echo "  git push origin main"
echo "  git push origin ${TAG}"
echo ""
echo "  # 2. GitHub Actions will automatically:"
echo "  #    - Run all tests (CI)"
echo "  #    - Build release binaries for Linux, Windows, macOS"
echo "  #    - Create a GitHub Release with downloadable archives"
echo ""
echo "  # 3. The release will appear at:"
echo "  #    https://github.com/${REPO}/releases/tag/${TAG}"
echo ""
echo "  # Optional: If you need to create the release manually"
echo "  # gh release create ${TAG} --title \"Plix ${VERSION}\" --notes-file CHANGELOG.md"
echo ""
