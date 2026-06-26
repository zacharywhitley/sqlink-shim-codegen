//! Phase 2 scalar-dispatch registry.
//!
//! Each `DispatchEntry` says: "when the host calls the scalar
//! named `sql_name`, marshal the SQL value args per `shape`,
//! invoke `wit_module::wit_func`, and marshal the result back to
//! a SQL value." The emitter walks this registry and emits one
//! `match` arm per entry in the scalar-function dispatcher; SQL
//! names not present in the registry still fall through to the
//! Phase 1 stub-error path so the surface stays loadable.
//!
//! The registry is hand-curated for Phase 2 — small enough to
//! prove dispatch works end-to-end across the major
//! type-marshaling shapes (text → geometry, geometry → text,
//! geometry → f64, geometry+geometry → f64, f64+f64 → geometry).
//! Phase 3 grows it to the full ~317-function surface, ideally
//! derived from the WIT files at codegen time rather than
//! hand-listed here.
//!
//! Type-mapping table (interface-DB type → WIT type → SqlValue):
//!
//!   text     → string         → SqlValue::Text(String)
//!   float64  → f64            → SqlValue::Real(f64)
//!   int64    → s64            → SqlValue::Integer(i64)
//!   uint32   → u32            → SqlValue::Integer(i64)
//!   int32    → s32            → SqlValue::Integer(i64)
//!   boolean  → bool           → SqlValue::Integer(0|1)
//!   binary   → list<u8>       → SqlValue::Blob(Vec<u8>)
//!     ↑ for the postgis-wasm WIT, "binary" at the interface-DB
//!       layer is the WKB-encoded form of a `geometry` resource.
//!       Crossing the WIT boundary requires reconstituting the
//!       resource via `Geometry::from_wkb(&bytes)?` and
//!       serializing the result back via `g.as_wkb()`.

/// A single function the codegen emits real dispatch for.
pub struct DispatchEntry {
    /// Matches the `scalars.name` column of the interface DB.
    /// Used to look up the runtime func-id assigned by the
    /// metadata emitter.
    pub sql_name: &'static str,
    /// The shape determines argument unpacking + result wrapping.
    pub shape: DispatchShape,
}

/// What the emitted match arm does. Each variant names the
/// imported function the dispatcher calls; the param/return
/// marshaling is fixed by the shape.
#[allow(dead_code)]
pub enum DispatchShape {
    /// `f(wkt: text) -> result<geometry, postgis-error>` →
    /// returns SqlValue::Blob(WKB).
    TextToGeomResult { wit_module: &'static str, wit_func: &'static str },
    /// `f(geom: borrow<geometry>) -> string` → returns
    /// SqlValue::Text.
    GeomToString { wit_module: &'static str, wit_func: &'static str },
    /// `f(geom: borrow<geometry>) -> result<string, postgis-error>` → returns
    /// SqlValue::Text.
    GeomToStringResult { wit_module: &'static str, wit_func: &'static str },
    /// `f(geom: borrow<geometry>) -> result<f64, postgis-error>` → returns
    /// SqlValue::Real.
    GeomToF64Result { wit_module: &'static str, wit_func: &'static str },
    /// `f(geom1: borrow<geometry>, geom2: borrow<geometry>) -> result<f64, postgis-error>` →
    /// returns SqlValue::Real.
    GeomGeomToF64Result { wit_module: &'static str, wit_func: &'static str },
    /// `f(geom: borrow<geometry>) -> u32` → returns
    /// SqlValue::Integer.
    GeomToU32 { wit_module: &'static str, wit_func: &'static str },
    /// `f(geom: borrow<geometry>) -> bool` → returns
    /// SqlValue::Integer(0|1).
    GeomToBool { wit_module: &'static str, wit_func: &'static str },
    /// `f(x: f64, y: f64) -> geometry` → returns SqlValue::Blob(WKB).
    F64F64ToGeom { wit_module: &'static str, wit_func: &'static str },
    /// `f(geom: borrow<geometry>) -> result<geometry, postgis-error>` →
    /// returns SqlValue::Blob(WKB).
    GeomToGeomResult { wit_module: &'static str, wit_func: &'static str },
}

