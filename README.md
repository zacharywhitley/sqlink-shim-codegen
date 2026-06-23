# sqlink-shim-codegen

Generate a SQLite extension that bridges a DataFission wasm
shim (PostGIS, MobilityDB, …) into SQLite as native functions,
aggregates, types, operators, and virtual tables.

## Usage

```
# 1. Produce the interface DB
extract-postgis-interface \
  --wasm /path/to/postgis-shim-composed.wasm \
  --output postgis.sqlite

# 2. Generate the bridge crate
sqlink-shim-codegen \
  --interface postgis.sqlite \
  --out ./postgis-sqlite-bridge

# 3. Build the bridge (TODO: today the output is a skeleton; see
#    AGENTS.md for the implementation TODOs)
cd postgis-sqlite-bridge && cargo build --release

# 4. Load into SQLite
sqlite3 ./test.db
sqlite> .load ./target/release/libpostgis_sqlite_bridge
sqlite> SELECT ST_AsText(ST_Buffer(ST_GeomFromText('POINT(1 1)'), 0.5));
```

## What gets generated

A complete Cargo crate skeleton under `--out`:

```
postgis-sqlite-bridge/
├── Cargo.toml
├── README.md
└── src/
    ├── lib.rs               # sqlite3_extension_init + module wiring
    ├── scalars.rs           # one comment-block per scalar
    ├── aggregates.rs        # one comment-block per aggregate
    ├── table_functions.rs   # UDTFs → virtual tables
    ├── window_functions.rs
    ├── types.rs             # custom types (BLOB + affinity)
    ├── operators.rs         # parser-rewrite tables
    ├── casts.rs             # CAST() rewrite rules
    ├── preprocessors.rs     # token-level rewrites
    ├── system_catalog.rs    # spatial_ref_sys etc.
    └── spatial_indexes.rs   # R*Tree-backed vtables per index
```

Today's emit is **structure + TODO markers** — the comment
blocks list every function/type/operator the interface DB
records, with enough context for an agent to fill in the actual
SQLite C-API call. See `AGENTS.md` for the per-section
implementation path.

## What lives where

| Concern | Repo |
|---|---|
| Generic extractor + schema | `shim-interface-core` |
| Per-shim extractor binaries | `postgis-shim-interface` / `mobilitydb-shim-interface` |
| Read `.sqlite` → `BridgePlan` | `shim-bridge-codegen-core` |
| **Emit SQLite extension code** | **this repo** |
| Emit DuckDB extension code | `ducklink-shim-codegen` |
