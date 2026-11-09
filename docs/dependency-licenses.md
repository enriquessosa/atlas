# Locked dependency license inventory

This inventory covers the 31 registry packages in `Cargo.lock` as inspected from locally cached crate manifests on 2026-11-09. It is a technical inventory, not a legal-compliance conclusion. License expressions below are copied from package `Cargo.toml` metadata; local license files were present for all 29 cached source packages.

| Declared expression | Packages and versions |
|---|---|
| `MIT OR Apache-2.0` | anstream 1.0.0; anstyle 1.0.14; anstyle-parse 1.0.0; anstyle-query 1.1.5; clap 4.6.7; clap_builder 4.6.7; clap_derive 4.6.7; clap_lex 1.1.1; colorchoice 1.0.5; heck 0.5.0; is_terminal_polyfill 1.70.2; itoa 1.0.18; proc-macro2 1.0.107; quote 1.0.47; serde 1.0.229; serde_core 1.0.229; serde_derive 1.0.229; serde_json 1.0.154; syn 2.0.119; syn 3.0.9; thiserror 2.0.21; thiserror-impl 2.0.21; windows-link 0.2.1; windows-sys 0.61.2 |
| `MIT` | strsim 0.11.1; zmij 1.0.23 |
| `Unlicense OR MIT` | memchr 2.8.3 |
| `Apache-2.0 OR MIT` | utf8parse 0.2.2 |
| `(MIT OR Apache-2.0) AND Unicode-3.0` | unicode-ident 1.0.27 |
| Not verified locally | anstyle-wincon 3.0.11; once_cell_polyfill 1.70.2 |

For the 29 cached source packages, license-named files were present. `unicode-ident` includes `LICENSE-UNICODE` in addition to its MIT and Apache license files; its conjunctive expression requires attention to the Unicode terms. For `MIT OR Apache-2.0` and `Unlicense OR MIT`, choosing the MIT option is possible for redistribution, subject to preserving the required notice. `utf8parse` also permits the MIT option. `strsim` and `zmij` declare MIT.

The local Cargo source archive and extracted source for `anstyle-wincon` and `once_cell_polyfill` were absent. Cached registry-index records identified versions and checksums but did not provide license expressions or license text. The lockfile and cached dependency metadata show these are in the optional Windows terminal dependency chain; do not infer their licenses from `anstream` or other neighboring packages. Obtain and inspect their exact source metadata before distributing a Windows binary.

The Atlas source crate contains Atlas's MIT license and its own notice. It does not bundle third-party dependency source or notices. Before distributing prebuilt binaries, review the exact linked dependency set for each target and provide any notices required by those dependencies. The available information is not sufficient to certify source or binary redistribution compliance, particularly for the two unverified Windows-target packages.