/// Phase 2 registry. Returns the list of scalars the emitter
/// should generate real dispatch for; everything else stays
/// stubbed.
///
/// Picked to cover every type-marshaling shape Phase 2 needs to
/// prove:
///   - `st_geomfromtext`: text → geometry (the "ingress" half of
///     the round-trip);
///   - `st_astext`: geometry → text (the "egress" half);
///   - `st_distance`: geometry+geometry → f64 (binary-pair input,
///     numeric output);
///   - `st_x`: geometry → f64 (single-binary input, numeric output);
///   - `st_area`: geometry → f64 (different importing WIT module —
///     postgis-measurements — proves the dispatch table can route
///     to multiple WIT interfaces);
///   - `st_makepoint`: f64+f64 → geometry (numeric input, binary
///     output; symmetric to `st_x`).
///   - `st_geomfromewkt`: text → geometry (second Result-text shape).
///   - `st_isempty`: geometry → bool (bool result).
///   - `st_npoints`: geometry → u32 (uint result).
///   - `st_centroid`: geometry → geometry (round-trips a resource).
pub fn registry() -> &'static [DispatchEntry] {
    use DispatchShape::*;
    &[
        DispatchEntry {
            sql_name: "st_geomfromtext",
            shape: TextToGeomResult {
                wit_module: "pg_ctor",
                wit_func: "st_geom_from_text",
            },
        },
        DispatchEntry {
            sql_name: "st_astext",
            shape: GeomToString {
                wit_module: "pg_out",
                wit_func: "st_as_text",
            },
        },
        DispatchEntry {
            sql_name: "st_asewkt",
            shape: GeomToString {
                wit_module: "pg_out",
                wit_func: "st_as_ewkt",
            },
        },
        DispatchEntry {
            sql_name: "st_asgeojson",
            shape: GeomToString {
                wit_module: "pg_out",
                wit_func: "st_as_geojson",
            },
        },
        DispatchEntry {
            sql_name: "st_distance",
            shape: GeomGeomToF64Result {
                wit_module: "pg_meas",
                wit_func: "st_distance",
            },
        },
        DispatchEntry {
            sql_name: "st_x",
            shape: GeomToF64Result {
                wit_module: "pg_acc",
                wit_func: "st_x",
            },
        },
        DispatchEntry {
            sql_name: "st_y",
            shape: GeomToF64Result {
                wit_module: "pg_acc",
                wit_func: "st_y",
            },
        },
        DispatchEntry {
            sql_name: "st_xmin",
            shape: GeomToF64Result {
                wit_module: "pg_acc",
                wit_func: "st_xmin",
            },
        },
        DispatchEntry {
            sql_name: "st_xmax",
            shape: GeomToF64Result {
                wit_module: "pg_acc",
                wit_func: "st_xmax",
            },
        },
        DispatchEntry {
            sql_name: "st_ymin",
            shape: GeomToF64Result {
                wit_module: "pg_acc",
                wit_func: "st_ymin",
            },
        },
        DispatchEntry {
            sql_name: "st_ymax",
            shape: GeomToF64Result {
                wit_module: "pg_acc",
                wit_func: "st_ymax",
            },
        },
        DispatchEntry {
            sql_name: "st_area",
            shape: GeomToF64Result {
                wit_module: "pg_meas",
                wit_func: "st_area",
            },
        },
        DispatchEntry {
            sql_name: "st_length",
            shape: GeomToF64Result {
                wit_module: "pg_meas",
                wit_func: "st_length",
            },
        },
        DispatchEntry {
            sql_name: "st_perimeter",
            shape: GeomToF64Result {
                wit_module: "pg_meas",
                wit_func: "st_perimeter",
            },
        },
        DispatchEntry {
            sql_name: "st_makepoint",
            shape: F64F64ToGeom {
                wit_module: "pg_ctor",
                wit_func: "st_make_point",
            },
        },
        DispatchEntry {
            sql_name: "st_npoints",
            shape: GeomToU32 {
                wit_module: "pg_acc",
                wit_func: "st_npoints",
            },
        },
        DispatchEntry {
            sql_name: "st_numgeometries",
            shape: GeomToU32 {
                wit_module: "pg_acc",
                wit_func: "st_num_geometries",
            },
        },
        DispatchEntry {
            sql_name: "st_isempty",
            shape: GeomToBool {
                wit_module: "pg_pred",
                wit_func: "st_is_empty",
            },
        },
        DispatchEntry {
            sql_name: "st_centroid",
            shape: GeomToGeomResult {
                wit_module: "pg_proc",
                wit_func: "st_centroid",
            },
        },
    ]
}

