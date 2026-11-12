# horizon-axiom

Unofficial Rust port of the server side of [Axiom](https://axiom.moulberry.com/) (AxiomPaperPlugin, MIT) for
[Horizon](https://github.com/arismoko/horizon). Not affiliated with Moulberry; ported with their permission.

Axiom's licensing and whitelist are untouched: the server plugin contains no license check, and this port adds or
removes none. Commercial use of Axiom needs Axiom's Commercial License: https://axiomdocs.moulberry.com/other/commerciallicense.html

> **Built for and working only with [arismoko/horizon](https://github.com/arismoko/horizon)** (the Horizon fork that carries the mod API). [horizon-rust/horizon](https://github.com/horizon-rust/horizon) is experimental for mods and not supported: the earlier version for its native hooks / horizon-mixins is frozen on the `horizon-plain` branch (work stopped on 2026-11-12).

## Which Horizon

This is a native trait mod: `horizon-mod.toml`, a `Cargo.toml` that depends on `horizon-mod-api`, `src/lib.rs` with the hooks, and the plugin logic in the `core/` crate (plain Rust, no engine needed for its tests).

- Pin: `[native] horizon = "54090ee36c4e2c7b43b66db864220c577b94b7e0"` in `horizon-mod.toml` (game `26.3`). That is a development build of arismoko/horizon `main` plus mod API additions that are not all merged upstream yet, so a stock `main` may lack some host methods this mod calls until they land.
- It compiled and loaded on that build and was smoke-tested there; it has not seen a long run with real players. Treat it as experimental.
- Feature descriptions below were written for the earlier horizon-rust version; where they name engine calls (`native_*`) or old file paths, read them as descriptions only. The Known gaps list is current.

## Third-party data

`core/assets/block_entities_v1.dict` is the zstd dictionary for block entity NBT, copied unchanged from
AxiomPaperPlugin (`src/main/resources/zstd_dictionaries/block_entities_v1.dict`, MIT, Copyright (c) 2023 Moulberry Pty Ltd).
It is embedded with `include_bytes!`. The MIT notice for AxiomPaperPlugin is in [LICENSE](LICENSE).
Compression and decompression use `ruzstd` 0.9 (MIT, pure Rust).

## Build

Build it as a native trait mod with arismoko's launcher (see the "native mods" how-to in the arismoko/horizon docs):

```sh
horizon-launcher native-build <path to this folder> --server-output <dir> --version <tag> --trust-native-code
```

`--trust-native-code` is your consent to compile and run this code inside the server. The logic crate tests run without the engine: `(cd core && cargo test)`.

## Known gaps

What does not work yet on arismoko/horizon (each is marked `TODO(arismoko-api: ...)` in `src/lib.rs`):

- Block entity data (chests, signs, ...) is neither read nor written by block edits, so blueprints lose block entities; no data fixer for old block-entity or entity NBT.
- Blueprints cannot save or load block states (states have no numeric id on arismoko), and older clients are refused (no per-release block mapping).
- No cancel hook for Axiom's entity spawns; hanging-entity facing is not read or set; markers cannot be listed.
- Region and plot checks (WorldGuard-style) are not seen; messages are plain text without colours.
- Chunk tickets, the game protocol version and the mod data folder are approximated.

## Licence

MIT. Based on AxiomPaperPlugin (Copyright (c) 2023 Moulberry Pty Ltd, MIT); see `LICENSE`.
