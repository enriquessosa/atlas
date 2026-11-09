# Architecture model and JSON contract

## Status and compatibility

This document describes the evolving Atlas v0.1 contract after Milestone 4. Structural and dependency direction, evidence provenance, repository-relative paths, deterministic IDs, and the rule that syntax does not imply runtime behavior are intended to remain stable. Schema version `1` remains independent of the package version. Analysis metadata additively exposes `structural_status` and `dependency_status` beside the aggregate `status`; graph schema remains version 1. These status fields and query response shapes are provisional during v0.1 development.

## Nodes and identities

The graph contains `Repository`, `Package`, `Target`, `Crate`, `Module`, `SourceFile`, and `Definition` nodes. IDs are strongly typed in Rust. Package IDs use name/version/relative manifest path; target and crate IDs include target name/kind/root. Module IDs are crate-target scoped logical `crate::...` paths. Source-file IDs use canonical repository-relative paths and can be shared across targets. Definition IDs include crate, kind, name, and declaration byte offset. Thus equal names in different modules or targets do not collide; inserting earlier source text can change definition IDs.

Supported definition kinds include functions, structs, enums, unions, traits, type aliases, constants, statics, and macro definitions. Nodes retain coarse visibility (`private`, `crate`, `public`, or `restricted`) for conservative import resolution. Private definitions resolve only within their defining module and descendants; restricted visibility other than `pub(crate)` remains unresolved. Impl blocks and trait/impl associated items are separate lexical nodes, not semantic ownership or trait-resolution claims. Local items inside function bodies and macro-generated definitions are not exhaustively modeled. Definition source spans are held on structural containment evidence.

Paths use normalized forward-slash-separated, repository-relative strings. `.` denotes the root. Absolute paths, parent traversal, and non-normalized components are rejected.

## Relationships

| Kind | Direction | Endpoints | Meaning |
|---|---|---|---|
| `contains` | parent → child | Repository→Package; Package→Target; Target→Crate; Crate→Module; Module→Module/Definition; Definition→Definition | Structural containment. |
| `declares_module` | declaring Module → child Module | Module→Module | Explicit `mod` declaration. |
| `loads_source` | Module → SourceFile | Module→SourceFile | Source mapping. |
| `imports` | importing Module → imported Module/Definition | Module→Module/Definition | Ordinary `use` syntax. |
| `reexports` | exporting Module → imported Module/Definition | Module→Module/Definition | `pub` or restricted visibility `use` syntax. |
| `references_module` | importing Module → referenced Module | Module→Module | A local module path resolved under Atlas's bounded rules. |
| `package_dependency` | dependent Package → dependency Package | Package→Package | Cargo package declaration, not a symbol or runtime call. |
| `unresolved_relationship` | source → no graph target | any source node | A source-backed declaration whose target cannot be safely represented. |

Resolved import/re-export edges retain normalized source spelling in optional edge field `syntax`, including aliases and `::*`. Their evidence points at the original `use` span. A resolved import to a module also creates a distinct `references_module` edge. Globs point only to the named module and are never expanded. Imports indicate declarations, not that a binding is used at runtime. No runtime calls, execution order, or data flow are inferred.

Unresolved relationships have `target: null` and a textual `unresolved_target`; they do not create placeholder nodes. Exact duplicate relationships merge sorted evidence. Edge identity includes kind, endpoints/description, resolution, condition, and syntax, so aliases and separate declarations remain distinguishable. Cycles are valid.

## Local resolution and Cargo dependencies

The syntax resolver operates within one crate after its reachable module graph has been discovered. It recognizes `crate::`, `self::`, repeated `super::`, uniquely declared crate-root paths, and module aliases established by uniquely resolved imports. Rust editions 2015, 2018, 2021, and 2024 are recognized. Direct Cargo dependency names and standard roots (`std`, `core`, `alloc`) are never mistaken for local modules. Leading absolute `::` imports are preserved but unresolved. A name is not resolved merely because the same final identifier exists elsewhere in the crate.

Nested groups, aliases, `self`, globs, and re-exports are preserved. A `use` path can resolve to a local module or a top-level definition. Atlas does not expand globs, resolve visibility, inspect arbitrary expression/type paths, or resolve external-crate symbols. Paths that require compiler resolution remain source-only unresolved relationships and diagnostics. Optional imports/dependencies and target conditions remain symbolic. Multiple condition alternatives for one logical target are left unresolved rather than arbitrarily selected.

Cargo metadata provides direct dependency declarations and their declared local `path`. A package edge is resolved only when canonicalizing that path identifies exactly one discovered workspace package directory with the dependency's package name. This avoids guessing from names when an outside-workspace path dependency collides with a workspace package. Optional dependencies carry an `optional_dependency` symbolic predicate. `cfg(...)` target conditions are normalized where possible; target triples are preserved symbolically. Dev/build/normal dependency kind, alias, and target spelling remain on `syntax`. Dependencies outside the workspace, or ambiguous package targets, remain unresolved source-only metadata relationships because the chosen `--no-deps` invocation does not provide external package nodes. No cross-crate symbol resolution is attempted.

