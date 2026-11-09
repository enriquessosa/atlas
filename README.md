# Atlas

Atlas is a local-first, read-only architecture explorer for Rust repositories. It is designed to show repository and crate structure, component relationships, and the source evidence behind reported facts.

## Current functionality

Atlas extracts source-backed definitions, `use` imports and re-exports, conservative local module resolution, Cargo workspace package dependency declarations, and component/dependency queries. It also finds shortest dependency paths and reports module/package strongly connected components (cycles). Supported definitions include functions, structs, enums, unions, traits, type aliases, constants, statics, and macro definitions. Impl and trait blocks are represented as implementation/associated-item definitions without asserting semantic ownership or trait resolution.

The local resolver follows explicit `crate::`, `self::`, `super::`, uniquely proven crate-root paths, and unambiguous module aliases to accessible modules or definitions in the same crate. Visibility and conditional alternatives are checked conservatively; restricted or ambiguous targets stay unresolved. Import groups, aliases, globs, and re-exports retain normalized source spelling and evidence. Globs are not expanded. `use` syntax proves a declaration only; it does **not** prove runtime use, symbol validity, execution, or data flow.

Cargo metadata creates confirmed package dependency edges when its declared dependency path canonically matches a discovered workspace package directory. Optional and target-specific edges preserve symbolic conditions. Dependencies outside the workspace remain explicit unresolved metadata relationships. No cross-crate symbol resolution, macro expansion, compiler name/type resolution, build-script execution, or host/feature evaluation is performed. Paths and cycles traverse only confirmed/resolved relationships in one selected graph family; cycles are observations, not automatic defects.

## Commands

```text
atlas --help
atlas --version
atlas inspect [PATH]
atlas modules [PATH]
atlas component <ID> [PATH]
atlas deps <ID> [PATH]
atlas path <FROM-ID> <TO-ID> [PATH] --family modules|packages
atlas cycles [PATH] --family modules|packages
atlas diagnostics [PATH]
atlas export [PATH]
```

`inspect`, `modules`, `diagnostics`, `component`, `deps`, `path`, and `cycles` perform repository analysis and accept `--format human|json`. `deps` accepts `--reverse` plus `--family all|modules|packages`; paths and cycles require one explicit family. `export` always emits JSON. Clap/unknown IDs exit 2; failed discovery exits 4; partial analysis returns known results with exit 0 and diagnostics; output errors use 1; commands still deferred to later work use 3.

```sh
atlas inspect .
atlas modules ./Cargo.toml
atlas diagnostics . --format json
atlas component 'module:crate:demo::module:crate::parser' .
atlas deps 'module:crate:demo::module:crate::parser' --family modules
atlas deps 'module:crate:demo::module:crate::parser' --family modules --reverse --format json
atlas path 'module:crate:demo::module:crate' 'module:crate:demo::module:crate::parser' --family modules
atlas cycles . --family modules
atlas export .
```

Start with `inspect`, select a stable module ID from `modules`, inspect it with `component`, and use `deps` or `path` to follow explicit local references. A path is a graph path, never a runtime call path. “No path found” is limited to the supported graph; when dependency status is partial it is explicitly inconclusive. Conditional paths retain their symbolic conditions.

## Requirements and local installation

Atlas requires Rust 1.88 or newer. The crate uses Rust 2024 and the project declares this minimum with Cargo's `rust-version` metadata. To build and install from a local checkout:

```sh
cargo install --path . --locked --offline
atlas --help
```

Offline installation works when all locked dependencies are already cached. Atlas itself does not make network requests during analysis; Cargo metadata discovery runs offline and does not execute build scripts or repository binaries.

## Example exploration workflow

```sh
atlas inspect /path/to/repository
atlas modules /path/to/repository
atlas component '<module ID copied from modules>' /path/to/repository
atlas deps '<module ID copied from modules>' /path/to/repository --family modules
atlas path '<source module ID>' '<target module ID>' /path/to/repository --family modules
atlas cycles /path/to/repository --family modules
atlas export /path/to/repository > architecture.json
```

