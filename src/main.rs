//! CLI driver for sqlink-shim-codegen.
//!
//! Reads a shim-interface SQLite database and emits a SQLite
//! extension crate the user can then `cargo build` and load
//! into SQLite via `.load`.

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, ValueEnum};

use sqlink_shim_codegen::Target;

#[derive(Copy, Clone, Debug, ValueEnum)]
enum TargetArg {
    /// Emit a native `cdylib` Rust crate that embeds wasmtime and
    /// loads the upstream composed shim wasm at SQLite extension-
    /// init time. (Default; slated for removal per PLAN-codegen-
    /// retarget.md Phase 5 once the wasm target reaches function-
    /// count parity.)
    NativeDylib,
    /// Emit a `cdylib` Rust crate for `wasm32-wasip2` that imports
    /// the upstream shim's WIT and exports sqlink's WIT contract.
    /// The result composes against the shim wasm via `wac plug`
    /// to produce one loadable wasm artifact.
    WasmComponent,
}

impl From<TargetArg> for Target {
    fn from(a: TargetArg) -> Self {
        match a {
            TargetArg::NativeDylib => Target::NativeDylib,
            TargetArg::WasmComponent => Target::WasmComponent,
        }
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "sqlink-shim-codegen",
    about = "Generate a SQLite extension crate bridging a DataFission shim into SQLite."
)]
struct Args {
    /// Path to a shim-interface `.sqlite` (produced by
    /// `postgis-shim-interface` / `mobilitydb-shim-interface`).
    #[arg(long)]
    interface: PathBuf,

    /// Output directory for the generated bridge crate.
    /// Created if missing; existing files are overwritten.
    #[arg(long)]
    out: PathBuf,

    /// Which output shape to produce.
    #[arg(long, value_enum, default_value_t = TargetArg::NativeDylib)]
    target: TargetArg,
}

fn main() -> Result<()> {
    let args = Args::parse();
    sqlink_shim_codegen::generate_with_target(
        &args.interface,
        &args.out,
        args.target.into(),
    )?;
    eprintln!("Wrote bridge crate to {}", args.out.display());
    Ok(())
}
