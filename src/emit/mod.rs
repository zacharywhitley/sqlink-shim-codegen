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
crate-type = ["cdylib"]

[dependencies]
# TODO: depend on the DataFission df-plugin-loader (path or git)
#       so the bridge can host the wasm shim at runtime.
# datafission-df-plugin-loader = {{ path = "../datafission/crates/df-plugin-loader" }}
# rusqlite = {{ version = "0.32", features = ["bundled", "loadable_extension"] }}
# anyhow = "1"
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
//! Load with `.load ./target/release/lib<name>_sqlite_bridge`.

mod scalars;
mod aggregates;
mod table_functions;
mod window_functions;
mod types;
mod operators;
mod casts;
mod preprocessors;
mod system_catalog;
mod spatial_indexes;

// TODO: wire up sqlite3_extension_init.
//
// The expected shape (see https://www.sqlite.org/loadext.html):
//
//   #[no_mangle]
//   pub extern "C" fn sqlite3_<name>_init(
//       db: *mut sqlite3,
//       err_msg: *mut *mut c_char,
//       api: *const sqlite3_api_routines,
//   ) -> c_int {
//       sqlite3_api = api;
//       // 1. Instantiate the wasm shim via df-plugin-loader.
//       // 2. Call scalars::register_all(db, &ext)?
//       // 3. Call aggregates::register_all(db, &ext)?
//       // ...
//       SQLITE_OK
//   }

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
    s.push_str(r##"//! Scalar-function registration.
//!
//! For each scalar, we want to call:
//!
//!   sqlite3_create_function_v2(
//!     db, "name", arg_count, SQLITE_UTF8 | SQLITE_DETERMINISTIC,
//!     user_data_ptr,  // boxed dispatch closure
//!     dispatch_fn,    // pulls args, builds 1-row batch, calls shim
//!     null,           // step
//!     null,           // final
//!     destroy_fn);    // drops the boxed closure
//!
//! See AGENTS.md → "How a scalar is dispatched at runtime" for
//! the call shape.

"##);
    for ext in &plan.extensions {
        s.push_str(&format!("// === extension: {} ===\n", ext.name));
        for sc in &ext.scalars {
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
    s.push_str(
r##"// TODO: actually emit registration calls. The data above is
// what every scalar needs; the per-target binding is
// sqlite3_create_function_v2. The dispatcher closure follows
// the "1-row batch" model in AGENTS.md.
"##,
    );
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
