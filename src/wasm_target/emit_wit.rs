//! Emit the WIT world + vendored deps for the wasm-component
//! bridge.
//!
//! The world imports the upstream shim's interfaces and exports
//! the canonical `sqlite:extension/*` surface. The vendored
//! `deps/` directory holds the dependency WIT packages that
//! `wit-bindgen::generate!` resolves at build time.
//!
//! ## Where the dep WIT comes from (Phase 1)
//!
//! The hand-written `extensions/postgis-bridge/wit/` directory
//! in the sqlink tree is the source of truth for the import
//! surface (postgis:wasm/*, sfcgal:component/*, sqlite:extension/*).
//! Phase 1 vendors those files verbatim into the generated
//! crate. Per `docs/plans/PLAN-codegen-retarget.md` D1, the
//! generator runs sibling to sqlink, so the path is well-known.
//!
//! Override via `SQLINK_POSTGIS_BRIDGE_WIT` if the tree lives
//! elsewhere; otherwise the generator searches
//! `~/git/sqlink/extensions/postgis-bridge/wit/deps/` (the
//! canonical layout).
//!
//! Phase 4+ replaces this vendoring with WIT derived from the
//! interface DB itself. Phase 1 takes the shortcut because the
//! Phase 1 deliverable is "the SHAPE compiles + composes" — the
//! WIT surface is the same surface the hand-written bridge
//! already proved.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};

use crate::core::BridgePlan;

/// Write `wit/world.wit` at the given path.
pub fn write_world(plan: &BridgePlan, dest: &Path) -> Result<()> {
    // Phase 1: postgis is the only shim. When mobilitydb lands
    // (Phase 4) we widen this dispatch to inspect plan.extensions
    // for the shim name and pick the right world template.
    let primary = plan
        .extensions
        .first()
        .map(|e| e.name.as_str())
        .unwrap_or("shim");
    let world = match primary {
        "postgis" => POSTGIS_WORLD_WIT,
        other => {
            return Err(anyhow!(
                "wasm-component target: no world template for shim '{other}' yet \
                 (Phase 1 supports postgis only; see PLAN-codegen-retarget.md \
                 Phase 4 for mobilitydb)"
            ))
        }
    };
    fs::write(dest, world).with_context(|| format!("writing {}", dest.display()))?;
    Ok(())
}

/// Copy the dependency WIT tree into `wit/deps/`. Source layout
/// is the hand-written postgis-bridge's `wit/deps/` directory.
pub fn write_deps(_plan: &BridgePlan, deps_dir: &Path) -> Result<()> {
    let src = source_deps_dir()?;
    copy_tree(&src, deps_dir).with_context(|| {
        format!("copying WIT deps from {} to {}", src.display(), deps_dir.display())
    })
}

/// Locate the source `wit/deps/` directory.
///
/// Resolution order:
///   1. `$SQLINK_POSTGIS_BRIDGE_WIT_DEPS` (explicit override)
///   2. `$HOME/git/sqlink/extensions/postgis-bridge/wit/deps`
///   3. `../sqlink/extensions/postgis-bridge/wit/deps` (relative
///      to current working dir  matches the codegen running
///      inside `~/git/sqlink-shim-codegen/`)
fn source_deps_dir() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("SQLINK_POSTGIS_BRIDGE_WIT_DEPS") {
        let p = PathBuf::from(p);
        if p.is_dir() {
            return Ok(p);
        }
        return Err(anyhow!(
            "SQLINK_POSTGIS_BRIDGE_WIT_DEPS={} does not exist",
            p.display()
        ));
    }
    if let Some(home) = std::env::var_os("HOME") {
        let p = PathBuf::from(home)
            .join("git/sqlink/extensions/postgis-bridge/wit/deps");
        if p.is_dir() {
            return Ok(p);
        }
    }
    let rel = PathBuf::from("../sqlink/extensions/postgis-bridge/wit/deps");
    if rel.is_dir() {
        return Ok(rel);
    }
    Err(anyhow!(
        "cannot locate postgis-bridge wit/deps. Set \
         SQLINK_POSTGIS_BRIDGE_WIT_DEPS=/path/to/sqlink/extensions/postgis-bridge/wit/deps"
    ))
}

