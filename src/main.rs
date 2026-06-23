//! CLI driver for sqlink-shim-codegen.
//!
//! Reads a shim-interface SQLite database and emits a SQLite
//! extension crate the user can then `cargo build` and load
//! into SQLite via `.load`.

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;

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
}

fn main() -> Result<()> {
    let args = Args::parse();
    sqlink_shim_codegen::generate(&args.interface, &args.out)?;
    eprintln!("Wrote bridge crate to {}", args.out.display());
    Ok(())
}
