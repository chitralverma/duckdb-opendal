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

    // Resolve the opendal version from this crate's Cargo.toml so the FFI can
    // report it programmatically instead of a hardcoded string. opendal exposes
    // no public VERSION const. We read the declared dependency version (an exact
    // pin, per plan §2), not Cargo.lock, to keep it simple and dependency-light.
    let opendal_version =
        read_manifest_dep_version(&PathBuf::from(&crate_dir).join("Cargo.toml"), "opendal")
            .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=OPENDAL_VERSION={opendal_version}");
    println!("cargo:rerun-if-changed=Cargo.toml");

    // getrandom 0.3+ needs bcrypt on Windows.
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "windows" {
        println!("cargo:rustc-link-lib=bcrypt");
    }
}

/// Read the declared `version` of dependency `name` from a Cargo.toml.
///
/// Handles both the string form (`name = "1.2"`) and the table form
/// (`name = { version = "1.2", features = [...] }`).
fn read_manifest_dep_version(manifest_path: &std::path::Path, name: &str) -> Option<String> {
    let contents = std::fs::read_to_string(manifest_path).ok()?;
    let doc: toml::Table = toml::from_str(&contents).ok()?;
    let dep = doc.get("dependencies")?.get(name)?;
    match dep {
        toml::Value::String(s) => Some(s.clone()),
        toml::Value::Table(t) => t
            .get("version")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        _ => None,
    }
}
