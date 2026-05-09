use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use hangar_core::{Argon2Params, Encryption, EntryKind, Reader, Writer, WriterMode};
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use walkdir::WalkDir;

#[derive(Parser)]
#[command(name = "hgr", version, about = "hangar archive tool")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create a new archive from files and directories.
    #[command(visible_alias = "c")]
    Create {
        /// Output archive path (e.g. backup.hgr).
        archive: PathBuf,
        /// Files or directories to include.
        #[arg(required = true)]
        inputs: Vec<PathBuf>,
        /// Compression level (1..22; default 3). Negative = ultra-fast.
        #[arg(short, long, default_value_t = 3)]
        level: i32,
        /// Worker threads for zstd. 0 = single-threaded; default = all logical cores.
        /// Decompressed content is identical regardless of this setting; the
        /// compressed bytes may differ slightly between ST and MT.
        #[arg(short = 'T', long, default_value_t = default_threads())]
        threads: u32,
        /// Enable zstd long-range mode (window 128 MB + LDM). Big ratio gain
        /// on content with long-distance duplicates (build artifacts, similar
        /// files, repeated assets); costs more RAM. Decode works everywhere.
        #[arg(long)]
        long: bool,
        /// Solid mode: pack files into shared zstd frames so cross-file
        /// duplicates collapse. Trades random access for dramatically better
        /// ratio on archives with similar files (build outputs, log dumps,
        /// repeated assets). Pair with --long for best results.
        #[arg(long)]
        solid: bool,
        /// Target raw bytes per solid block (default 64 MB). Bigger blocks =
        /// better ratio but slower single-file extract. Ignored without --solid.
        #[arg(long, default_value_t = 64 * 1024 * 1024)]
        block_size: u64,
        /// Encrypt the archive with a password (XChaCha20-Poly1305 + Argon2id).
        #[arg(short = 'e', long)]
        encrypt: bool,
        /// Read password from this file's first line instead of prompting.
        /// Useful for scripts and CI; mind the file permissions.
        #[arg(long, value_name = "FILE")]
        password_from: Option<PathBuf>,
    },
    /// Extract an archive into a directory.
    #[command(visible_alias = "x")]
    Extract {
        archive: PathBuf,
        /// Output directory (created if missing). Defaults to current dir.
        #[arg(short, long, default_value = ".")]
        output: PathBuf,
        /// Read password from this file's first line instead of prompting.
        #[arg(long, value_name = "FILE")]
        password_from: Option<PathBuf>,
    },
    /// List the contents of an archive.
    #[command(visible_alias = "l")]
    List {
        archive: PathBuf,
        #[arg(long, value_name = "FILE")]
        password_from: Option<PathBuf>,
    },
    /// Test archive integrity (decompresses and verifies hashes; discards output).
    #[command(visible_alias = "t")]
    Test {
        archive: PathBuf,
        #[arg(long, value_name = "FILE")]
        password_from: Option<PathBuf>,
    },
}

fn default_threads() -> u32 {
    std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(1)
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Create {
            archive,
            inputs,
            level,
            threads,
            long,
            solid,
            block_size,
            encrypt,
            password_from,
        } => create(
            &archive, &inputs, level, threads, long, solid, block_size, encrypt,
            password_from.as_deref(),
        ),
        Cmd::Extract { archive, output, password_from } => {
            extract(&archive, &output, password_from.as_deref())
        }
        Cmd::List { archive, password_from } => list(&archive, password_from.as_deref()),
        Cmd::Test { archive, password_from } => test(&archive, password_from.as_deref()),
    }
}

/// Read the password from `--password-from` when given, else prompt the
/// user on the controlling terminal.
fn read_password_for_open(password_from: Option<&Path>) -> hangar_core::Result<String> {
    if let Some(path) = password_from {
        return read_password_file(path);
    }
    rpassword::prompt_password("Password: ")
        .map_err(|e| hangar_core::Error::Crypto(format!("read password: {e}")))
}

fn read_password_file(path: &Path) -> hangar_core::Result<String> {
    let raw = fs::read_to_string(path).map_err(hangar_core::Error::Io)?;
    let line = raw.lines().next().unwrap_or("").to_string();
    if line.is_empty() {
        return Err(hangar_core::Error::Crypto(
            "password file is empty".into(),
        ));
    }
    Ok(line)
}

