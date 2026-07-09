//! Build script for opendalfs-core.
//!
//! Generates the C header (`../src/include/rust.h`) from the `extern "C"`
//! surface using cbindgen. The C++ extension shell includes this header.

use std::env;
use std::path::PathBuf;

fn main() {
    let crate_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");

    // Emit the header into the C++ shell's include dir: <repo>/src/include/rust.h
    let out_path = PathBuf::from(&crate_dir)
        .join("..")
        .join("src")
        .join("include")
        .join("rust.h");

    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    let config = cbindgen::Config::from_file(PathBuf::from(&crate_dir).join("cbindgen.toml"))
        .expect("failed to read cbindgen.toml");

    match cbindgen::Builder::new()
        .with_crate(&crate_dir)
        .with_config(config)
        .generate()
    {
        Ok(bindings) => {
            bindings.write_to_file(&out_path);
        }
        Err(e) => {
            // Don't hard-fail the build if header generation hiccups; print so it's visible.
            println!("cargo:warning=cbindgen failed to generate rust.h: {e}");
        }
    }

    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=cbindgen.toml");

    // getrandom 0.3+ needs bcrypt on Windows.
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "windows" {
        println!("cargo:rustc-link-lib=bcrypt");
    }
}
