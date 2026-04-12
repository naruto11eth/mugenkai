# AGENTS.md

Notes for AI coding agents (Claude Code, GPT Codex, mugenkai's own buddy)
working in this repository.

## Stack

- **Host:** Rust 1.90+, edition 2024, gtk4-rs, libadwaita, glow, harfbuzz_rs
- **FFI quarantine:** `crates/host-ghostty-sys/` is the only crate where
  `unsafe` is allowed at the workspace level. All other crates set
  `unsafe_code = "deny"` via `[workspace.lints.rust]`.
- **Daemon:** TypeScript on Bun 1.3+, in `daemon/` (Bun workspaces)
- **Build glue:** Zig 0.15.2 (vendored ghostty submodule), `just` recipes

## Conventions

- Always use the `Justfile` recipes for build/test/lint. Don't invent new
  commands; add a recipe instead.
- Cargo recipes set `LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu` for bindgen.
- Never use `cat`, `grep`, `find` directly — use the Read, Grep, Glob tools.
- Never `git add -A` or commit anything unless explicitly asked.
- No `Co-Authored-By Claude` headers in commits.
- Edit existing files; only create new files when necessary.

## Where things are

- **Plan:** `docs/plans/2026-04-07-001-feat-mugenkai-linux-mvp-plan.md`
  (gitignored — local-only planning artifact)
- **Spike report:** `docs/solutions/2026-04-12-libghostty-linux-spike.md`
- **Reference repos** (read-only, gitignored): `temp/cmux/`, `temp/ghostty/`,
  `temp/iterm2/`, `temp/claude-code-fork/`
- **Vendored ghostty:** `vendor/ghostty/` (submodule, lands in Phase 1 Unit 2)

## Test policy

- Behavioral tests only. No tests that grep source files for substrings.
- No tests coupled to implementation shape (e.g. "this function calls X").
- Lifetime/leak tests for FFI types use `dhat`.
- Integration tests for the renderer read pixels from offscreen framebuffers.

## Code style

- Default to no comments. Comment WHY, never WHAT.
- Custom errors over `unwrap()` / `expect()` outside of tests.
- No `as any` analog (Rust: no `unsafe { transmute(...) }` outside FFI).
- Match the existing crate's naming conventions.