fn copy_tree(src: &Path, dst: &Path) -> Result<()> {
    if !src.is_dir() {
        return Err(anyhow!("source {} is not a directory", src.display()));
    }
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_tree(&from, &to)?;
        } else if file_type.is_file() {
            fs::copy(&from, &to)
                .with_context(|| format!("copy {} -> {}", from.display(), to.display()))?;
        }
        // skip symlinks / other  not expected in WIT trees
    }
    Ok(())
}

/// PostGIS world.wit. Mirrors the hand-written bridge's world
/// minus the topology imports (those aren't used by stubs and
/// dropping them shrinks the compose surface). Exports the
/// metadata + scalar + aggregate + vtab quartet so the host's
/// `Minimal::instantiate_async` describe-call resolves and the
/// extended worlds work once Phase 2 turns the stubs into real
/// dispatchers.
const POSTGIS_WORLD_WIT: &str = r##"package sqlink-bridge:postgis@0.1.0;

/// Generated by sqlink-shim-codegen (Phase 1, target=wasm-component).
/// Bridges postgis-wasm's spatial functions onto the canonical
/// `sqlite:extension/*` contract. Same import surface as the
/// hand-written `extensions/postgis-bridge/wit/world.wit` in
/// sqlink; same export surface so the host's load path sees a
/// drop-in component.
world bridge {
    import postgis:wasm/postgis-types@0.1.0;
    import postgis:wasm/postgis-constructors@0.1.0;
    import postgis:wasm/postgis-accessors@0.1.0;
    import postgis:wasm/postgis-measurements@0.1.0;
    import postgis:wasm/postgis-predicates@0.1.0;
    import postgis:wasm/postgis-processing@0.1.0;
    import postgis:wasm/postgis-output@0.1.0;
    import postgis:wasm/postgis-transformations@0.1.0;
    import postgis:wasm/postgis-aggregates@0.1.0;
    import postgis:wasm/postgis-clustering@0.1.0;
    import postgis:wasm/postgis-spatial-index@0.1.0;
    import postgis:wasm/postgis-linear-ref@0.1.0;
    import postgis:wasm/postgis-three-d@0.1.0;
    import postgis:wasm/postgis-geodetic@0.1.0;
    import postgis:wasm/postgis-sfcgal@0.1.0;
    import postgis:wasm/postgis-raster-types@0.1.0;
    import postgis:wasm/postgis-raster-constructors@0.1.0;
    import postgis:wasm/postgis-raster-accessors@0.1.0;
    import postgis:wasm/postgis-raster-stats@0.1.0;
    import postgis:wasm/postgis-raster-mapalgebra@0.1.0;
    import postgis:wasm/postgis-raster-pixels@0.1.0;
    import postgis:wasm/postgis-raster-output@0.1.0;
    import postgis:wasm/postgis-raster-vector@0.1.0;
    import postgis:wasm/postgis-raster-predicates@0.1.0;
    import postgis:wasm/postgis-raster-processing@0.1.0;
    import postgis:wasm/postgis-raster-aggregates@0.1.0;
    import postgis:wasm/postgis-operators@0.1.0;
    import postgis:wasm/postgis-geocoder@0.1.0;
    import postgis:wasm/postgis-topology-types@0.1.0;
    import postgis:wasm/postgis-topology-output@0.1.0;
    import postgis:wasm/postgis-topology-edit@0.1.0;
    import postgis:wasm/postgis-topology-query@0.1.0;
    import postgis:wasm/postgis-topology-topogeom@0.1.0;

    import sfcgal:component/geometry@1.0.0;
    import sfcgal:component/io@1.0.0;

    import sqlite:extension/types@0.1.0;
    import sqlite:extension/spi@0.1.0;
    import sqlite:extension/logging@0.1.0;
    import sqlite:extension/config@0.1.0;
    import sqlite:extension/state@0.1.0;
    import sqlite:extension/cache@0.1.0;

    export sqlite:extension/metadata@0.1.0;
    export sqlite:extension/scalar-function@0.1.0;
    export sqlite:extension/aggregate-function@0.1.0;
    export sqlite:extension/vtab@0.1.0;
}
"##;
