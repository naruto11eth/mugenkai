# mugenkai

> *Mugen (無限 infinite) + Kai (界 realm) = "infinite realm"*

A Linux-first AI/crypto-native terminal. Embeds ghostty's terminal core
(`libghostty-vt`) for VT/escape parsing, draws cells through a custom Rust
glyph renderer on GTK4 + libadwaita, and routes AI orchestration to a
sidecar daemon (`mugenkaid`) over a Unix-socket JSON-RPC protocol.

**Status: 0.1.0-dev — Phase 1 (foundation) in progress.** See the plan in
`docs/plans/` (workspace-local) and the libghostty Linux spike report in
`docs/solutions/` for current state.

## Quickstart (developers)

```sh
# 1. Install toolchains
mise install zig@0.15.2     # or grab the tarball from ziglang.org/download/0.15.2/
curl -fsSL https://bun.sh/install | bash
# rustup default stable (>= 1.90)

# 2. System deps (Ubuntu/Debian)
sudo apt install -y libgtk-4-dev libadwaita-1-dev libepoxy-dev libclang-dev

# 3. Build everything
just build

# 4. Run the host
just run
```

## Repository layout

```
mugenkai/
├── Cargo.toml             cargo workspace root
├── crates/
│   ├── host-cli/          binary entry point (mugenkai)
│   ├── host-core/         pure-logic crate: pane tree, config, protocol types
│   ├── host-ghostty-sys/  bindgen FFI to libghostty-vt.so (unsafe quarantine)
│   ├── host-ghostty/      safe Rust over host-ghostty-sys
│   ├── host-pty/          forkpty + tokio AsyncFd byte pump
│   ├── host-renderer/     GLArea + glow + harfbuzz_rs glyph renderer
│   └── host-ui/           gtk4 + libadwaita widgets
├── daemon/                bun workspace
│   ├── server/            mugenkaid: Unix socket + JSON-RPC dispatcher
│   ├── protocol/          shared TS protocol types
│   └── skills-runtime/    skill loader + runner
├── vendor/
│   └── ghostty/           submodule, vanilla ghostty-org/ghostty (Phase 1 Unit 2)
├── spike/
│   └── libghostty-linux/  Phase 0 spike, runnable proof
└── Justfile               canonical dev workflow
```

## License

MIT — see `LICENSE`.
