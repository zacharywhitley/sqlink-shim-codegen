# Agent guide — sqlink-shim-codegen

This crate emits a SQLite extension that bridges a DataFission
wasm shim into SQLite. Today's emit is a **structural skeleton
with TODOs**; the implementation work below is what fills it in.

## Read this first

See `~/git/shim-bridge-codegen-core/PIPELINE.md` for the
six-repo map. This crate is one of two per-target codegens
(peer: `ducklink-shim-codegen`).

Pipeline:

```
shim.wasm
  └► postgis-shim-interface / mobilitydb-shim-interface  ─►  *.sqlite
        └► shim-bridge-codegen-core::load_plan          ─►  BridgePlan
              └► sqlink-shim-codegen (THIS REPO)        ─►  generated bridge crate
                    └► cargo build --release            ─►  libfoo_sqlite_bridge.so
                          └► SQLite .load                ─►  ST_Intersects etc. callable
```

The generated crate's runtime needs to:
1. Embed a wasmtime instance and load the shim.
2. Register every function/aggregate/UDTF the BridgePlan
   listed.
3. For each call, marshal SQLite values into the columnar batch
   the shim consumes, then unmarshal the result.

## SQLite-specific quirks you can't gloss over

These bite anyone trying to wrap PostgreSQL-flavoured surfaces
into SQLite:

1. **No custom operators.** `a && b` is a parse error in SQLite.
   Either run queries through sqlink's preprocessor to rewrite
   them to `op_and(a, b)`, or document that users must write the
   function form. The preprocessor path is preferred.

2. **No first-class custom types.** SQLite has 5 storage
   classes; `GEOMETRY` columns end up as `BLOB` with a sidecar
   type-affinity hint. Type round-trips work because the shim's
   own binary payload (EWKB) carries the type tag.

3. **CAST() is fixed.** SQLite's CAST is built into the parser;
   you can't add `CAST(x AS GEOMETRY)` as an extension point.
   Same fix as operators: preprocessor rewrite to
   `ST_GeomFromText(x)` (or whatever the cast-rewrite table
   says) before SQLite sees the SQL.

4. **STRICT mode tightens the loose-typing escape hatch.**
   Bridges should declare custom types as `BLOB` (the only
   storage class compatible with arbitrary payloads) and NOT
   try to use the affinity-name hint in STRICT tables.

5. **Aggregates need finalize-or-fail.** `xFinal` is called
   even on errors; the shim's accumulator state must be
   destructible without a successful finalize.

6. **Virtual tables for UDTFs and system catalog.** Each gets a
   sqlite3_module. The xBestIndex hook is where you tell SQLite
   how the shim wants args filtered; for trivial UDTFs it's
   "give me every arg as positional and I'll do the filter".

## How a scalar is dispatched at runtime

Generated code for one scalar:

```rust
// Inside sqlite3_extension_init:
let user_data = Box::into_raw(Box::new(ScalarDispatcher {
    shim:  /* Rc<RuntimeWasmExtension> */,
    name:  "st_intersects",
}));
sqlite3_create_function_v2(
    db,
    b"ST_Intersects\0".as_ptr() as _,
    2,                                 // arity
    SQLITE_UTF8 | SQLITE_DETERMINISTIC,
    user_data as _,
    Some(scalar_dispatch),
    None, None, Some(drop_dispatcher),
);
```

The `scalar_dispatch` callback:

1. Pulls each arg via `sqlite3_value_*` into a `TargetValueKind`
   from `shim-bridge-codegen-core::marshal`.
2. Builds a 1-row batch (today: hand-off to
   `df-plugin-loader`'s scalar-invoke helper, which already
   knows the wire format; tomorrow:
   `WireBatchBuilder::finalize` once it's no longer a stub).
3. Calls the shim function via the loader's dispatch surface.
4. Reads the 1-row result back, calls `sqlite3_result_*`.

## How operators / casts get rewritten

The bridge depends on sqlink's parser-preprocessor hook (this
is what makes "sqlink-shim-codegen" different from a plain
SQLite extension generator). The generated `operators.rs` and
`casts.rs` build a static rewrite table; the bridge registers it
with the preprocessor at extension-init time. No SQLite C API is
involved.

If the host using the bridge can't run the preprocessor (e.g.
calling SQLite directly from C without sqlink), the rewrites
don't fire and users have to write function form.

## TODO list — what to implement next

The skeleton compiles and is documented. To make it produce
working bridges:

### Phase 1 — scalar dispatch ✅ LANDED 2026-06-23

- [x] Add `datafission-df-plugin-loader` as a path-dep to the
      generated `Cargo.toml` (see `emit::cargo_toml`).
- [x] In `emit::lib_rs`, emit the real `sqlite3_extension_init`
      function with shim instantiation (uses rusqlite's
      `Connection::extension_init2` to handle the
      `SQLITE_EXTENSION_INIT2` macro plumbing).
- [x] Emit a new `registry` module that loads the composed
      shim once on init, walks every `register_scalar_function`
      callback via a minimal `ExtensionTarget`, and exposes
      `lookup_scalar(name) -> Arc<dyn ScalarFunctionDef>` for
      the per-call dispatcher.
- [x] In `emit::scalars_rs`, emit `Connection::create_scalar_function`
      calls + a `dispatch_scalar` closure that marshals
      SQLite `ValueRef` → `FunctionValue`, invokes
      `ScalarFunctionDef::execute`, marshals the result back
      to `ToSqlOutput`. One registration per canonical name +
      one per alias.
