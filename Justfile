# Mugenkai dev workflow. Single canonical entry point for all builds and checks.
# Run `just` with no args to see this list.

set shell := ["bash", "-cu"]
set dotenv-load := true

# Toolchain locations (overridable via env)
zig := env_var_or_default("ZIG", "/home/naruto/.local/bin/zig")
libclang_path := env_var_or_default("LIBCLANG_PATH", "/usr/lib/x86_64-linux-gnu")
bun := env_var_or_default("BUN", "/home/naruto/.bun/bin/bun")

# Make sure user-local toolchains are on PATH for inner script invocations
# (bun, zig, cargo all install to ~/.bun, ~/.local/bin, ~/.cargo respectively)
export PATH := "/home/naruto/.bun/bin:/home/naruto/.local/bin:/home/naruto/.cargo/bin:" + env_var("PATH")

# Default: list recipes
default:
    @just --list

# ─── Build ────────────────────────────────────────────────────────────

# Build everything (rust crates + bun packages)
build: build-rust build-bun

# Build the Rust workspace
build-rust:
    LIBCLANG_PATH={{libclang_path}} cargo build --workspace

# Install bun deps and typecheck
build-bun:
    cd daemon && {{bun}} install
    cd daemon && {{bun}} run typecheck

# Build libghostty-vt from the vendored ghostty submodule (Phase 1 Unit 2)
vendor-build:
    @if ! command -v {{zig}} >/dev/null; then \
        echo "zig not found at {{zig}} — install zig 0.15.2 (mise install zig@0.15.2 or download from ziglang.org/download/0.15.2/)" >&2; \
        exit 1; \
    fi
    @if [ ! -d vendor/ghostty/.git ] && [ ! -f vendor/ghostty/build.zig ]; then \
        echo "vendor/ghostty submodule not initialized — run: git submodule update --init --recursive" >&2; \
        exit 1; \
    fi
    cd vendor/ghostty && {{zig}} build -Doptimize=ReleaseFast

# ─── Run ──────────────────────────────────────────────────────────────

# Run the host binary
run:
    LIBCLANG_PATH={{libclang_path}} cargo run -p host-cli

# Run the daemon binary
run-daemon:
    cd daemon && {{bun}} run --cwd server start

# ─── Test ─────────────────────────────────────────────────────────────

# Run all tests (rust + bun)
test: test-rust test-bun

test-rust:
    LIBCLANG_PATH={{libclang_path}} cargo test --workspace

test-bun:
    #!/usr/bin/env bash
    set -eu
    cd daemon
    # bun test errors when there are no *.test.ts files yet. Tolerate that
    # until daemon tests start landing in Phase 3.
    if find . -name "*.test.ts" -not -path "./node_modules/*" | grep -q .; then
        {{bun}} test
    else
        echo "[test-bun] no *.test.ts files yet; skipping"
    fi

# ─── Lint ─────────────────────────────────────────────────────────────

# Format + lint everything
lint: lint-rust lint-bun

lint-rust:
    cargo fmt --all -- --check
    LIBCLANG_PATH={{libclang_path}} cargo clippy --workspace --all-targets -- -D warnings

lint-bun:
    cd daemon && {{bun}} run typecheck

# Auto-fix formatting
fmt:
    cargo fmt --all
    cd daemon && {{bun}} x prettier --write "**/*.{ts,json,md}" || true

# ─── Distribution ─────────────────────────────────────────────────────

# Build the AppImage (Phase 5 Unit 14)
appimage:
    @echo "AppImage builder lands in Phase 5 Unit 14"
    @exit 1

# ─── Housekeeping ─────────────────────────────────────────────────────

# Clean all build outputs
clean:
    cargo clean
    rm -rf daemon/node_modules
    rm -rf vendor/ghostty/zig-out vendor/ghostty/.zig-cache

# Print toolchain versions
versions:
    @echo "rustc:    $(rustc --version)"
    @echo "cargo:    $(cargo --version)"
    @echo "zig:      $({{zig}} version 2>/dev/null || echo 'NOT FOUND')"
    @echo "bun:      $({{bun}} --version 2>/dev/null || echo 'NOT FOUND')"
    @echo "libclang: {{libclang_path}}"
