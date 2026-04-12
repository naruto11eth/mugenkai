//! Build script for `host-ghostty-sys`.
//!
//! Responsibilities:
//! 1. Locate `vendor/ghostty/` (the pinned ghostty submodule).
//! 2. Run `zig build -Doptimize=ReleaseFast --prefix $OUT_DIR/ghostty-install`
//!    to produce `libghostty-vt.so` from ghostty's vendored sources.
//! 3. Emit `cargo:rustc-link-search` + `cargo:rustc-link-lib` directives so
//!    downstream crates link the resulting shared library.
//! 4. Run `bindgen` against `wrapper.h` (which `#include`s `<ghostty/vt.h>`)
//!    to generate Rust FFI types under `$OUT_DIR/bindings.rs`.
//!
//! Caching: zig has its own incremental build cache, so re-running `zig build`
//! is sub-second on subsequent invocations. We additionally tell cargo to
//! re-run only when ghostty's source files change.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("crates/host-ghostty-sys must live two levels under the workspace root")
        .to_path_buf();
    let ghostty_root = workspace_root.join("vendor").join("ghostty");
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR set by cargo"));
    let install_prefix = out_dir.join("ghostty-install");

    if !ghostty_root.join("build.zig").exists() {
        panic!(
            "vendor/ghostty submodule not initialized at {}\n\
             run: git submodule update --init --recursive\n\
             (or, before git init, clone manually: git clone https://github.com/ghostty-org/ghostty.git vendor/ghostty)",
            ghostty_root.display()
        );
    }

    let zig = locate_zig();
    println!("cargo:rerun-if-env-changed=ZIG");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=wrapper.h");
    println!(
        "cargo:rerun-if-changed={}/build.zig",
        ghostty_root.display()
    );
    println!(
        "cargo:rerun-if-changed={}/build.zig.zon",
        ghostty_root.display()
    );
    println!("cargo:rerun-if-changed={}/src", ghostty_root.display());

    let optimize = if env::var("PROFILE").as_deref() == Ok("release") {
        "ReleaseFast"
    } else {
        // Debug builds use ReleaseSafe to keep iteration fast while still
        // catching memory issues. Pure Debug zig builds of ghostty are very
        // slow and produce a 162MB+ binary; ReleaseSafe is the right balance
        // for development.
        "ReleaseSafe"
    };

    // -Demit-lib-vt=true sets defaults for a libghostty-vt-only build, which
    // disables xcframework, the macOS app, and docs. -Di18n=false drops the
    // gettext msgfmt pipeline so we don't need the GNU gettext package
    // installed on the host. See docs/solutions/2026-04-12-libghostty-linux-spike.md
    // for the broader reasoning about why we use libghostty-vt at all.
    let status = Command::new(&zig)
        .current_dir(&ghostty_root)
        .arg("build")
        .arg(format!("-Doptimize={optimize}"))
        .arg("-Demit-lib-vt=true")
        .arg("-Di18n=false")
        .arg("--prefix")
        .arg(&install_prefix)
        .status()
        .unwrap_or_else(|err| {
            panic!(
                "failed to spawn `{} build` in {}: {err}",
                zig.display(),
                ghostty_root.display()
            )
        });
    assert!(
        status.success(),
        "`zig build` in {} exited with {}",
        ghostty_root.display(),
        status
    );

    let lib_dir = install_prefix.join("lib");
    let include_dir = install_prefix.join("include");

    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=dylib=ghostty-vt");
    // libghostty-vt's pkg-config lists `Libs.private: -lc++`. On Linux GCC
    // toolchains we link `stdc++` instead. As of ghostty 557de7c the libc++
    // dependency may be reduced; we still link defensively because harfbuzz
    // and other vendored C++ libs need a C++ runtime.
    println!("cargo:rustc-link-lib=dylib=stdc++");

    // Re-export the install prefix so downstream code (and tests) can find
    // libghostty-vt.so at runtime via LD_LIBRARY_PATH or rpath.
    println!("cargo:libdir={}", lib_dir.display());
    println!("cargo:include={}", include_dir.display());

    // Embed an rpath so binaries find libghostty-vt.so without
    // LD_LIBRARY_PATH at dev time. AppImage builds (Phase 5) re-rpath
    // separately.
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib_dir.display());

    let bindings = bindgen::Builder::default()
        .header("wrapper.h")
        .clang_arg(format!("-I{}", include_dir.display()))
        .allowlist_function("ghostty_.*")
        .allowlist_type("Ghostty.*")
        .allowlist_var("GHOSTTY_.*")
        .allowlist_var("Ghostty.*_.*")
        .prepend_enum_name(false)
        .derive_default(true)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .generate()
        .expect("bindgen failed to generate libghostty-vt bindings");

    bindings
        .write_to_file(out_dir.join("bindings.rs"))
        .expect("failed to write bindings.rs");
}

fn locate_zig() -> PathBuf {
    if let Ok(path) = env::var("ZIG") {
        return PathBuf::from(path);
    }
    for candidate in [
        "/home/naruto/.local/bin/zig",
        "/usr/local/bin/zig",
        "/usr/bin/zig",
    ] {
        let p = PathBuf::from(candidate);
        if p.exists() {
            return p;
        }
    }
    PathBuf::from("zig")
}
