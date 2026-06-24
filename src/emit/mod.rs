//! Per-kind emit modules. Each returns a finished `String` of
//! Rust source for one file in the generated bridge crate.
//!
//! The emitters are intentionally template-shaped (string
//! formatting, no proc-macro or syn) so an agent reading this
//! repo can scan the output shape at a glance and adjust it
//! without understanding a macro language.

use crate::core::BridgePlan;

pub fn cargo_toml(plan: &BridgePlan) -> String {
    let crate_name = sanitize_crate_name(&primary_extension_name(plan));
    format!(
r##"[package]
name = "{name}-sqlite-bridge"
version = "0.1.0"
edition = "2021"
description = "Generated SQLite extension that bridges the {name} DataFission shim into SQLite."
license = "Apache-2.0"

[lib]
# `cdylib` is the loadable-extension shape SQLite's `.load`
# expects. `lib` is kept so the inner modules are also
# unit-testable.
name = "{name}_sqlite_bridge"
crate-type = ["cdylib", "rlib"]

[dependencies]
# Path-deps into the source DataFission tree so we can call
# the loader's wasm scalar-invoke surface directly. Move to
# git-deps when DataFission ships releases.
datafission-df-plugin-loader = {{ path = "../datafission/crates/df-plugin-loader" }}
datafission-df-plugin-api    = {{ path = "../datafission/crates/df-plugin-api" }}
datafission-functions        = {{ path = "../datafission/crates/functions" }}

# rusqlite's `loadable_extension` feature provides the
# `Connection::extension_init2` entry-point helper; no
# bundled libsqlite (the extension links against the host
# SQLite at load time).
rusqlite = {{ version = "0.32", features = ["loadable_extension", "functions"] }}

anyhow     = "1"
once_cell  = "1"
parking_lot = "0.12"
tracing    = "0.1"
serde_json = "1"

[profile.release]
lto         = true
codegen-units = 1
opt-level   = "z"
strip       = true
"##,
        name = crate_name,
    )
}

pub fn lib_rs(plan: &BridgePlan) -> String {
    let header = generated_header();
    let mut s = String::new();
    s.push_str(&header);
    s.push_str(
r##"//! Generated SQLite extension entry point.
//!
//! Load with:
//!   sqlite> .load ./target/release/lib<ext>_sqlite_bridge
//!
//! Phase 1 (2026-06-23): scalar dispatch wired through
//! df-plugin-loader. ST_GeomFromText is fully functional; the
//! other categories (aggregates, UDTFs, window funcs, types,
//! operators, casts, preprocessors, system catalog, spatial
//! indexes) are scaffold-only — see per-module TODOs and
//! AGENTS.md for the phased plan.

pub mod registry;
pub mod scalars;
pub mod aggregates;
pub mod table_functions;
pub mod window_functions;
pub mod types;
pub mod operators;
pub mod casts;
pub mod preprocessors;
pub mod system_catalog;
pub mod spatial_indexes;

use std::os::raw::{c_char, c_int};

use rusqlite::ffi;
use rusqlite::{Connection, Result};

/// SQLite extension entry point. Loaded by SQLite when the
/// user runs `.load`. rusqlite's `extension_init2` wraps the
/// `SQLITE_EXTENSION_INIT2` macro: it stashes the api routines
/// pointer and hands us a safe `Connection`.
///
/// Symbol name must be `sqlite3_extension_init` for the
/// default `.load` behavior (no explicit init-function name).
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[no_mangle]
pub unsafe extern "C" fn sqlite3_extension_init(
    db: *mut ffi::sqlite3,
    pz_err_msg: *mut *mut c_char,
    p_api: *mut ffi::sqlite3_api_routines,
) -> c_int {
    Connection::extension_init2(db, pz_err_msg, p_api, init_inner)
}

fn init_inner(conn: Connection) -> Result<bool> {
    // Load the composed shim wasm exactly once. The path comes
    // from `<EXT>_SHIM_WASM` env var so the bridge isn't pinned
    // to a build-time path (matches the host's runtime-loaded
    // model).
    registry::load_shim().map_err(|e| {
        rusqlite::Error::UserFunctionError(
            format!("shim load: {e}").into()
        )
    })?;

    // Register Phase-1 scalars.
    scalars::register_all(&conn)
        .map_err(|e| rusqlite::Error::UserFunctionError(
            format!("scalar registration: {e}").into()
        ))?;

    // Returning `true` means "extension fully initialized — no
    // need to keep it loaded across DB sessions". `false` would
    // ask SQLite to auto-load on every new connection in the
    // process.
    Ok(true)
}

"##,
    );
    s.push_str(&format!(
        "// Extensions loaded by this bridge:\n//\n{}\n",
        plan.extensions
            .iter()
            .map(|e| format!(
                "//   - {} v{}  ({} scalars, {} agg, {} udtf, {} window, {} types, \
                 {} ops, {} casts, {} preps, {} catalog, {} indexes)",
                e.name, e.version,
                e.scalars.len(), e.aggregates.len(),
                e.table_functions.len(), e.window_functions.len(),
                e.column_types.len(), e.operators.len(),
                e.cast_rewrites.len(), e.preprocessor_patterns.len(),
                e.system_catalog_tables.len(), e.spatial_indexes.len()
            ))
            .collect::<Vec<_>>()
            .join("\n")
    ));
    s
}

