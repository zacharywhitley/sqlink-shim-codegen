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
datafission-types            = {{ path = "../datafission/crates/types" }}

# rusqlite's `loadable_extension` feature provides the
# `Connection::extension_init2` entry-point helper; no
# bundled libsqlite (the extension links against the host
# SQLite at load time).
rusqlite = {{ version = "0.32", features = ["loadable_extension", "functions", "vtab"] }}

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
        // `{e:#}` walks the anyhow chain so the user sees the
        // underlying cause (wasm parse error, missing import,
        // etc.) rather than just the top-level wrapper.
        rusqlite::Error::UserFunctionError(
            format!("shim load: {e:#}").into()
        )
    })?;

    // Register scalars (Phase 2) + aggregates (Phase 3c).
    scalars::register_all(&conn)
        .map_err(|e| rusqlite::Error::UserFunctionError(
            format!("scalar registration: {e}").into()
        ))?;
    aggregates::register_all(&conn)
        .map_err(|e| rusqlite::Error::UserFunctionError(
            format!("aggregate registration: {e}").into()
        ))?;
    table_functions::register_all(&conn)
        .map_err(|e| rusqlite::Error::UserFunctionError(
            format!("table function registration: {e}").into()
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

/// Register every scalar the shim publishes.
///
/// Phase 2 (2026-06-24): registration is now arity-agnostic. The
/// `dispatch_scalar` helper marshals every arg through
/// `ValueRef` → `FunctionValue` (works for any SQLite type) and
/// every result back through `function_value_to_tosql` (handles
/// every `FunctionValue` variant). So one registration loop
/// covers every signature shape — no per-shape marker structs
/// needed because SQLite has dynamic typing through its
/// `ValueRef` enum.
///
/// Variadic functions (arity = -1) are emitted with rusqlite's
/// `arity = -1` convention.
pub fn register_all(conn: &Connection) -> Result<()> {
"##,
    );

    let mut emitted = 0;
    let mut alias_count = 0;
    for ext in &plan.extensions {
        for sc in &ext.scalars {
            // Determine arity: if every variant has the same arg
            // count, use that; otherwise use -1 (rusqlite's
            // variadic marker). SQLite itself has no overload
            // mechanism — a name resolves to one function — so
            // when multiple variants exist, our wrapper accepts
            // any count and lets the shim's execute() validate.
            let variants = &sc.param_signatures;
            let arity = if variants.is_empty() {
                -1i32
            } else {
                let first = variants[0].len();
                if variants.iter().all(|v| v.len() == first) {
                    first as i32
                } else {
                    -1
                }
            };
            let flags = if sc.is_deterministic {
                "FunctionFlags::SQLITE_DETERMINISTIC | FunctionFlags::SQLITE_UTF8"
            } else {
                "FunctionFlags::SQLITE_UTF8"
            };
            s.push_str(&format!(
                "    register_scalar(conn, \"{name}\", {arity}, {flags})?;\n",
                name = sc.canonical_name,
            ));
            for alias in &sc.aliases {
                s.push_str(&format!(
                    "    register_scalar(conn, \"{alias}\", {arity}, {flags})?; // alias of {name}\n",
                    alias = alias, name = sc.canonical_name,
                ));
                alias_count += 1;
            }
            emitted += 1;
        }
    }
    s.push_str(&format!(
        "    // Phase 2: {emitted} canonical + {alias_count} alias names registered.\n"
    ));
    if emitted == 0 {
        s.push_str("    // (no scalars in this interface DB)\n");
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
    let mut any_null = false;
    for i in 0..n {
        let v = ctx.get_raw(i);
        if matches!(v, ValueRef::Null) {
            any_null = true;
        }
        args.push(value_ref_to_function_value(v));
    }
    // Phase 3b (2026-06-24): honor propagates_null. Most spatial
    // scalars are NULL-propagating per SQL-92 — if any input is
    // NULL the result is NULL, no function call needed. Skips
    // both the shim wasm round-trip AND the inevitable parse
    // error from the shim trying to handle Null as bytes.
    if any_null && def.propagates_null() {
        return Ok(ToSqlOutput::Owned(Value::Null));
    }
    let result = def.execute(&args).map_err(|e| {
        rusqlite::Error::UserFunctionError(Box::new(std::io::Error::other(format!("{e:?}"))))
    })?;
    Ok(function_value_to_tosql(result))
}

/// SQLite ValueRef → FunctionValue. Null/Real/Integer/Text/Blob
/// map 1:1; nothing else is reachable through SQLite's value
/// system.
pub(crate) fn value_ref_to_function_value(v: ValueRef<'_>) -> FunctionValue {
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
pub(crate) fn function_value_to_tosql(v: FunctionValue) -> ToSqlOutput<'static> {
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

"##,
    );
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
    aggregates: RwLock<HashMap<String, Arc<dyn AggregateFunctionDef>>>,
    table_functions: RwLock<HashMap<String, Arc<dyn TableFunctionDef>>>,
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
        aggregates: Vec::new(),
        table_functions: Vec::new(),
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
    let mut aggregates = HashMap::with_capacity(capture.aggregates.len() * 2);
    for def in capture.aggregates {{
        let canonical = def.name().to_string();
        for alias in def.aliases() {{
            aggregates.insert(alias.to_string(), Arc::clone(&def));
        }}
        aggregates.insert(canonical, def);
    }}
    let mut table_functions = HashMap::with_capacity(capture.table_functions.len() * 2);
    for def in capture.table_functions {{
        let canonical = def.name().to_string();
        for alias in def.aliases() {{
            table_functions.insert(alias.to_string(), Arc::clone(&def));
        }}
        table_functions.insert(canonical, def);
    }}

    SHIM.set(ShimRegistry {{
        _ext: ext,
        scalars: RwLock::new(scalars),
        aggregates: RwLock::new(aggregates),
        table_functions: RwLock::new(table_functions),
    }}).map_err(|_| anyhow::anyhow!("ShimRegistry already initialised"))?;

    Ok(())
}}

