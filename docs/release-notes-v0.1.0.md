# Atlas v0.1.0 — release notes draft

Atlas v0.1.0 is the initial release of a local-first, read-only CLI for exploring Rust repository architecture.

## Included

- Offline Cargo workspace, package, and target discovery.
- Rust module hierarchy discovery from target roots and explicit module declarations.
- Source-backed extraction of supported definitions, imports, and re-exports.
- Conservative local module-reference resolution and Cargo workspace package dependency relationships.
- Repository summaries, module listings, component details, direct and reverse dependency queries, shortest dependency paths, and module/package cycle reports.
- Deterministic JSON graph export with repository-relative evidence, symbolic conditions, diagnostics, and structural/dependency completeness statuses.

## Requirements and operation

Rust 1.88 or newer is required. Atlas's analysis uses offline Cargo metadata and reads reachable Rust source files. It does not modify the analyzed repository, run its binaries or build scripts, or require a network connection at runtime.

Build from a source checkout with `cargo build --locked --offline` when dependencies are locally cached. See the README for command examples and output details.

## Known limitations

Atlas is not a compiler front end. It does not provide compiler-equivalent name/type/visibility resolution, expand declarative or procedural macros, evaluate all `cfg` and feature alternatives, resolve symbols across external crates, or infer runtime calls, execution order, and data flow. Unresolved and conditional relationships remain explicit; partial analysis means absence from a query is not proof of absence in the program.

The source project is licensed under MIT. Third-party dependencies have separate license terms. Review the dependency license inventory and target-specific obligations before redistributing prebuilt binaries.
