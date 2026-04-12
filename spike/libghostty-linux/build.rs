use std::env;
use std::path::PathBuf;

fn main() {
    let ghostty_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("temp/ghostty");
    let include_dir = ghostty_root.join("zig-out/include");
    let lib_dir = ghostty_root.join("zig-out/lib");

    println!("cargo:rerun-if-changed=src/wrapper.h");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=dylib=ghostty-vt");
    println!("cargo:rustc-link-lib=dylib=stdc++");

    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let bindings = bindgen::Builder::default()
        .header("src/wrapper.h")
        .clang_arg(format!("-I{}", include_dir.display()))
        .allowlist_function("ghostty_.*")
        .allowlist_type("Ghostty.*")
        .allowlist_var("GHOSTTY_.*")
        .generate()
        .expect("bindgen failed");
    bindings
        .write_to_file(out.join("bindings.rs"))
        .expect("bindings write failed");
}