## Evidence, conditions, diagnostics

Evidence records include evidence kind, provenance (`rust_syntax`, `cargo_metadata`, or `analyzer`), optional relative path, and optional half-open byte span with display line/column. JSON contains no snippets, timestamps, or default absolute filesystem paths. Import evidence covers the whole `use` declaration; each flattened path retains its own normalized spelling and the shared declaration span.

Conditions use symbolic `all`, `any`, `not`, constants, key/value or bare predicates, and `unknown`. Atlas does not evaluate these expressions or select the host configuration. Conditions from crate/module scopes propagate to definitions and imports. The edge resolution state is `resolved` for an unconditional local path, `conditional` for a known symbolic condition, and `unsupported` or `unresolved` when Atlas cannot safely establish the relationship.

Diagnostics use stable codes, severity, phase, message, optional relative path/span, and related node IDs. Unresolved imports remain in the graph and carry diagnostics; expected external crate imports are informational, while unresolved local imports and ambiguity are warnings. Parsing/discovery failures are errors. Human output groups by diagnostic code and displays representative evidence; JSON retains every diagnostic. `structural_status` reflects repository/package/target/module/source discovery; `dependency_status` reflects supported dependency relationships. Aggregate `status` is failed if operational discovery fails, partial if either scope is partial, otherwise complete. Expected external imports make dependency status partial but do not make structural status partial. This is still Atlas-scope completeness, never compiler-equivalent. Empty query results on partial dependency analysis do not establish absence.

## JSON and queries

The graph export envelope has `schema_version`, `analysis`, `nodes`, `edges`, and `diagnostics`. Nodes, edges, evidence, and diagnostics sort deterministically. Enum variants serialize as `snake_case`. Query JSON reports schema version and analysis status. `path` performs iterative breadth-first search over only `references_module` edges for `modules` or `package_dependency` edges for `packages`; deterministic tie-breaking uses target node ID then edge ID. The first shortest path is returned. A conditional edge makes the result configuration-dependent. A not-found result means no path in the supported graph; `absence_is_inconclusive` is true when dependency analysis is partial. A zero-length path from a node to itself is found.

`cycles --family modules|packages` computes strongly connected components using iterative Kosaraju traversal. It reports components with more than one node and single-node self-loops, with internal edges and evidence. This is a structural observation, not a defect classification. Path and cycle query JSON include statuses and symbolic edge conditions; they do not include source snippets or machine-local paths.

`atlas component <ID> [PATH]` reports structural parents/children, supported definitions, direct relationships/dependents, unresolved edges, evidence, and relevant diagnostics. `atlas deps <ID> [PATH]` lists outgoing relationships by default; `--reverse` lists incoming dependents. `--family all|modules|packages` selects dependency families. `atlas path <FROM-ID> <TO-ID> [PATH] --family modules|packages` finds a shortest supported dependency path. `atlas cycles [PATH] --family modules|packages` reports cycles. Successful partial analysis exits 0; usage errors and unknown IDs exit 2; unimplemented commands exit 3; failed discovery exits 4; output errors exit 1.

## Discovery boundary

Atlas invokes `cargo metadata --format-version 1 --no-deps --offline --locked` with bounded time and output. It does not retry online, execute build scripts, expand macros, or run project binaries. Workspace packages and target roots come from Cargo metadata. Source traversal follows declared module paths only. Symlink sources must remain within the workspace root; lexical paths are retained for child-module lookup and canonical relative paths identify physical files.

`complete` means every reachable source for discovered targets was processed within the documented syntax scope; it does not mean successful compilation or semantic completeness. Macro invocations may introduce unseen modules, imports, or definitions, and diagnostics mark observed item-macro limitations.

## Release preparation notes

Nix 2.34.9 is installed, but this environment has no existing flake or established pinned local nixpkgs input. The Nix daemon socket is inaccessible, so an offline package/shell evaluation could not be validated; no flake is supplied. `Cargo.lock` pins Rust dependencies for Cargo's locked offline build path.

Atlas's own source is licensed under MIT; the notice currently matches the 2026 attribution convention established in Pulse and Drift. Third-party dependencies retain their own licenses. Cached metadata inventory is not a legal-compliance conclusion: before each distribution format, verify all direct/transitive license and notice obligations, including source crate versus prebuilt binary packaging requirements.

See [the locked dependency license inventory](dependency-licenses.md) for the locally verified expressions and the two Windows-target packages whose source license metadata was unavailable offline.