/// Acquire the create-time password: from `--password-from` if given,
/// otherwise interactively (twice, with a confirm step).
fn acquire_create_password(password_from: Option<&Path>) -> Result<String> {
    if let Some(path) = password_from {
        let p = read_password_file(path).map_err(|e| anyhow!("{}", e))?;
        return Ok(p);
    }
    let p1 = rpassword::prompt_password("Password: ")
        .map_err(|e| anyhow!("read password: {e}"))?;
    if p1.is_empty() {
        return Err(anyhow!("password must not be empty"));
    }
    let p2 = rpassword::prompt_password("Confirm password: ")
        .map_err(|e| anyhow!("read password: {e}"))?;
    if p1 != p2 {
        return Err(anyhow!("passwords do not match"));
    }
    Ok(p1)
}

fn mtime_parts(meta: &fs::Metadata) -> (i64, u32) {
    let mt = meta.modified().ok();
    match mt.and_then(|t| t.duration_since(UNIX_EPOCH).ok()) {
        Some(d) => (d.as_secs() as i64, d.subsec_nanos()),
        None => (0, 0),
    }
}

#[cfg(unix)]
fn unix_mode(meta: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode()
}

#[cfg(not(unix))]
fn unix_mode(_meta: &fs::Metadata) -> u32 {
    0o644
}

/// Build the archive-relative path for a walked entry. Roots passed on the
/// CLI become the top of the archive: `hgr c a.hgr foo/bar` stores entries
/// as `bar/...`. Single-file inputs store just the file name.
fn archive_path(root: &Path, entry: &Path) -> Result<String> {
    let rel = if root == entry {
        Path::new(
            root.file_name()
                .ok_or_else(|| anyhow!("input has no file name: {}", root.display()))?,
        )
        .to_path_buf()
    } else {
        let parent = root.parent().unwrap_or(Path::new(""));
        entry
            .strip_prefix(parent)
            .with_context(|| format!("strip {} from {}", parent.display(), entry.display()))?
            .to_path_buf()
    };
    let mut s = String::with_capacity(rel.as_os_str().len());
    for comp in rel.components() {
        let part = comp
            .as_os_str()
            .to_str()
            .ok_or_else(|| anyhow!("non-utf8 path component in {}", entry.display()))?;
        if !s.is_empty() {
            s.push('/');
        }
        s.push_str(part);
    }
    Ok(s)
}

fn create(
    archive: &Path,
    inputs: &[PathBuf],
    level: i32,
    threads: u32,
    long: bool,
    solid: bool,
    block_size: u64,
    encrypt: bool,
    password_from: Option<&Path>,
) -> Result<()> {
    if archive.exists() {
        return Err(anyhow!(
            "{} already exists; remove it first",
            archive.display()
        ));
    }
    let encryption = if encrypt {
        let password = acquire_create_password(password_from)?;
        eprintln!("deriving key…");
        Some(Encryption::from_new_password(&password, Argon2Params::default())?)
    } else {
        if password_from.is_some() {
            eprintln!("warning: --password-from ignored without --encrypt");
        }
        None
    };
    let f = File::create(archive)
        .with_context(|| format!("create {}", archive.display()))?;
    let buf = BufWriter::new(f);
    let mode = if solid {
        WriterMode::Solid {
            target_bytes: block_size,
        }
    } else {
        WriterMode::PerFile
    };
    let mut writer = Writer::new(buf, Some(level), threads, long, mode, encryption)?;

    let mut files = 0u64;
    let mut bytes_in = 0u64;

    for input in inputs {
        let input = input
            .canonicalize()
            .with_context(|| format!("resolve {}", input.display()))?;
        for ent in WalkDir::new(&input).follow_links(false) {
            let ent = ent?;
            let path = ent.path();
            let meta = ent.metadata()?;
            let arc_path = archive_path(&input, path)?;
            let (mtime_sec, mtime_nsec) = mtime_parts(&meta);
            let mode = unix_mode(&meta);

            if meta.file_type().is_dir() {
                writer.add_dir(&arc_path, mode, mtime_sec, mtime_nsec)?;
            } else if meta.file_type().is_symlink() {
                let target = fs::read_link(path)?;
                let t = target
                    .to_str()
                    .ok_or_else(|| anyhow!("non-utf8 symlink target at {}", path.display()))?;
                writer.add_symlink(&arc_path, t, mtime_sec, mtime_nsec)?;
            } else if meta.file_type().is_file() {
                let f = File::open(path)
                    .with_context(|| format!("open {}", path.display()))?;
                let r = BufReader::new(f);
                writer.add_file(&arc_path, mode, mtime_sec, mtime_nsec, r)?;
                files += 1;
                bytes_in += meta.len();
            }
        }
    }

    let inner = writer.finish()?;
    drop(inner);

    let bytes_out = fs::metadata(archive)?.len();
    let ratio = if bytes_in == 0 {
        0.0
    } else {
        100.0 * (1.0 - bytes_out as f64 / bytes_in as f64)
    };
    eprintln!(
        "created {}: {} files, {} → {} bytes ({:.1}% saved, level {}, threads {}{}{})",
        archive.display(),
        files,
        bytes_in,
        bytes_out,
        ratio,
        level,
        threads,
        if long { ", long" } else { "" },
        if solid {
            format!(", solid {}MB", block_size / (1024 * 1024))
        } else {
            String::new()
        }
    );
    Ok(())
}

