//! `--target wasm-component` emitter.
//!
//! Produces a Rust crate compilable for `wasm32-wasip2` as a
//! `cdylib`. The component imports the upstream shim's WIT
//! interfaces (the same ones the hand-written
//! `extensions/postgis-bridge` in sqlink consumes) and exports
//! the canonical `sqlite:extension/minimal`-shape contract
//! (metadata + scalar-function + aggregate-function + vtab).
//!
//! Phase 1 (per `docs/plans/PLAN-codegen-retarget.md`): all
//! exported function bodies are STUBS — they return an error
//! `Result::Err("<bridge>: function '<name>' is stubbed in
//! Phase 1")` so the load + compose + register path is
//! exercisable end-to-end without real dispatch. Phase 2
//! replaces the stubs with the marshaling logic from the
//! hand-written bridge.
//!
//! Layout produced under `out_dir`:
//!
//! ```text
//! Cargo.toml
//! README.md
//! src/lib.rs
//! wit/world.wit
//! wit/deps/postgis-wasm/...        (vendored from postgis-bridge)
//! wit/deps/sfcgal-component/...    (vendored from postgis-bridge)
//! wit/deps/sqlite-extension/...    (vendored from postgis-bridge)
//! ```
//!
//! The WIT `deps/` are vendored at codegen time from the
//! hand-written postgis-bridge crate — the source of truth for
//! the import surface. Phase 4+ moves to fetching WIT from the
//! interface DB or from upstream tags.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::core::BridgePlan;
use crate::rustfmt_files;

mod emit_cargo;
mod emit_lib;
mod emit_readme;
mod emit_wit;

/// Entry point invoked from `lib.rs::generate_with_target`.
pub fn emit(plan: &BridgePlan, out_dir: &Path) -> Result<()> {
    let crate_name = crate_name_for(plan);

    fs::create_dir_all(out_dir.join("src"))?;
    fs::create_dir_all(out_dir.join("wit"))?;
    fs::create_dir_all(out_dir.join("wit/deps"))?;

    // Cargo.toml
    fs::write(out_dir.join("Cargo.toml"), emit_cargo::cargo_toml(plan, &crate_name))?;

    // WIT (world + vendored deps).
    emit_wit::write_world(plan, &out_dir.join("wit/world.wit"))?;
    emit_wit::write_deps(plan, &out_dir.join("wit/deps"))
        .context("emitting wit/deps/")?;

    // src/lib.rs
    let lib_rs_path = out_dir.join("src/lib.rs");
    fs::write(&lib_rs_path, emit_lib::lib_rs(plan, &crate_name))?;

    // README.md
    fs::write(out_dir.join("README.md"), emit_readme::readme(plan, &crate_name))?;

    // rustfmt the emitted Rust source. Best-effort.
    let to_fmt: Vec<PathBuf> = vec![lib_rs_path];
    rustfmt_files(&to_fmt);

    Ok(())
}

/// Compose the crate name from the primary extension. PostGIS
/// becomes `postgis-sqlink-bridge`; the `-sqlink-` segment
/// disambiguates the wasm-component bridges from the existing
/// native-dylib `postgis-sqlite-bridge` crate.
pub(crate) fn crate_name_for(plan: &BridgePlan) -> String {
    let primary = plan
        .extensions
        .first()
        .map(|e| e.name.as_str())
        .unwrap_or("shim");
    format!("{}-sqlink-bridge", sanitize(primary))
}

pub(crate) fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}
