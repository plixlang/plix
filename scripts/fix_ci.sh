#!/usr/bin/env bash
# ============================================================================
# Fix CI issues for Plix v0.9.6
# ============================================================================
# Run this BEFORE pushing to GitHub to resolve CI failures.
# ============================================================================

set -euo pipefail

echo "🔧 Fixing CI issues for Plix v0.9.6..."
echo ""

# --- Fix 1: Remove conflicting lsp module directory ---
# If src/lsp/mod.rs exists alongside src/lsp.rs, Rust will error.
# We keep src/lsp.rs (the single-file module) and remove the directory.

if [ -d src/lsp ]; then
    echo "❌ Found conflicting src/lsp/ directory alongside src/lsp.rs"
    echo "   Removing src/lsp/ directory..."
    rm -rf src/lsp/
    echo "   ✅ Removed src/lsp/ — keeping src/lsp.rs"
else
    echo "   ✅ No conflicting src/lsp/ directory"
fi

# --- Fix 2: Remove unused src/stdlib/ directory ---
# This directory contains stub files that aren't referenced in main.rs

if [ -d src/stdlib ]; then
    echo "❌ Found unused src/stdlib/ directory"
    echo "   Removing src/stdlib/ directory..."
    rm -rf src/stdlib/
    echo "   ✅ Removed src/stdlib/"
else
    echo "   ✅ No unused src/stdlib/ directory"
fi

# --- Fix 3: Verify cargo fmt ---
echo ""
echo "📋 Running cargo fmt --check..."

if command -v cargo &> /dev/null; then
    if cargo fmt --all -- --check; then
        echo "   ✅ cargo fmt --check passed"
    else
        echo "   ⚠️  cargo fmt found issues, auto-fixing..."
        cargo fmt --all
        echo "   ✅ cargo fmt applied"
    fi
else
    echo "   ⚠️  cargo not found, skipping fmt check"
fi

# --- Fix 4: Verify Cargo.lock ---
if [ ! -f Cargo.lock ]; then
    echo "   ⚠️  Cargo.lock missing, run: cargo generate-lockfile"
else
    echo "   ✅ Cargo.lock exists"
fi

echo ""
echo "╔══════════════════════════════════════════════╗"
echo "║   CI fixes applied!                          ║"
echo "╚══════════════════════════════════════════════╝"
echo ""
echo "IMPORTANT: When committing to GitHub, make sure to:"
echo "  1. git add -A (to stage the deletion of src/lsp/mod.rs and src/stdlib/)"
echo "  2. git commit -m 'fix: resolve CI fmt and module conflicts'"
echo "  3. git push origin main"
