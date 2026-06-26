//! Generate a SQLite extension that bridges a DataFission shim
//! into SQLite as native functions / aggregates / types /
//! operators. See AGENTS.md for the target's quirks.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use shim_bridge_codegen_core::load_plan;

pub mod emit;
pub mod wasm_target;

/// Which output shape to emit. The native-dylib target produces
/// a `cdylib` Rust crate that embeds wasmtime and loads the
/// upstream composed shim wasm at SQLite extension-init time.
/// The wasm-component target produces a `cdylib` for
/// `wasm32-wasip2` that imports the upstream shim's WIT and
/// exports sqlink's WIT contract  composed against the shim
/// wasm via `wac plug` to produce one loadable wasm artifact.
///
/// Per PLAN-codegen-retarget.md (D3, 2026-06-26) the native
/// target is slated for removal once the wasm path covers the
/// full function surface (Phase 5). It stays today so the
/// existing native bridges keep regenerating cleanly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    NativeDylib,
    WasmComponent,
}

/// Generate a complete bridge crate from a shim-interface
/// SQLite database. Defaults to the native-dylib target.
///
/// Generated `.rs` files are run through `rustfmt --edition 2021`
/// at write time so the resulting crate is `cargo fmt --check`-
/// clean. Missing or failing `rustfmt` does not abort generation.
pub fn generate(interface_sqlite: &Path, out_dir: &Path) -> Result<()> {
    generate_with_target(interface_sqlite, out_dir, Target::NativeDylib)
}

/// Generate a complete bridge crate for the given target.
pub fn generate_with_target(
    interface_sqlite: &Path,
    out_dir: &Path,
    target: Target,
) -> Result<()> {
    let plan = load_plan(interface_sqlite)
        .with_context(|| format!("loading {}", interface_sqlite.display()))?;
    fs::create_dir_all(out_dir)
        .with_context(|| format!("creating {}", out_dir.display()))?;
    fs::create_dir_all(out_dir.join("src"))?;

    match target {
        Target::NativeDylib => emit_native_dylib(&plan, out_dir),
        Target::WasmComponent => wasm_target::emit(&plan, out_dir),
    }
}

fn emit_native_dylib(
    plan: &shim_bridge_codegen_core::BridgePlan,
    out_dir: &Path,
) -> Result<()> {
    fs::write(out_dir.join("Cargo.toml"), emit::cargo_toml(plan))?;
    let rust_files: &[(&str, String)] = &[
        ("src/lib.rs", emit::lib_rs(plan)),
        ("src/registry.rs", emit::registry_rs(plan)),
        ("src/scalars.rs", emit::scalars_rs(plan)),
        ("src/aggregates.rs", emit::aggregates_rs(plan)),
        ("src/table_functions.rs", emit::table_functions_rs(plan)),
        ("src/window_functions.rs", emit::window_functions_rs(plan)),
        ("src/types.rs", emit::types_rs(plan)),
        ("src/operators.rs", emit::operators_rs(plan)),
        ("src/casts.rs", emit::casts_rs(plan)),
        ("src/preprocessors.rs", emit::preprocessors_rs(plan)),
        ("src/system_catalog.rs", emit::system_catalog_rs(plan)),
        ("src/spatial_indexes.rs", emit::spatial_indexes_rs(plan)),
    ];
    let mut written: Vec<PathBuf> = Vec::with_capacity(rust_files.len());
    for (rel, body) in rust_files {
        let path = out_dir.join(rel);
        fs::write(&path, body)?;
        written.push(path);
    }
    fs::write(out_dir.join("README.md"), emit::readme(plan))?;

    rustfmt_files(&written);
    Ok(())
}

/// Run `rustfmt --edition 2021` against each file. Best-effort:
/// a missing or failing rustfmt logs to stderr and continues, so
/// the codegen still produces output usable as-is.
pub(crate) fn rustfmt_files(paths: &[PathBuf]) {
    for path in paths {
        let status = Command::new("rustfmt")
            .arg("--edition")
            .arg("2021")
            .arg(path)
            .status();
        match status {
            Ok(s) if s.success() => {}
            Ok(s) => eprintln!("[codegen] rustfmt {} exited with {s}", path.display()),
            Err(e) => {
                eprintln!("[codegen] rustfmt invocation failed for {}: {e}", path.display());
            }
        }
    }
}

/// Public re-exports so emit submodules don't have to re-import.
pub(crate) use shim_bridge_codegen_core as core;