fn extract(archive: &Path, output: &Path, password_from: Option<&Path>) -> Result<()> {
    fs::create_dir_all(output)
        .with_context(|| format!("mkdir -p {}", output.display()))?;
    let f = File::open(archive)
        .with_context(|| format!("open {}", archive.display()))?;
    let mut reader = Reader::open(BufReader::new(f), || read_password_for_open(password_from))?;

    // First pass: dirs + symlinks. These don't need decompression and we
    // want directories materialized before any file in them lands.
    let entries = reader.entries.clone();
    for e in &entries {
        let dest = output.join(&e.path);
        match e.kind {
            EntryKind::Dir => {
                fs::create_dir_all(&dest)
                    .with_context(|| format!("mkdir {}", dest.display()))?;
            }
            EntryKind::Symlink => {
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent).ok();
                }
                let target = e.link_target.as_deref().unwrap_or("");
                #[cfg(unix)]
                {
                    let _ = fs::remove_file(&dest);
                    std::os::unix::fs::symlink(target, &dest)
                        .with_context(|| format!("symlink {} -> {}", dest.display(), target))?;
                }
                #[cfg(not(unix))]
                {
                    eprintln!(
                        "warning: skipping symlink {} -> {} (unsupported on this OS)",
                        dest.display(),
                        target
                    );
                }
            }
            EntryKind::File => {} // handled in second pass
        }
    }

    // Second pass: files. for_each_file decodes each (possibly solid) zstd
    // frame exactly once and hands us each contained file as a slice.
    let output = output.to_path_buf();
    reader.for_each_file(|_, entry, bytes| {
        let dest = output.join(&entry.path);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).ok();
        }
        let mut out = BufWriter::new(File::create(&dest)?);
        out.write_all(bytes)?;
        out.flush()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if entry.mode != 0 {
                let _ = fs::set_permissions(&dest, fs::Permissions::from_mode(entry.mode));
            }
        }
        Ok(())
    })?;

    eprintln!("extracted {} entries from {}", entries.len(), archive.display());
    Ok(())
}

fn list(archive: &Path, password_from: Option<&Path>) -> Result<()> {
    let f = File::open(archive)
        .with_context(|| format!("open {}", archive.display()))?;
    let reader = Reader::open(BufReader::new(f), || read_password_for_open(password_from))?;
    println!(
        "{:>12}  {:>12}  {:>5}  {:>10}  {}",
        "size", "frame_size", "kind", "frame@", "path"
    );
    for e in &reader.entries {
        let kind = match e.kind {
            EntryKind::File => "file",
            EntryKind::Dir => "dir",
            EntryKind::Symlink => "link",
        };
        println!(
            "{:>12}  {:>12}  {:>5}  {:>10}  {}",
            e.uncompressed_size, e.frame_compressed_size, kind, e.frame_offset, e.path
        );
    }
    Ok(())
}

fn test(archive: &Path, password_from: Option<&Path>) -> Result<()> {
    let f = File::open(archive)
        .with_context(|| format!("open {}", archive.display()))?;
    let mut reader = Reader::open(BufReader::new(f), || read_password_for_open(password_from))?;
    let n = reader.entries.len();
    reader.verify_all()?;
    eprintln!("ok: {} entries verified ({})", n, archive.display());
    Ok(())
}