pub fn scalars_rs(plan: &BridgePlan) -> String {
    let mut s = generated_header();
    s.push_str(
r##"//! Scalar-function registration.
//!
//! Phase 1 (2026-06-23): the architecture is wired up and
//! ST_GeomFromText is fully functional. Other scalars are
//! listed below as comments; per-name registration follows
//! identically once you uncomment + adapt the dispatcher.
//!
//! Dispatch shape: each SQLite scalar call pulls args from
//! `rusqlite::functions::Context`, maps them to
//! `datafission_functions::types::FunctionValue`, calls the
//! shim's `ScalarFunctionDef::execute`, and maps the result
//! back to a `rusqlite::types::ToSqlOutput`.

use std::sync::Arc;

use rusqlite::functions::{Context, FunctionFlags};
use rusqlite::types::{ToSqlOutput, Value, ValueRef};
use rusqlite::{Connection, Result};

use datafission_functions::traits::ScalarFunctionDef;
use datafission_functions::types::FunctionValue;

use crate::registry;

/// Register every Phase-1 scalar against the given connection.
pub fn register_all(conn: &Connection) -> Result<()> {
"##,
    );

    let mut emitted = 0;
    for ext in &plan.extensions {
        for sc in &ext.scalars {
            // Phase 1: only ST_GeomFromText is wired live. The
            // rest are commented out (with arity / aliases) so a
            // future phase can flip them on without re-running the
            // codegen.
            let is_phase1 = sc.canonical_name == "st_geomfromtext";
            if is_phase1 {
                let nargs = sc.param_signatures.first().map(|v| v.len()).unwrap_or(1) as i32;
                let det = if sc.is_deterministic { "SQLITE_DETERMINISTIC" } else { "0u32.into()" };
                s.push_str(&format!(
                    "    register_scalar(conn, \"{name}\", {nargs}, {det})?;\n",
                    name = sc.canonical_name,
                    nargs = nargs,
                    det = if sc.is_deterministic {
                        "FunctionFlags::SQLITE_DETERMINISTIC | FunctionFlags::SQLITE_UTF8"
                    } else {
                        "FunctionFlags::SQLITE_UTF8"
                    },
                ));
                for alias in &sc.aliases {
                    s.push_str(&format!(
                        "    register_scalar(conn, \"{alias}\", {nargs}, {det})?; // alias of {name}\n",
                        alias = alias, nargs = nargs,
                        det = if sc.is_deterministic {
                            "FunctionFlags::SQLITE_DETERMINISTIC | FunctionFlags::SQLITE_UTF8"
                        } else {
                            "FunctionFlags::SQLITE_UTF8"
                        },
                        name = sc.canonical_name,
                    ));
                }
                emitted += 1;
            }
        }
    }
    if emitted == 0 {
        s.push_str("    // No Phase-1 scalars matched in this interface DB.\n");
    }

    s.push_str(
r##"    Ok(())
}

/// Register one scalar by name. The shim's registry is looked
/// up at dispatch time so each `name` resolves to the same
/// `Arc<dyn ScalarFunctionDef>` the shim handed us at load.
fn register_scalar(
    conn: &Connection,
    sql_name: &str,
    arity: i32,
    flags: FunctionFlags,
) -> Result<()> {
    // Resolve once at registration time. If the shim doesn't
    // know this name, fail fast rather than at first call.
    let def: Arc<dyn ScalarFunctionDef> = registry::lookup_scalar(sql_name)
        .ok_or_else(|| rusqlite::Error::UserFunctionError(
            format!("scalar `{sql_name}` not registered by the shim").into()
        ))?;

    conn.create_scalar_function(sql_name, arity, flags, move |ctx| -> Result<ToSqlOutput<'static>> {
        dispatch_scalar(&def, ctx)
    })
}

/// Per-call dispatcher: marshal sqlite args → FunctionValue,
/// invoke the shim's ScalarFunctionDef, marshal the result back.
///
/// Returns an owned `ToSqlOutput<'static>` because every
/// `FunctionValue` variant we map produces an owned `Value`.
fn dispatch_scalar<'a>(
    def: &Arc<dyn ScalarFunctionDef>,
    ctx: &Context<'a>,
) -> Result<ToSqlOutput<'static>> {
    let n = ctx.len();
    let mut args = Vec::with_capacity(n);
    for i in 0..n {
        let v = ctx.get_raw(i);
        args.push(value_ref_to_function_value(v));
    }
    let result = def.execute(&args).map_err(|e| {
        rusqlite::Error::UserFunctionError(Box::new(std::io::Error::other(format!("{e:?}"))))
    })?;
    Ok(function_value_to_tosql(result))
}

