# hangar

Archive tool. Smaller than ZIP, faster than RAR, beats 7z on max ratio.

Engine is [Zstandard](https://facebook.github.io/zstd/) wrapped in a small random-access container (`.hgr`). Two surfaces share the same Rust core: a CLI (`hgr`) and a Tauri desktop app (`Hangar`).

## Build

```bash
cargo build --release
```

Outputs:

- `target/release/hgr` — CLI binary
- `target/release/hangar` — desktop app (run directly, or build a `.app` with `cargo tauri build`)

## CLI

```bash
hgr c archive.hgr file1 dir/ [-l 9] [--solid] [--long] [-e]
hgr x archive.hgr [-o outdir/]
hgr l archive.hgr
hgr t archive.hgr
```

`-l` picks the compression level (1–22, default 3). `--solid` packs files into shared frames for cross-file dedup; pair with `--long` to widen the match window to 128 MB. `-e` encrypts the archive with a password (XChaCha20-Poly1305 keyed via Argon2id).

For scripts, `--password-from FILE` reads the password from a file's first line.

## Layout

- `crates/core/` — `hangar-core` library: format, codec, encryption, read/write
- `crates/cli/` — `hgr` binary
- `crates/gui/` — `Hangar` desktop app
- `bench/` — head-to-head benchmark vs zip / 7zz / rar

## Benchmark

```bash
bash bench/run.sh
```

Builds a ~22 MB mixed corpus from local files and runs zip, 7zz, rar, and hgr at several settings, three times each, taking the best wall time. Prints a table.