pub fn lookup_scalar(name: &str) -> Option<Arc<dyn ScalarFunctionDef>> {{
    let r = SHIM.get()?;
    r.scalars.read().get(name).cloned()
}}

pub fn lookup_aggregate(name: &str) -> Option<Arc<dyn AggregateFunctionDef>> {{
    let r = SHIM.get()?;
    r.aggregates.read().get(name).cloned()
}}

pub fn lookup_table_function(name: &str) -> Option<Arc<dyn TableFunctionDef>> {{
    let r = SHIM.get()?;
    r.table_functions.read().get(name).cloned()
}}

pub fn all_table_function_names() -> Vec<String> {{
    let r = match SHIM.get() {{ Some(r) => r, None => return vec![] }};
    r.table_functions.read().keys().cloned().collect()
}}

struct CapturingTarget {{
    scalars: Vec<Arc<dyn ScalarFunctionDef>>,
    aggregates: Vec<Arc<dyn AggregateFunctionDef>>,
    table_functions: Vec<Arc<dyn TableFunctionDef>>,
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
        def: Arc<dyn AggregateFunctionDef>,
    ) -> std::result::Result<(), ExtensionError> {{
        self.aggregates.push(def);
        Ok(())
    }}
    fn register_table_function(
        &mut self,
        _namespace: &str,
        def: Arc<dyn TableFunctionDef>,
    ) -> std::result::Result<(), ExtensionError> {{
        self.table_functions.push(def);
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
    s.push_str(
r##"//! Aggregate-function registration.
//!
//! Phase 3c (2026-06-24): wired via rusqlite's
//! `Connection::create_aggregate_function`. Each shim
//! AggregateFunctionDef becomes one ShimAggregate instance
//! registered under every canonical + alias name.
//!
//! State plumbing:
//!   - rusqlite's Aggregate<A, T> trait: A is the per-group
//!     accumulator type, T is the SQL return value (must
//!     impl ToSql). We use A = AccState (newtype around
//!     Box<dyn Accumulator>) and T = ToSqlOutput<'static>.
//!   - rusqlite requires A: RefUnwindSafe + UnwindSafe. The
//!     shim's Accumulator trait doesn't promise either, so
//!     AccState manually opts in (correctness rests on the
//!     shim not panicking on accumulate/finalize — its
//!     wasm-side impl uses Result, no unwinds expected).

use std::panic::{RefUnwindSafe, UnwindSafe};
use std::sync::Arc;

use rusqlite::functions::{Aggregate, Context, FunctionFlags};
use rusqlite::types::{ToSqlOutput, Value};
use rusqlite::{Connection, Result};

use datafission_functions::traits::{Accumulator, AggregateFunctionDef};
use datafission_functions::types::FunctionValue;

use crate::registry;
use crate::scalars::{function_value_to_tosql, value_ref_to_function_value};

/// Per-group accumulator state with the UnwindSafe opt-in that
/// rusqlite requires on `A`.
struct AccState(Box<dyn Accumulator>);
impl RefUnwindSafe for AccState {}
impl UnwindSafe for AccState {}

/// Stateless dispatcher: the per-group state lives in `AccState`,
/// not in this struct. One instance per (name × registration).
struct ShimAggregate {
    def: Arc<dyn AggregateFunctionDef>,
}

impl Aggregate<AccState, ToSqlOutput<'static>> for ShimAggregate {
    fn init(&self, _ctx: &mut Context<'_>) -> Result<AccState> {
        Ok(AccState(self.def.create_accumulator()))
    }

    fn step(&self, ctx: &mut Context<'_>, acc: &mut AccState) -> Result<()> {
        // PostGIS aggregates are unary (ST_Union, ST_Extent,
        // ST_Collect — all take one geometry). For multi-arg
        // future aggregates: walk every ctx arg and call
        // accumulate per row's input tuple. The Accumulator
        // trait takes one value per accumulate call.
        let n = ctx.len();
        if n == 0 {
            return Ok(());
        }
        let v = ctx.get_raw(0);
        // NULL convention: SQL-92 aggregates skip NULL inputs
        // (except COUNT(*)). The shim's Accumulator may also
        // skip; pass NULL through and let the shim decide.
        let value = value_ref_to_function_value(v);
        acc.0.accumulate(&value).map_err(|e| {
            rusqlite::Error::UserFunctionError(Box::new(
                std::io::Error::other(format!("{e:?}"))
            ))
        })
    }

    fn finalize(
        &self,
        _ctx: &mut Context<'_>,
        acc: Option<AccState>,
    ) -> Result<ToSqlOutput<'static>> {
        match acc {
            Some(a) => {
                let result = a.0.finalize().map_err(|e| {
                    rusqlite::Error::UserFunctionError(Box::new(
                        std::io::Error::other(format!("{e:?}"))
                    ))
                })?;
                Ok(function_value_to_tosql(result))
            }
            None => {
                // No rows accumulated → SQL aggregates return NULL.
                // (Except COUNT, which would return 0 — but that
                // never reaches this branch in practice.)
                let _ = self;
                let _ = FunctionValue::Null;
                Ok(ToSqlOutput::Owned(Value::Null))
            }
        }
    }
}

/// Register every aggregate the shim publishes.
pub fn register_all(conn: &Connection) -> Result<()> {
"##,
    );

    let mut canonical = 0;
    let mut alias_count = 0;
    for ext in &plan.extensions {
        for agg in &ext.aggregates {
            let variants = &agg.param_signatures;
            let arity = if variants.is_empty() {
                -1i32
            } else {
                let first = variants[0].len();
                if variants.iter().all(|v| v.len() == first) {
                    first as i32
                } else {
                    -1
                }
            };
            s.push_str(&format!(
                "    register_aggregate(conn, \"{name}\", {arity})?;\n",
                name = agg.canonical_name,
            ));
            for alias in &agg.aliases {
                s.push_str(&format!(
                    "    register_aggregate(conn, \"{alias}\", {arity})?; // alias of {name}\n",
                    alias = alias, name = agg.canonical_name,
                ));
                alias_count += 1;
            }
            canonical += 1;
        }
    }
    s.push_str(&format!(
        "    // Phase 3c: {canonical} canonical + {alias_count} alias names registered.\n"
    ));
    if canonical == 0 {
        s.push_str("    // (no aggregates in this interface DB)\n");
    }

    s.push_str(
r##"    Ok(())
}

fn register_aggregate(conn: &Connection, sql_name: &str, arity: i32) -> Result<()> {
    let def = registry::lookup_aggregate(sql_name).ok_or_else(|| {
        rusqlite::Error::UserFunctionError(
            format!("aggregate `{sql_name}` not registered by the shim").into()
        )
    })?;
    conn.create_aggregate_function(
        sql_name,
        arity,
        FunctionFlags::SQLITE_DETERMINISTIC | FunctionFlags::SQLITE_UTF8,
        ShimAggregate { def },
    )
}
"##,
    );
    s
}