/// SQLite ValueRef → FunctionValue. Null/Real/Integer/Text/Blob
/// map 1:1; nothing else is reachable through SQLite's value
/// system.
fn value_ref_to_function_value(v: ValueRef<'_>) -> FunctionValue {
    match v {
        ValueRef::Null => FunctionValue::Null,
        ValueRef::Integer(i) => FunctionValue::Int64(i),
        ValueRef::Real(f) => FunctionValue::Float64(f),
        ValueRef::Text(b) => FunctionValue::String(
            String::from_utf8_lossy(b).into_owned()
        ),
        ValueRef::Blob(b) => FunctionValue::Binary(b.to_vec()),
    }
}

/// FunctionValue → ToSqlOutput. Owned variants only — the
/// SQLite layer will copy, so we don't try to borrow.
fn function_value_to_tosql(v: FunctionValue) -> ToSqlOutput<'static> {
    let value = match v {
        FunctionValue::Null => Value::Null,
        FunctionValue::Boolean(b) => Value::Integer(b as i64),
        FunctionValue::Int8(i) => Value::Integer(i as i64),
        FunctionValue::Int16(i) => Value::Integer(i as i64),
        FunctionValue::Int32(i) => Value::Integer(i as i64),
        FunctionValue::Int64(i) => Value::Integer(i),
        FunctionValue::UInt8(i) => Value::Integer(i as i64),
        FunctionValue::UInt16(i) => Value::Integer(i as i64),
        FunctionValue::UInt32(i) => Value::Integer(i as i64),
        FunctionValue::UInt64(i) => Value::Integer(i as i64),
        FunctionValue::Float32(f) => Value::Real(f as f64),
        FunctionValue::Float64(f) => Value::Real(f),
        FunctionValue::String(s) => Value::Text(s),
        FunctionValue::Binary(b) => Value::Blob(b),
        // Array / Map / Struct don't have a canonical SQLite
        // representation. Serialize as JSON text for now —
        // callers can ROUND_TRIP through json_extract.
        other => Value::Text(
            serde_json::to_string(&other).unwrap_or_else(|_| "<unrepresentable>".into())
        ),
    };
    ToSqlOutput::Owned(value)
}