- [x] Verified end-to-end: the generated bridge against the
      live PostGIS interface DB `cargo check`s clean against
      df-plugin-loader and rusqlite. `ST_GeomFromText` and its
      14 aliases are wired live.

#### Phase 1 runtime contract

The generated bridge expects the composed **shim** wasm
(NOT the upstream postgis-wasm composed) at the path in env
var `<EXT>_SHIM_WASM`. The shim composed is what `wac plug`
produces from the shim's `postgis.wasm` plus the upstream
`postgis-composed.wasm`:

```sh
# 1. Build the shim
cd $HOME/git/datafission/extensions/postgis
cargo build --release --target wasm32-wasip2

# 2. wac plug the shim against upstream
wac plug --plug deps/postgis-composed.wasm \
  target/wasm32-wasip2/release/postgis.wasm \
  -o /tmp/postgis-shim-composed.wasm

# 3. Use the shim composed in the env var
export POSTGIS_SHIM_WASM=/tmp/postgis-shim-composed.wasm

# 4. Load + smoke (needs sqlite3 with extension support;
#    macOS system sqlite3 has -DSQLITE_OMIT_LOAD_EXTENSION,
#    use brew sqlite at /opt/homebrew/opt/sqlite/bin)
/opt/homebrew/opt/sqlite/bin/sqlite3 :memory: <<SQL
.load ./target/release/libpostgis_sqlite_bridge
SELECT length(ST_GeomFromText('POINT(1 1)'));        -- → 21
SELECT hex(ST_GeomFromText('POINT(1 1)'));           -- → 01010...F03F
SELECT length(ST_GeomFromText('POLYGON((0 0, 4 0, 4 4, 0 4, 0 0))'));  -- → 93
SELECT typeof(ST_GeomFromText('POINT(1 1)'));        -- → blob
SQL
```

Verified 2026-06-24: smoke produces correct WKB for POINT,
LINESTRING, POLYGON. All 14 aliases dispatch through the same
ScalarFunctionDef. Invalid input propagates a clean error back
to SQLite.

#### Phase 1 known limitations (intentional — defer)

- Other scalar names are listed as comments in `scalars.rs`
  but not registered. To turn one on, copy the
  `register_scalar(...)` line in `register_all` and adapt the
  arity. Each new name should JUST WORK if the shim wrapped it
  — the dispatch is fully generic.
- macOS system sqlite3 has `-DSQLITE_OMIT_LOAD_EXTENSION`; use
  brew's sqlite3 at `/opt/homebrew/opt/sqlite/bin/sqlite3`.
- cdylib size: 11 MB stripped (LTO + opt-level "z"). Build
  target/ is ~3 GB during compile because wasmtime +
  postgis-wasm transitives are heavy. Acceptable for now.

### Phase 2 — aggregates

- [ ] Step/final dispatcher; thread accumulator state through
      `sqlite3_aggregate_context`.
- [ ] For window-capable aggregates, use
      `sqlite3_create_window_function`.

### Phase 3 — virtual tables

- [ ] UDTFs as virtual tables (one module per function).
- [ ] System catalog tables (one module per table).
- [ ] Spatial indexes: one R*Tree virtual table per shim
      index + the trigger pair that maintains it. For non-
      rectangular indexes (KdTree, Quadtree, Octree) the
      shim's CPU query takes over via a UDTF that the
      operator-rewrite layer routes to.

### Phase 4 — types / operators / casts / preprocessors

- [ ] Wire the rewrite tables to sqlink's preprocessor hook.
- [ ] Documentation in the generated README on how to use the
      bridge from a host that doesn't run sqlink.

## Things NOT to do

- **Don't dispatch one row at a time forever.** The 1-row batch
  is for the initial shim; the architecture supports batched
  calls via SQLite's UDTF interface. When perf matters, fan
  inputs into a vtable that pulls N rows at once.
- **Don't ignore NULL.** SQLite NULL must map to "no value" in
  the columnar batch's null bitmap, not a zero blob. Many shim
  functions are NULL-propagating; honor `propagates_null`.
- **Don't hard-code function names.** Aliases are extension-
  declared; emit one registration per alias.
- **Don't emit text-encoded payloads.** Pass EWKB/MFJSON blobs
  through verbatim. The shim parses them.
- **Don't add a wasmtime dep here.** The codegen crate is
  pure-data; the wasmtime dep belongs in the GENERATED bridge
  crate (which path-deps `df-plugin-loader`).

## Verifying the skeleton compiles

```
cargo check         # this crate
cargo run -- --help # CLI works
cargo run -- --interface /tmp/postgis-interface.sqlite \
             --out /tmp/postgis-bridge-skel
cd /tmp/postgis-bridge-skel && cargo check
# Skeleton compiles even though it has no real impl yet.
```

(Today the generated crate won't `cargo check` because its
TODOs reference deps that aren't declared. That's the first
thing Phase 1 fixes.)

## Reference points

- SQLite extension API: https://www.sqlite.org/c3ref/intro.html
- `sqlite3_create_function_v2`: https://www.sqlite.org/c3ref/create_function.html
- Virtual tables: https://www.sqlite.org/vtab.html
- The DataFission loader (`df-plugin-loader`): provides the
  scalar/aggregate invoke helpers the dispatcher closures call.
- `~/git/datafission/crates/df-plugin-api/src/extension.rs`:
  source of truth for what shim authors can advertise.