pub fn table_functions_rs(plan: &BridgePlan) -> String {
    let mut s = generated_header();
    s.push_str(
r##"//! Table-function (UDTF) registration.
//!
//! Phase 4c (2026-06-24): a single `ShimVTab` adapter
//! parameterised by `Aux = Arc<dyn TableFunctionDef>` exposes
//! every shim UDTF as a SQLite eponymous virtual table. Each
//! UDTF name gets one `create_module` registration; the per-
//! UDTF def is passed in as `aux`.
//!
//! Schema construction
//!   * `output_schema(input_types)` gives us the output columns.
//!     They become the visible columns in the SQLite vtable.
//!   * Each input arg becomes a hidden BLOB column. `best_index`
//!     marks them as required-for-filter so SQLite passes
//!     `SELECT ... FROM udtf(arg0, arg1, ...)` arguments
//!     through to `filter()`.
//!
//! Cursor lifecycle
//!   * `filter(args)` builds FunctionValues, calls
//!     `def.execute(...)`, stores the returned iterator, and
//!     advances to the first row.
//!   * `next` / `eof` / `column` / `rowid` drive row-by-row
//!     consumption of the shim's iterator.

use std::sync::Arc;

use rusqlite::ffi;
use rusqlite::types::ToSql;
use rusqlite::vtab::{
    eponymous_only_module, Context, IndexConstraintOp, IndexInfo, VTab,
    VTabConfig, VTabConnection, VTabCursor, Values,
};
use rusqlite::{Connection, Result};

use datafission_functions::traits::{TableFunctionDef, TableFunctionIterator, TableRow};
use datafission_functions::types::FunctionValue;

use crate::registry;
use crate::scalars::{function_value_to_tosql, value_ref_to_function_value};

/// Register every UDTF the shim publishes.
pub fn register_all(conn: &Connection) -> Result<()> {
"##);

    let mut canonical = 0;
    let mut alias_count = 0;
    for ext in &plan.extensions {
        for tf in &ext.table_functions {
            s.push_str(&format!(
                "    register_udtf(conn, \"{name}\")?;\n",
                name = tf.canonical_name,
            ));
            for alias in &tf.aliases {
                s.push_str(&format!(
                    "    register_udtf(conn, \"{alias}\")?; // alias of {name}\n",
                    alias = alias, name = tf.canonical_name,
                ));
                alias_count += 1;
            }
            canonical += 1;
        }
    }
    s.push_str(&format!(
        "    // Phase 4c: {canonical} canonical + {alias_count} alias UDTFs registered.\n"
    ));
    if canonical == 0 {
        s.push_str("    // (no UDTFs in this interface DB)\n");
    }

    s.push_str(
r##"    Ok(())
}

fn register_udtf(conn: &Connection, sql_name: &str) -> Result<()> {
    let def = registry::lookup_table_function(sql_name).ok_or_else(|| {
        rusqlite::Error::UserFunctionError(
            format!("udtf `{sql_name}` not registered by the shim").into()
        )
    })?;
    conn.create_module(sql_name, eponymous_only_module::<ShimVTab>(), Some(def))
}

#[repr(C)]
struct ShimVTab {
    /// Base class — must be first (sqlite3_vtab ABI requirement).
    base: ffi::sqlite3_vtab,
    /// Shim def for this UDTF. Cloned from the module's Aux on
    /// each connect (one connect per `SELECT FROM udtf(...)`).
    def: Arc<dyn TableFunctionDef>,
    /// Output column count (used by best_index to skip those
    /// constraints — only input-arg constraints route to filter).
    n_output: usize,
    /// Input arg count (used by best_index + filter to size argv).
    n_input: usize,
}

unsafe impl<'vtab> VTab<'vtab> for ShimVTab {
    type Aux = Arc<dyn TableFunctionDef>;
    type Cursor = ShimVTabCursor;

    fn connect(
        db: &mut VTabConnection,
        aux: Option<&Self::Aux>,
        _args: &[&[u8]],
    ) -> Result<(String, ShimVTab)> {
        let def = aux.cloned().ok_or_else(|| rusqlite::Error::UserFunctionError(
            "ShimVTab::connect: aux missing".into()
        ))?;

        // Output schema: ask the def what columns it will emit
        // given its first param signature's types. Most shim UDTFs
        // have a stable output schema; for those that branch on
        // input types, this assumes the first signature is the
        // canonical one.
        let first_sig = def.param_types().into_iter().next().unwrap_or_default();
        let output_cols = def.output_schema(&first_sig);
        let n_output = output_cols.len();
        let n_input = first_sig.len();

        let mut schema = String::from("CREATE TABLE x(");
        for (i, col) in output_cols.iter().enumerate() {
            if i > 0 { schema.push_str(", "); }
            // Quote the column name and assign a SQLite affinity
            // based on the shim's DataType.
            schema.push_str(&format!(
                "\"{}\" {}",
                col.name.replace('"', "\"\""),
                datatype_to_affinity(&col.data_type)
            ));
        }
        for i in 0..n_input {
            if !output_cols.is_empty() || i > 0 { schema.push_str(", "); }
            schema.push_str(&format!("\"arg{i}\" BLOB HIDDEN"));
        }
        schema.push(')');

        // INNOCUOUS allows the vtable to be used from triggers /
        // views without elevated permission.
        db.config(VTabConfig::Innocuous)?;

        Ok((schema, ShimVTab {
            base: ffi::sqlite3_vtab::default(),
            def,
            n_output,
            n_input,
        }))
    }

    fn best_index(&self, info: &mut IndexInfo) -> Result<()> {
        // Walk every constraint. The input args are at columns
        // [n_output .. n_output+n_input). Mark each EQ constraint
        // on those as required-for-filter so SQLite hands the
        // value to filter() via argv.
        //
        // Two-pass to avoid the borrow conflict on info: collect
        // matching constraint indices first, then mutate usage.
        let usable_arg_constraints: Vec<usize> = info.constraints()
            .enumerate()
            .filter_map(|(i, c)| {
                if !c.is_usable() { return None; }
                let col = c.column() as usize;
                if col < self.n_output { return None; }
                if c.operator() != IndexConstraintOp::SQLITE_INDEX_CONSTRAINT_EQ { return None; }
                Some(i)
            })
            .collect();
        for (rank, i) in usable_arg_constraints.iter().enumerate() {
            let mut usage = info.constraint_usage(*i);
            usage.set_argv_index((rank + 1) as std::os::raw::c_int);
            usage.set_omit(true);
        }
        // Estimated cost: arbitrary low value so the planner
        // prefers our vtable over re-evaluating per row.
        info.set_estimated_cost(1.0);
        Ok(())
    }

    fn open(&'vtab mut self) -> Result<Self::Cursor> {
        Ok(ShimVTabCursor {
            base: ffi::sqlite3_vtab_cursor::default(),
            def: Arc::clone(&self.def),
            iter: None,
            current: None,
            rowid: 0,
            done: true,  // true until filter() loads an iter
        })
    }
}

/// SQLite cursor over the rows yielded by the shim's
/// TableFunctionIterator. Caches the current row so column()
/// can read it without re-driving the iterator.
#[repr(C)]
struct ShimVTabCursor {
    /// Base class — must be first.
    base: ffi::sqlite3_vtab_cursor,
    def: Arc<dyn TableFunctionDef>,
    iter: Option<Box<dyn TableFunctionIterator>>,
    current: Option<TableRow>,
    rowid: i64,
    done: bool,
}

impl ShimVTabCursor {
    fn advance(&mut self) -> Result<()> {
        let iter = match &mut self.iter {
            Some(i) => i,
            None => { self.done = true; return Ok(()); }
        };
        match iter.next_row() {
            Some(Ok(row)) => {
                self.current = Some(row);
                self.rowid += 1;
                self.done = false;
            }
            Some(Err(e)) => {
                return Err(rusqlite::Error::UserFunctionError(Box::new(
                    std::io::Error::other(format!("{e:?}"))
                )));
            }
            None => {
                self.current = None;
                self.done = true;
            }
        }
        Ok(())
    }
}

unsafe impl VTabCursor for ShimVTabCursor {
    fn filter(
        &mut self,
        _idx_num: std::os::raw::c_int,
        _idx_str: Option<&str>,
        args: &Values<'_>,
    ) -> Result<()> {
        // Build FunctionValue args from the SQLite Values handed
        // by best_index. argv order matches our set_argv_index
        // assignment, which iterates constraints in column order
        // — so args[0] is arg0, args[1] is arg1, etc.
        let mut fv_args: Vec<FunctionValue> = Vec::with_capacity(args.len());
        for v in args {
            fv_args.push(value_ref_to_function_value(v));
        }

        let iter = self.def.execute(&fv_args).map_err(|e| {
            rusqlite::Error::UserFunctionError(Box::new(
                std::io::Error::other(format!("{e:?}"))
            ))
        })?;
        self.iter = Some(iter);
        self.current = None;
        self.rowid = 0;
        self.done = false;
        self.advance()
    }

    fn next(&mut self) -> Result<()> {
        self.advance()
    }

    fn eof(&self) -> bool {
        self.done
    }

    fn column(&self, ctx: &mut Context, i: std::os::raw::c_int) -> Result<()> {
        let i = i as usize;
        let row = self.current.as_ref().ok_or_else(|| {
            rusqlite::Error::UserFunctionError(
                "ShimVTabCursor::column called with no current row".into()
            )
        })?;
        let value = row.values.get(i).cloned().unwrap_or(FunctionValue::Null);
        let out = function_value_to_tosql(value);
        ctx.set_result(&out)
    }

    fn rowid(&self) -> Result<i64> {
        Ok(self.rowid)
    }
}

/// Map shim DataType → SQLite type affinity string. The
/// returned string ends up in CREATE TABLE so SQLite uses it
/// to pick storage class. Affinities are advisory in SQLite
/// (all columns can hold any value); the values that flow
/// through column() are already correctly typed.
fn datatype_to_affinity(dt: &datafission_types::DataType) -> &'static str {
    use datafission_types::DataType as D;
    match dt {
        D::Boolean | D::Int8 | D::Int16 | D::Int32 | D::Int64
        | D::UInt8 | D::UInt16 | D::UInt32 | D::UInt64
            => "INTEGER",
        D::Float32 | D::Float64 => "REAL",
        D::Text | D::Char { .. } | D::Varchar { .. } => "TEXT",
        D::Binary => "BLOB",
        _ => "BLOB",  // arrays / structs / extensions all blob-shaped
    }
}
"##,
    );
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
    s.push_str(
r##"//! Operator handling.
//!
//! ## Phase 4d — ARCHITECTURALLY BLOCKED for a loadable
//! extension (2026-06-24)
//!
//! Operators like `g1 && g2` and `g1 <-> g2` are PARSER-LEVEL
//! tokens in PostgreSQL. SQLite's parser does not recognise
//! them — `SELECT g1 && g2` is a syntax error BEFORE any
//! function dispatch could intervene. There is no SQLite C
//! API to register a custom operator token.
//!
//! Loadable extensions cannot fix this. The bridge can register
//! a normal scalar function under any name, but the SQL parser
//! is fixed.
//!
//! ## Where this work actually lives
//!
//! Operator/cast/preprocessor support is a SQL-PREPROCESSING
//! concern, not an extension concern. The proper home is a
//! separate `sqlink-preprocess` crate that:
//!
//!   - Parses the SQL surface (via sqlparser-rs)
//!   - Rewrites operators: `g1 && g2` → `st_bboxintersects(g1, g2)`
//!   - Rewrites casts:     `CAST(x AS GEOMETRY)` → `st_geomfromtext(x)`
//!     when x is a string literal
//!   - Hands the rewritten SQL to rusqlite's `prepare`/`execute`
//!
//! That crate doesn't ship as a SQLite extension — it's a host-
//! side library users wrap their SQL with. It can absolutely be
//! generated from the same `BridgePlan` this crate emits, but
//! it's a separate target crate.
//!
//! ## The bridge-side compromise
//!
//! For users who don't want to wrap their SQL, the bridge could
//! register the operator NAMES as regular scalars under encoded
//! names:
//!
//!   register_scalar("op_amp_amp",   ...) → st_bboxintersects
//!   register_scalar("op_arrow",     ...) → st_knndistance
//!   register_scalar("op_at_gt",     ...) → st_contains
//!
//! Users write `op_amp_amp(g1, g2)` instead of `g1 && g2`. It
//! works (a function with that name is fine in SQLite) but it's
//! syntactically ugly. Not currently emitted — flip on by
//! removing the early return below.

use rusqlite::{Connection, Result};

pub fn register_all(_conn: &Connection) -> Result<()> {
    // Bridge-side compromise (op_<encoded>(args)) is deliberately
    // off by default. The proper home is sqlink-preprocess (a
    // separate sibling crate that wraps SQL before execute).
    Ok(())
}

// ----------------------------------------------------------------------
// Operators the shim advertises.
// ----------------------------------------------------------------------

"##);
    for ext in &plan.extensions {
        s.push_str(&format!("// === extension: {} ===\n", ext.name));
        for op in &ext.operators {
            s.push_str(&format!(
                "// `{}` (lhs={:?}, rhs={:?})  →  {}\n",
                op.symbol, op.lhs_type_id, op.rhs_type_id, op.function_name
            ));
        }
    }
    s
}

pub fn casts_rs(plan: &BridgePlan) -> String {
    let mut s = generated_header();
    s.push_str(
r##"//! CAST(x AS T) rewrites.
//!
//! ## Phase 4d — same blocker as operators.
//!
//! SQLite's `CAST(x AS GEOMETRY)` is parser-level — the type
//! name is consumed as a built-in token and converted at parse
//! time. There is no hook for a custom-type CAST. We cannot
//! rewrite `CAST(x AS GEOMETRY)` → `st_geomfromtext(x)` from
//! inside a loadable extension; the parse fails first.
//!
//! The proper home for cast rewrites is the same separate
//! `sqlink-preprocess` crate documented in operators.rs.

use rusqlite::{Connection, Result};

pub fn register_all(_conn: &Connection) -> Result<()> {
    Ok(())
}

// ----------------------------------------------------------------------
// Cast rewrites the shim advertises.
// ----------------------------------------------------------------------

"##);
    for ext in &plan.extensions {
        s.push_str(&format!("// === extension: {} ===\n", ext.name));
        for c in &ext.cast_rewrites {
            s.push_str(&format!(
                "// CAST(<{}> AS {}) → {} (hint: {})\n",
                c.source_kind, c.target_type, c.function_name, c.source_fn_hint
            ));
        }
    }
    s
}

pub fn preprocessors_rs(plan: &BridgePlan) -> String {
    let mut s = generated_header();
    s.push_str(
r##"//! SQL preprocessor patterns — token-level rewrites the shim
//! advertises (e.g. PostGIS's EWKT-shorthand prefix-arrow).
//!
//! ## Phase 4d — same blocker as operators / casts.
//!
//! Token-level rewrites are by definition a parser concern.
//! They cannot be implemented inside a loadable extension. See
//! operators.rs for the architectural picture and the
//! `sqlink-preprocess` proposal.

use rusqlite::{Connection, Result};

pub fn register_all(_conn: &Connection) -> Result<()> {
    Ok(())
}

// ----------------------------------------------------------------------
// Preprocessor patterns the shim advertises.
// ----------------------------------------------------------------------

"##);
    for ext in &plan.extensions {
        s.push_str(&format!("// === extension: {} ===\n", ext.name));
        for p in &ext.preprocessor_patterns {
            s.push_str(&format!("// token `{}` → {}\n", p.op_token, p.function_name));
        }
    }
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
    let primary = primary_extension_name(plan);
    let crate_name = sanitize_crate_name(&primary);
    let shim_env = format!("{}_SHIM_WASM", primary.to_uppercase().replace('-', "_"));
    let lib_name = format!("lib{}_sqlite_bridge.dylib", crate_name.replace('-', "_"));

    let mut s = String::new();
    s.push_str(&format!("# {primary}-sqlite-bridge\n\n"));
    s.push_str(&format!(
        "Generated SQLite loadable extension that bridges the **{primary}** DataFission \
         wasm shim into SQLite as native scalar functions, aggregates, and UDTFs via \
         rusqlite's `create_scalar_function` / `create_aggregate_function` / `VTab` \
         traits.\n\n"
    ));
    s.push_str("Produced by [`sqlink-shim-codegen`](https://github.com/zacharywhitley/sqlink-shim-codegen) \
                from a shim-interface SQLite database. **Do not edit by hand** — regenerate \
                from the source.\n\n");

    s.push_str("## Surface\n\n");
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

    s.push_str(&format!(
r##"
## Build

```sh
cargo build --release
```

The build needs sibling checkouts of the path-dep'd workspace
crates (`datafission-df-plugin-loader`, `datafission-df-plugin-api`,
`datafission-functions`) at `../datafission/crates/`.

## Load + use

The bridge needs the composed shim wasm at runtime; set
`{shim_env}` before `.load`:

```sh
{shim_env}=/path/to/{primary}-composed.wasm \
  sqlite3 -cmd ".load target/release/{lib_name}" :memory:
```

**macOS gotcha**: the system `/usr/bin/sqlite3` is compiled
with `-DSQLITE_OMIT_LOAD_EXTENSION` and refuses `.load`. Use
homebrew sqlite (`/opt/homebrew/Cellar/sqlite/<ver>/bin/sqlite3`)
or any distro sqlite that ships extension support.

## Regen

When the upstream shim's SQL surface changes:

```sh
cd ~/git/sqlink-shim-codegen
cargo run --release -- \
  --interface /path/to/{primary}-interface.sqlite \
  --out ~/git/{primary}-sqlite-bridge
```

The codegen pipes every emitted `.rs` through
`rustfmt --edition 2021`, so the resulting crate is
`cargo fmt -p {crate_name}-sqlite-bridge -- --check`-clean by
construction.

## Architecture

- Scalars: registered via rusqlite's
  `Connection::create_scalar_function_with_state`, dispatched
  one row at a time (SQLite is not vectorised).
- Aggregates: rusqlite's `Aggregate` trait with state held
  per-group in `Box<dyn Accumulator>`.
- UDTFs: rusqlite VTab eponymous tables; output schema inferred
  from `def.param_types()`.

## License

Apache-2.0. Generated source so the same license as the
codegen.
"##,
    ));
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