The IDs in angle brackets are values copied from Atlas output, not literal IDs. `component`, `deps`, `path`, and `cycles` show source locations for relevant relationships when evidence exists. JSON contains repository-relative paths, byte spans, provenance, symbolic conditions, status, and diagnostics; it does not embed source snippets or timestamps.

Atlas invokes `cargo metadata --format-version 1 --no-deps --offline --locked`, with a 30-second timeout and bounded output capture. It does not retry online. Source traversal begins at Cargo target roots and follows explicit module declarations; it does not scan every Rust file. Symlink targets must resolve inside the workspace boundary. Conditional path mappings that require configuration evaluation stay unresolved.

## Supported repository structures and output

Cargo workspace members and target crate roots come from Cargo metadata, including virtual workspaces and packages with multiple targets. From those roots Atlas follows inline modules, external `foo.rs` modules, legacy `foo/mod.rs` layouts, nested module declarations, and literal `#[path = "..."]` mappings. It does not recursively scan unrelated Rust files. Conditional modules are retained symbolically; Atlas does not select the current host configuration. A source path changed by `cfg_attr` is left unresolved if selecting it requires configuration evaluation.

`atlas export` emits schema version 1 JSON with `analysis`, `nodes`, `edges`, and `diagnostics`. The `analysis` object contains aggregate, structural, and dependency statuses. Source-derived edges carry repository-relative file paths and byte spans where available. Query JSON reports the selected result, evidence, statuses, and relevant diagnostics. Output is deterministic for an unchanged repository and contains no timestamps or default absolute paths. See [the architecture model](docs/architecture-model.md) for node, edge, identity, and status semantics.

Atlas reads repository manifests and reachable Rust source but does not modify them. Cargo runs offline in metadata-only mode. It does not execute repository binaries, build scripts, procedural macros, or arbitrary project code.

## Build and verify

With dependencies already available locally:

```sh
cargo build --locked --offline
cargo test --all-targets --locked --offline
cargo fmt --all -- --check
cargo clippy --all-targets --locked --offline -- -D warnings
```

The repository intentionally has no Nix flake yet: although Nix 2.34.9 is installed, no pinned local `nixpkgs` input could be established or validated. The configured Nix daemon socket is inaccessible in this environment, and no remote input was fetched. Add a flake only after choosing and validating a pinned offline input strategy.

## Distribution status

Atlas is licensed under MIT; see [LICENSE](LICENSE). Third-party dependencies retain their own licenses; see the [local dependency license inventory](docs/dependency-licenses.md). Source crate and prebuilt binary distributions may have different notice obligations, so review dependency notices for each distribution format.

## Current limitations

- No compiler-equivalent name/type/visibility resolution. The module resolver only recognizes a bounded syntax-driven subset.
- Imports of external crates, unresolved names, ambiguous conditional alternatives, and dependencies whose package node is absent remain unresolved and produce diagnostics. An empty result in partial analysis is not evidence that no dependency exists.
- Associated items appear under implementation or trait definitions as lexical associations only; Atlas makes no claim about which type or trait implementation owns or satisfies them semantically.
- Methods and paths inside function bodies, fields, macro output, procedural macro output, and item-generating attributes are not exhaustively modeled. Module-level invocation macros produce partial-analysis diagnostics.
- Cargo package dependency edges describe manifest declarations only. They do not establish module imports, symbol relationships, runtime calls, or that a dependency is active in the selected build.
- No macro expansion or `cfg` evaluation. A source macro may create modules, definitions, or imports that do not appear in the graph; Atlas emits diagnostics for unexpanded item macro invocations.
- `cfg_attr` source-path changes remain unresolved. A syntax error prevents traversal of further module declarations in that source file.
- IDs are deterministic for unchanged package manifests, target roots, and logical module paths. Package ID keys include repository-relative manifest location, so moving a package within a repository changes its ID.
- The JSON schema is versioned, but node fields and enum additions may evolve as dependency analysis is implemented.
- JSON and `inspect` expose an aggregate `status` plus `structural_status` and `dependency_status`. `complete` means all reachable structures/relationships in that documented Atlas scope were processed; it never means compiler-equivalent. Expected external references make dependency coverage partial while structural coverage can remain complete. Operational parse/discovery gaps can make both partial.
- Rust is the only planned language for v0.1.0.
