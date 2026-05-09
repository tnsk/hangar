# hangar

A modern archive tool. Compresses tighter than ZIP, beats 7z on max ratio, and decompresses faster than all of them. Pick a preset, drop your files, get a `.hgr` archive.

## What's in the box

- **Desktop app** — drag and drop to compress, drop a `.hgr` back in to extract. Three presets (Fast / Balanced / Max) cover almost everyone.
- **CLI** (`hgr`) — same engine, scriptable.
- **7 languages**: Turkish, English, German, French, Russian, Spanish, Arabic (with RTL layout).
- **8 themes**: Hangar (default warm light), Carbon (quiet dark), Matrix (phosphor terminal), Tron (cyan grid), Synthwave (pink/purple sunset), Dracula, Catppuccin, Nord.
- **Optional password encryption** — XChaCha20-Poly1305 AEAD with keys derived via Argon2id. The whole archive (frames + index) is wrapped, so even file names and sizes stay private.

## Benchmark

22 MB mixed corpus, best of three runs:

| Tool  | Settings           |     Output | % of input | Compress | Extract |
| ----- | ------------------ | ---------: | ---------: | -------: | ------: |
| zip   | default            |    7.97 MB |      35.2% |    0.45s |   0.10s |
| zip   | -9                 |    7.95 MB |      35.1% |    0.66s |   0.10s |
| rar   | -m3                |    6.36 MB |      28.1% |    0.17s |   0.04s |
| rar   | -m5                |    6.30 MB |      27.8% |    0.25s |   0.04s |
| 7zz   | -mx=5              |    2.93 MB |      12.9% |    1.85s |   0.07s |
| 7zz   | -mx=9              |    2.90 MB |      12.8% |    2.09s |   0.07s |
| hgr   | -3 (default)       |    7.61 MB |      33.6% |    0.05s |   0.02s |
| hgr   | -9 --solid --long  |    3.26 MB |      14.4% |    0.09s |   0.02s |
| hgr   | -19 --solid --long |    2.85 MB |      12.6% |    2.45s |   0.02s |

`bash bench/run.sh` reproduces this on your machine.

## Build

```bash
cargo build --release
```

Or grab a prebuilt binary for your platform from [Releases](https://github.com/tnsk/hangar/releases).

## CLI

```bash
hgr c archive.hgr file1 dir/        # create
hgr x archive.hgr -o out/           # extract
hgr l archive.hgr                   # list contents
hgr t archive.hgr                   # verify integrity
```

Flags worth knowing:

- `-l N` — compression level, 1–22 (default 3)
- `--solid` — pack files into shared frames; pair with `--long` for a 128 MB match window. Best ratio, biggest win on archives with similar files
- `-T N` — worker threads (default: all cores)
- `-e` — encrypt with a password (prompted; use `--password-from FILE` for scripts)

Run `hgr <command> --help` for the full list.