/// Emit the body of one match arm — the code that runs inside
/// `match func_id { N => { ... } }`. `arm_indent` is the
/// whitespace prefix for each line so the emitted source stays
/// rustfmt-clean.
pub fn emit_arm_body(shape: &DispatchShape, sql_name: &str, arm_indent: &str) -> String {
    match shape {
        DispatchShape::TextToGeomResult { wit_module, wit_func } => format!(
            "{i}let wkt = arg_text(&args, 0, \"{sql_name}\")?;\n\
             {i}let g = {wit_module}::{wit_func}(wkt)\n\
             {i}    .map_err(|e| format!(\"{sql_name}: {{}}\", postgis_err_string(e)))?;\n\
             {i}Ok(SqlValue::Blob(g.as_wkb()))",
            i = arm_indent,
            wit_module = wit_module,
            wit_func = wit_func,
            sql_name = sql_name,
        ),
        DispatchShape::GeomToString { wit_module, wit_func } => format!(
            "{i}let g = from_wkb(arg_blob(&args, 0, \"{sql_name}\")?, \"{sql_name}\")?;\n\
             {i}Ok(SqlValue::Text({wit_module}::{wit_func}(&g)))",
            i = arm_indent,
            wit_module = wit_module,
            wit_func = wit_func,
            sql_name = sql_name,
        ),
        DispatchShape::GeomToStringResult { wit_module, wit_func } => format!(
            "{i}let g = from_wkb(arg_blob(&args, 0, \"{sql_name}\")?, \"{sql_name}\")?;\n\
             {i}let s = {wit_module}::{wit_func}(&g)\n\
             {i}    .map_err(|e| format!(\"{sql_name}: {{}}\", postgis_err_string(e)))?;\n\
             {i}Ok(SqlValue::Text(s))",
            i = arm_indent,
            wit_module = wit_module,
            wit_func = wit_func,
            sql_name = sql_name,
        ),
        DispatchShape::GeomToF64Result { wit_module, wit_func } => format!(
            "{i}let g = from_wkb(arg_blob(&args, 0, \"{sql_name}\")?, \"{sql_name}\")?;\n\
             {i}let r = {wit_module}::{wit_func}(&g)\n\
             {i}    .map_err(|e| format!(\"{sql_name}: {{}}\", postgis_err_string(e)))?;\n\
             {i}Ok(SqlValue::Real(r))",
            i = arm_indent,
            wit_module = wit_module,
            wit_func = wit_func,
            sql_name = sql_name,
        ),
        DispatchShape::GeomGeomToF64Result { wit_module, wit_func } => format!(
            "{i}let a = from_wkb(arg_blob(&args, 0, \"{sql_name}\")?, \"{sql_name}\")?;\n\
             {i}let b = from_wkb(arg_blob(&args, 1, \"{sql_name}\")?, \"{sql_name}\")?;\n\
             {i}let r = {wit_module}::{wit_func}(&a, &b)\n\
             {i}    .map_err(|e| format!(\"{sql_name}: {{}}\", postgis_err_string(e)))?;\n\
             {i}Ok(SqlValue::Real(r))",
            i = arm_indent,
            wit_module = wit_module,
            wit_func = wit_func,
            sql_name = sql_name,
        ),
        DispatchShape::GeomToU32 { wit_module, wit_func } => format!(
            "{i}let g = from_wkb(arg_blob(&args, 0, \"{sql_name}\")?, \"{sql_name}\")?;\n\
             {i}Ok(SqlValue::Integer({wit_module}::{wit_func}(&g) as i64))",
            i = arm_indent,
            wit_module = wit_module,
            wit_func = wit_func,
            sql_name = sql_name,
        ),
        DispatchShape::GeomToBool { wit_module, wit_func } => format!(
            "{i}let g = from_wkb(arg_blob(&args, 0, \"{sql_name}\")?, \"{sql_name}\")?;\n\
             {i}Ok(SqlValue::Integer({wit_module}::{wit_func}(&g) as i64))",
            i = arm_indent,
            wit_module = wit_module,
            wit_func = wit_func,
            sql_name = sql_name,
        ),
        DispatchShape::F64F64ToGeom { wit_module, wit_func } => format!(
            "{i}let x = arg_f64(&args, 0, \"{sql_name}\")?;\n\
             {i}let y = arg_f64(&args, 1, \"{sql_name}\")?;\n\
             {i}Ok(SqlValue::Blob({wit_module}::{wit_func}(x, y).as_wkb()))",
            i = arm_indent,
            wit_module = wit_module,
            wit_func = wit_func,
            sql_name = sql_name,
        ),
        DispatchShape::GeomToGeomResult { wit_module, wit_func } => format!(
            "{i}let g = from_wkb(arg_blob(&args, 0, \"{sql_name}\")?, \"{sql_name}\")?;\n\
             {i}let r = {wit_module}::{wit_func}(&g)\n\
             {i}    .map_err(|e| format!(\"{sql_name}: {{}}\", postgis_err_string(e)))?;\n\
             {i}Ok(SqlValue::Blob(r.as_wkb()))",
            i = arm_indent,
            wit_module = wit_module,
            wit_func = wit_func,
            sql_name = sql_name,
        ),
    }
}