// ----------------------------------------------------------------------
// Comment block: every scalar in this interface DB. Uncomment
// the matching `register_scalar(...)` call in `register_all`
// above to enable a name in a future phase.
// ----------------------------------------------------------------------

"##,
    );

    for ext in &plan.extensions {
        s.push_str(&format!("// === extension: {} ===\n", ext.name));
        for sc in &ext.scalars {
            if sc.canonical_name == "st_geomfromtext" { continue; }
            let nargs = sc.param_signatures.first().map(|v| v.len()).unwrap_or(0);
            s.push_str(&format!(
                "// scalar `{}` (deterministic={}, propagates_null={}, arity={}, return={})\n",
                sc.canonical_name, sc.is_deterministic, sc.propagates_null, nargs, sc.return_type
            ));
            if !sc.aliases.is_empty() {
                s.push_str(&format!("//   aliases: {}\n", sc.aliases.join(", ")));
            }
        }
        s.push('\n');
    }
    s
}

pub fn registry_rs(plan: &BridgePlan) -> String {
    let mut s = generated_header();
    let env_var = format!(
        "{}_SHIM_WASM",
        primary_extension_name(plan).to_uppercase().replace('-', "_")
    );
    s.push_str(&format!(
r##"//! Shim registry — loads the composed wasm shim exactly once
//! at extension-init time and exposes a name → ScalarFunctionDef
//! lookup for the per-call dispatcher.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{{Context, Result}};
use once_cell::sync::OnceCell;
use parking_lot::RwLock;

use datafission_df_plugin_api::{{
    DataTypePlugin, Extension, ExtensionError, ExtensionTarget, SystemCatalogProvider,
}};
use datafission_df_plugin_loader::RuntimeWasmExtension;
use datafission_functions::traits::{{
    AggregateFunctionDef, ScalarFunctionDef, TableFunctionDef, WindowFunctionDef,
}};

/// Lazily-loaded shim handle. Initialised in `load_shim()` from
/// the `{env}` env var.
static SHIM: OnceCell<ShimRegistry> = OnceCell::new();

struct ShimRegistry {{
    _ext: RuntimeWasmExtension,  // keep the wasm Store alive
    scalars: RwLock<HashMap<String, Arc<dyn ScalarFunctionDef>>>,
}}

pub fn load_shim() -> Result<()> {{
    if SHIM.get().is_some() {{
        return Ok(());
    }}
    let path = std::env::var("{env}")
        .with_context(|| format!(
            "Set {env}=/path/to/composed-shim.wasm before .load"
        ))?;
    let ext = RuntimeWasmExtension::from_file(&path)
        .with_context(|| format!("loading shim {{path}}"))?;

    let mut capture = CapturingTarget {{
        scalars: Vec::new(),
    }};
    ext.register(&mut capture)
        .map_err(|e| anyhow::anyhow!("shim register: {{e}}"))?;

    let mut scalars = HashMap::with_capacity(capture.scalars.len() * 2);
    for def in capture.scalars {{
        let canonical = def.name().to_string();
        for alias in def.aliases() {{
            scalars.insert(alias.to_string(), Arc::clone(&def));
        }}
        scalars.insert(canonical, def);
    }}

    SHIM.set(ShimRegistry {{
        _ext: ext,
        scalars: RwLock::new(scalars),
    }}).map_err(|_| anyhow::anyhow!("ShimRegistry already initialised"))?;

    Ok(())
}}

pub fn lookup_scalar(name: &str) -> Option<Arc<dyn ScalarFunctionDef>> {{
    let r = SHIM.get()?;
    r.scalars.read().get(name).cloned()
}}

/// Minimal ExtensionTarget that just collects every scalar the
/// shim registers. We ignore other categories in Phase 1 —
/// later phases extend this to capture aggregates / UDTFs /
/// window functions / types / system catalog / spatial indexes
/// in parallel maps.
struct CapturingTarget {{
    scalars: Vec<Arc<dyn ScalarFunctionDef>>,
}}

impl ExtensionTarget for CapturingTarget {{
    fn register_scalar_function(
        &mut self,
        _namespace: &str,
        def: Arc<dyn ScalarFunctionDef>,
    ) -> std::result::Result<(), ExtensionError> {{
        self.scalars.push(def);
        Ok(())
    }}
    fn register_aggregate_function(
        &mut self,
        _namespace: &str,
        _def: Arc<dyn AggregateFunctionDef>,
    ) -> std::result::Result<(), ExtensionError> {{
        Ok(())
    }}
    fn register_table_function(
        &mut self,
        _namespace: &str,
        _def: Arc<dyn TableFunctionDef>,
    ) -> std::result::Result<(), ExtensionError> {{
        Ok(())
    }}
    fn register_window_function(
        &mut self,
        _namespace: &str,
        _def: Arc<dyn WindowFunctionDef>,
    ) -> std::result::Result<(), ExtensionError> {{
        Ok(())
    }}
    fn register_data_type(
        &mut self,
        _plugin: Arc<dyn DataTypePlugin>,
    ) -> std::result::Result<(), ExtensionError> {{
        Ok(())
    }}
    fn register_system_catalog_provider(
        &mut self,
        _provider: Arc<dyn SystemCatalogProvider>,
    ) -> std::result::Result<(), ExtensionError> {{
        Ok(())
    }}
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {{
        self
    }}
}}
"##,
        env = env_var,
    ));
    s
}

