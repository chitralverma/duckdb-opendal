//! Build script for duckdb-opendal.
//!
//! Generates the C header (`src/include/rust.h`) from the `extern "C"` surface
//! using cbindgen. The C++ extension shell (same `src/` dir) includes it.

use std::env;
use std::path::PathBuf;

fn main() {
    let crate_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");

    // C++ and Rust share this crate's src/; emit the header into src/include.
    let out_path = PathBuf::from(&crate_dir)
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
    println!("cargo:rerun-if-changed=src/uri.rs");
    println!("cargo:rerun-if-changed=cbindgen.toml");

    // Resolve the opendal version so the FFI can report it programmatically
    // (opendal exposes no public VERSION const). We read the *resolved* version
    // from Cargo.lock — not the Cargo.toml requirement — so it works no matter
    // how the dep is pinned (registry version, git rev/branch, or path). For a
    // git source we also append the short commit, e.g. "0.58.0 (git a1b2c3d)".
    let opendal_version =
        read_locked_version(&PathBuf::from(&crate_dir).join("Cargo.lock"), "opendal")
            .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=OPENDAL_VERSION={opendal_version}");
    println!("cargo:rerun-if-changed=Cargo.lock");
    println!("cargo:rerun-if-changed=Cargo.toml");

    // getrandom 0.3+ needs bcrypt on Windows.
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "windows" {
        println!("cargo:rustc-link-lib=bcrypt");
    }
}

/// Read the *resolved* version of package `name` from Cargo.lock.
///
/// Works for any pin kind. For a git source the short commit is appended
/// (`"<version> (git <sha7>)"`) so the reported version is unambiguous when the
/// crate version alone would not reflect the pinned ref.
fn read_locked_version(lock_path: &std::path::Path, name: &str) -> Option<String> {
    let contents = std::fs::read_to_string(lock_path).ok()?;
    let doc: toml::Table = toml::from_str(&contents).ok()?;
    let packages = doc.get("package")?.as_array()?;

    let pkg = packages.iter().find(|p| {
        p.as_table()
            .and_then(|t| t.get("name"))
            .and_then(|n| n.as_str())
            == Some(name)
    })?;
    let table = pkg.as_table()?;
    let version = table.get("version")?.as_str()?.to_string();

    // Append the short git commit if this is a git source.
    if let Some(source) = table.get("source").and_then(|s| s.as_str()) {
        if let Some(hash) = source.rsplit_once('#').map(|(_, h)| h) {
            if source.starts_with("git+") && !hash.is_empty() {
                let short: String = hash.chars().take(7).collect();
                return Some(format!("{version} (git {short})"));
            }
        }
    }
    Some(version)
}