pub fn aggregates_rs(plan: &BridgePlan) -> String {
    let mut s = generated_header();
    s.push_str(r##"//! Aggregate-function registration.
//!
//! SQLite aggregates use sqlite3_create_window_function (for
//! window-capable aggregates) or sqlite3_create_function_v2 with
//! step+final callbacks.
//!
//! Shim aggregates are partition-aware: the host streams rows
//! into the wasm guest's accumulator, then drains a final value.

"##);
    for ext in &plan.extensions {
        for agg in &ext.aggregates {
            s.push_str(&format!(
                "// aggregate `{}` (grouped={}, partial={}, order_sensitive={}, accepts_config={})\n",
                agg.canonical_name,
                agg.supports_grouped,
                agg.supports_partial,
                agg.is_order_sensitive,
                agg.accepts_config,
            ));
            if !agg.aliases.is_empty() {
                s.push_str(&format!("//   aliases: {}\n", agg.aliases.join(", ")));
            }
        }
    }
    s.push_str("\n// TODO: emit sqlite3_create_function_v2 calls with step+final.\n");
    s
}

pub fn table_functions_rs(plan: &BridgePlan) -> String {
    let mut s = generated_header();
    s.push_str(r##"//! Table-function (UDTF) registration.
//!
//! Mapped to SQLite virtual tables via sqlite3_module. Each
//! shim UDTF becomes one virtual table whose xBestIndex/xFilter
//! pulls rows from the shim.

"##);
    for ext in &plan.extensions {
        for tf in &ext.table_functions {
            s.push_str(&format!("// udtf `{}`\n", tf.canonical_name));
        }
    }
    s.push_str("\n// TODO: emit sqlite3_create_module_v2 calls.\n");
    s
}

pub fn window_functions_rs(plan: &BridgePlan) -> String {
    let mut s = generated_header();
    s.push_str(r##"//! Window-function registration (sqlite3_create_window_function).

"##);
    for ext in &plan.extensions {
        for w in &ext.window_functions {
            s.push_str(&format!("// window `{}`\n", w.canonical_name));
        }
    }
    s.push_str("\n// TODO: emit sqlite3_create_window_function calls.\n");
    s
}

pub fn types_rs(plan: &BridgePlan) -> String {
    let mut s = generated_header();
    s.push_str(r##"//! Custom column types.
//!
//! SQLite does NOT have first-class custom types; the
//! conventional pattern is BLOB columns + a sidecar text-type
//! affinity. GEOMETRY ends up stored as EWKB blobs.
//!
//! The bridge advertises the type names so application code
//! can use `CREATE TABLE t (g GEOMETRY)` syntactically — but
//! SQLite stores it as BLOB. Type-id round-trips through the
//! shim are preserved via the shim's own type tag inside the
//! blob payload.

"##);
    for ext in &plan.extensions {
        for ct in &ext.column_types {
            s.push_str(&format!(
                "// type_id={:5} name={:<24} size={:>4}  cast_from={:?}  cast_to={:?}\n",
                ct.type_id, ct.type_name, ct.storage_size, ct.cast_from, ct.cast_to
            ));
        }
    }
    s.push_str("\n// TODO: no explicit registration needed for SQLite — types are advisory.\n\
                 //       Use this list to wire CAST() rewrites in casts.rs.\n");
    s
}

pub fn operators_rs(plan: &BridgePlan) -> String {
    let mut s = generated_header();
    s.push_str(r##"//! Operator handling.
//!
//! SQLite has NO custom operators. The bridge handles `a && b`,
//! `a <-> b`, etc. by registering a parser-rewrite shim that
//! turns each `lhs OP rhs` into `op_<symbol>(lhs, rhs)`. The
//! function `op_<symbol>` is registered as a regular scalar in
//! scalars.rs.
//!
//! For the host this means: either intercept queries via a
//! preprocessor (sqlink's wrapper sees the SQL text before
//! SQLite does), or document that users must write the function
//! form themselves. The first option is preferred when
//! available.

"##);
    for ext in &plan.extensions {
        for op in &ext.operators {
            s.push_str(&format!(
                "// `{}` (lhs={:?}, rhs={:?})  →  {}\n",
                op.symbol, op.lhs_type_id, op.rhs_type_id, op.function_name
            ));
        }
    }
    s.push_str("\n// TODO: build the operator → function rewrite table\n\
                 //       and feed it to sqlink's parser-preprocessor hook.\n");
    s
}

pub fn casts_rs(plan: &BridgePlan) -> String {
    let mut s = generated_header();
    s.push_str(r##"//! CAST(x AS T) rewrites.
//!
//! `source_kind` tells the rewriter when to fire:
//!   - "any"             — always
//!   - "stringliteral"   — only literal strings
//!   - "geographycolumn" — only when x has geography column type
//!
//! SQLite's CAST() doesn't natively understand custom types, so
//! the bridge rewrites `CAST(x AS GEOMETRY)` into
//! `ST_GeomFromText(x)` (or the appropriate function for the
//! source kind).

"##);
    for ext in &plan.extensions {
        for c in &ext.cast_rewrites {
            s.push_str(&format!(
                "// CAST(<{}> AS {}) → {} (hint: {})\n",
                c.source_kind, c.target_type, c.function_name, c.source_fn_hint
            ));
        }
    }
    s.push_str("\n// TODO: register CAST-rewrite rules in sqlink's preprocessor.\n");
    s
}

pub fn preprocessors_rs(plan: &BridgePlan) -> String {
    let mut s = generated_header();
    s.push_str(r##"//! SQL preprocessor patterns — token-level rewrites the shim
//! advertises (e.g. PostGIS's EWKT-shorthand prefix-arrow).

"##);
    for ext in &plan.extensions {
        for p in &ext.preprocessor_patterns {
            s.push_str(&format!("// token `{}` → {}\n", p.op_token, p.function_name));
        }
    }
    s.push_str("\n// TODO: emit the preprocessor rewrite table.\n");
    s
}

pub fn system_catalog_rs(plan: &BridgePlan) -> String {
    let mut s = generated_header();
    s.push_str(r##"//! System-catalog virtual tables (spatial_ref_sys etc.).
//!
//! Each becomes a sqlite3_module-backed virtual table. The
//! schema is in the comments below; the dispatch pulls rows
//! from the shim's catalog provider.

"##);
    for ext in &plan.extensions {
        for ct in &ext.system_catalog_tables {
            s.push_str(&format!("// catalog `{}.{}`\n", ct.catalog_name, ct.table_name));
            for col in &ct.columns {
                s.push_str(&format!(
                    "//   {} {} ({})\n",
                    col.name,
                    col.data_type,
                    if col.nullable { "nullable" } else { "not null" },
                ));
            }
        }
    }
    s.push_str("\n// TODO: emit sqlite3_create_module_v2 calls for each.\n");
    s
}

pub fn spatial_indexes_rs(plan: &BridgePlan) -> String {
    let mut s = generated_header();
    s.push_str(
r##"//! Spatial index registration.
//!
//! SQLite has R*Tree (`rtree` module, built in) and nothing
//! else. The bridge maps each shim spatial-index `type_id` to
//! an R*Tree-backed virtual table; SQL `CREATE INDEX … USING
//! <name>` becomes a CREATE VIRTUAL TABLE statement under the
//! hood. Insert/delete triggers keep the rtree in sync.
//!
//! For non-rectangular indexes (KdTree, Quadtree, Octree) the
//! shim's CPU-side query takes over: we register a UDTF
//! `<name>_query(args)` that the optimizer can use when it
//! sees a WHERE clause involving an index-aware operator. See
//! operators.rs for the rewrite that feeds it.

"##,
    );
    for ext in &plan.extensions {
        for ix in &ext.spatial_indexes {
            s.push_str(&format!("// index `{}` type_id={}\n", ix.name, ix.type_id));
        }
    }
    s.push_str(
        "\n// TODO: emit one CREATE VIRTUAL TABLE … USING rtree(...)\n\
         //       per shim index, plus the trigger pair that\n\
         //       maintains it on the source table.\n",
    );
    s
}

pub fn readme(plan: &BridgePlan) -> String {
    let mut s = String::new();
    s.push_str("# Generated SQLite bridge\n\n");
    s.push_str("This crate was produced by `sqlink-shim-codegen`. Do not edit\n");
    s.push_str("by hand — regenerate from the source `.sqlite`.\n\n");
    s.push_str("## Extensions wrapped\n\n");
    s.push_str(
        "| Extension | Version | Scalars | Aggregates | UDTFs | Windows | Types | \
         Operators | Casts | Preprocessors | Catalog | Indexes |\n",
    );
    s.push_str("|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|\n");
    for e in &plan.extensions {
        s.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            e.name, e.version,
            e.scalars.len(),
            e.aggregates.len(),
            e.table_functions.len(),
            e.window_functions.len(),
            e.column_types.len(),
            e.operators.len(),
            e.cast_rewrites.len(),
            e.preprocessor_patterns.len(),
            e.system_catalog_tables.len(),
            e.spatial_indexes.len(),
        ));
    }
    s
}

fn generated_header() -> String {
    "// === GENERATED by sqlink-shim-codegen — do not edit by hand ===\n\n".into()
}

fn sanitize_crate_name(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

fn primary_extension_name(plan: &BridgePlan) -> String {
    plan.extensions.first().map(|e| e.name.clone()).unwrap_or_else(|| "shim".into())
}
