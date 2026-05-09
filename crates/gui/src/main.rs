// Suppress the extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use hangar_core::{
    Argon2Params, Encryption, EntryKind, Reader, Writer, WriterMode,
};
use serde::Serialize;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Instant, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, State};
use walkdir::WalkDir;

/// Shared cooperative-cancellation flag. A cancel is requested by flipping
/// this to `true`; long-running commands (compress/extract) check it at
/// safe points and abort with `errCancelled`.
#[derive(Default)]
struct Cancel(Arc<AtomicBool>);

impl Cancel {
    fn reset(&self) {
        self.0.store(false, Ordering::SeqCst);
    }
    fn request(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    fn is_set(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
    fn flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.0)
    }
}

#[tauri::command]
fn cancel_op(cancel: State<'_, Cancel>) {
    cancel.request();
}

/// Progress event emitted to the frontend during compress/extract.
#[derive(Serialize, Clone)]
struct Progress {
    phase: &'static str, // "compress" | "extract"
    current_bytes: u64,
    total_bytes: u64,
    current_file: Option<String>,
    files_done: u64,
    files_total: u64,
}

/// Read adapter that fires a callback every `threshold` bytes and aborts
/// with `Interrupted` if the cancel flag is set. Lets a long single-file
/// read be cancelled at byte granularity, not just per-file.
struct ProgressReader<R: Read> {
    inner: R,
    bytes_so_far: u64,
    last_emit: u64,
    threshold: u64,
    on_emit: Box<dyn FnMut(u64) + Send>,
    cancel: Arc<AtomicBool>,
}

impl<R: Read> Read for ProgressReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let n = self.inner.read(buf)?;
        self.bytes_so_far += n as u64;
        if self.bytes_so_far - self.last_emit >= self.threshold {
            self.last_emit = self.bytes_so_far;
            (self.on_emit)(self.bytes_so_far);
        }
        Ok(n)
    }
}

const PROGRESS_CHUNK: u64 = 1024 * 1024;

/// Structured error sent to the frontend. `code` is looked up against
/// the i18n table; `message` is the English fallback.
#[derive(Debug, Serialize)]
struct ErrorPayload {
    code: String,
    message: String,
}

impl ErrorPayload {
    fn generic<S: Into<String>>(msg: S) -> Self {
        Self {
            code: "errGeneric".into(),
            message: msg.into(),
        }
    }
    fn coded<S: Into<String>>(code: &str, msg: S) -> Self {
        Self {
            code: code.into(),
            message: msg.into(),
        }
    }
}

fn err_code(e: &hangar_core::Error) -> &'static str {
    use hangar_core::Error::*;
    match e {
        Io(_) => "errIo",
        BadMagic => "errBadMagic",
        UnsupportedVersion { .. } => "errUnsupportedVersion",
        Corrupt(_) => "errCorrupt",
        IndexChecksum => "errIndexChecksum",
        ContentChecksum { .. } => "errContentChecksum",
        InvalidPath(_) => "errInvalidPath",
        Utf8(_) => "errUtf8",
        PasswordRequired => "errPasswordRequired",
        WrongPasswordOrTampered => "errWrongPassword",
        Crypto(_) => "errCrypto",
    }
}

impl From<hangar_core::Error> for ErrorPayload {
    fn from(e: hangar_core::Error) -> Self {
        Self {
            code: err_code(&e).to_string(),
            message: e.to_string(),
        }
    }
}

impl From<std::io::Error> for ErrorPayload {
    fn from(e: std::io::Error) -> Self {
        Self::coded("errIo", e.to_string())
    }
}

impl From<walkdir::Error> for ErrorPayload {
    fn from(e: walkdir::Error) -> Self {
        Self::coded("errIo", e.to_string())
    }
}

#[derive(Serialize)]
struct CompressResult {
    files: u64,
    bytes_in: u64,
    bytes_out: u64,
    elapsed_secs: f64,
    ratio_pct: f64,
    archive: String,
}

#[derive(Serialize)]
struct ExtractResult {
    entries: u64,
    elapsed_secs: f64,
    output_dir: String,
}

#[derive(Serialize)]
struct ListEntry {
    path: String,
    kind: String,
    size: u64,
    frame_size: u64,
    frame_offset: u64,
}

#[derive(Serialize)]
struct ListResult {
    entries: Vec<ListEntry>,
    total_uncompressed: u64,
    archive_size: u64,
}

#[derive(Serialize)]
struct ProbeResult {
    encrypted: bool,
    archive_size: u64,
}

fn mtime_parts(meta: &fs::Metadata) -> (i64, u32) {
    match meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
    {
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

fn archive_path(root: &Path, entry: &Path) -> Result<String, ErrorPayload> {
    let rel = if root == entry {
        Path::new(
            root.file_name()
                .ok_or_else(|| ErrorPayload::generic(format!(
                    "input has no file name: {}",
                    root.display()
                )))?,
        )
        .to_path_buf()
    } else {
        let parent = root.parent().unwrap_or(Path::new(""));
        entry
            .strip_prefix(parent)
            .map_err(|e| ErrorPayload::generic(e.to_string()))?
            .to_path_buf()
    };
    let mut s = String::new();
    for c in rel.components() {
        let part = c
            .as_os_str()
            .to_str()
            .ok_or_else(|| ErrorPayload::generic(format!(
                "non-utf8 path component in {}",
                entry.display()
            )))?;
        if !s.is_empty() {
            s.push('/');
        }
        s.push_str(part);
    }
    Ok(s)
}

#[tauri::command]
async fn probe(archive: String) -> Result<ProbeResult, ErrorPayload> {
    let path = PathBuf::from(&archive);
    let archive_size = fs::metadata(&path)?.len();
    let f = File::open(&path)?;
    let (header, _enc) = Reader::probe(BufReader::new(f))?;
    Ok(ProbeResult {
        encrypted: header.flags & hangar_core::format::FLAG_ENCRYPTED != 0,
        archive_size,
    })
}

#[tauri::command]
async fn compress(
    app: AppHandle,
    cancel: State<'_, Cancel>,
    inputs: Vec<String>,
    output: String,
    level: i32,
    threads: u32,
    solid: bool,
    long: bool,
    block_size: u64,
    // Optional encryption password. Empty/absent = not encrypted.
    password: Option<String>,
) -> Result<CompressResult, ErrorPayload> {
    cancel.reset();
    let cancel_flag = cancel.flag();
    let archive = PathBuf::from(&output);
    if archive.exists() {
        return Err(ErrorPayload::coded(
            "errAlreadyExists",
            format!("{} already exists", archive.display()),
        ));
    }
    let start = Instant::now();

    let result = compress_inner(
        &app,
        &cancel_flag,
        &inputs,
        &archive,
        level,
        threads,
        solid,
        long,
        block_size,
        password,
        start,
    );
    // Half-written archive is unrecoverable; wipe it on any error.
    if result.is_err() && archive.exists() {
        let _ = fs::remove_file(&archive);
    }
    // Translate io errors that happened *because* the user cancelled into a
    // dedicated errCancelled code so the UI can localise it cleanly.
    if cancel.is_set() && result.is_err() {
        return Err(ErrorPayload::coded("errCancelled", "operation cancelled"));
    }
    result
}

fn compress_inner(
    app: &AppHandle,
    cancel: &Arc<AtomicBool>,
    inputs: &[String],
    archive: &Path,
    level: i32,
    threads: u32,
    solid: bool,
    long: bool,
    block_size: u64,
    password: Option<String>,
    start: Instant,
) -> Result<CompressResult, ErrorPayload> {

    // Pre-walk to total up bytes/files for a determinate progress bar.
    let mut total_bytes: u64 = 0;
    let mut total_files: u64 = 0;
    let mut canonical_inputs: Vec<PathBuf> = Vec::with_capacity(inputs.len());
    for input_str in inputs {
        let input = PathBuf::from(input_str).canonicalize().map_err(|e| {
            ErrorPayload::generic(format!("resolve {}: {}", input_str, e))
        })?;
        for ent in WalkDir::new(&input).follow_links(false) {
            let ent = ent?;
            if ent.file_type().is_file() {
                total_bytes += ent.metadata()?.len();
                total_files += 1;
            }
        }
        canonical_inputs.push(input);
    }

    let _ = app.emit(
        "progress",
        Progress {
            phase: "compress",
            current_bytes: 0,
            total_bytes,
            current_file: None,
            files_done: 0,
            files_total: total_files,
        },
    );

    let f = File::create(&archive)?;
    let mode = if solid {
        WriterMode::Solid {
            target_bytes: block_size,
        }
    } else {
        WriterMode::PerFile
    };
    let encryption = match password.as_deref() {
        Some(p) if !p.is_empty() => {
            Some(Encryption::from_new_password(p, Argon2Params::default())?)
        }
        _ => None,
    };
    let mut writer = Writer::new(
        BufWriter::new(f),
        Some(level),
        threads,
        long,
        mode,
        encryption,
    )?;

    let mut files_done: u64 = 0;
    let mut bytes_done: u64 = 0; // bytes from completed files

    for input in &canonical_inputs {
        for ent in WalkDir::new(input).follow_links(false) {
            // Per-file cancel check — keeps the cancel responsive on archives
            // of many small files where ProgressReader rarely runs.
            if cancel.load(Ordering::Relaxed) {
                return Err(ErrorPayload::coded("errCancelled", "operation cancelled"));
            }
            let ent = ent?;
            let path = ent.path();
            let meta = ent.metadata()?;
            let arc_path = archive_path(input, path)?;
            let (mtime_sec, mtime_nsec) = mtime_parts(&meta);
            let mode_bits = unix_mode(&meta);

            if meta.file_type().is_dir() {
                writer.add_dir(&arc_path, mode_bits, mtime_sec, mtime_nsec)?;
            } else if meta.file_type().is_symlink() {
                let target = fs::read_link(path)?;
                let t = target.to_str().ok_or_else(|| {
                    ErrorPayload::generic(format!(
                        "non-utf8 symlink target at {}",
                        path.display()
                    ))
                })?;
                writer.add_symlink(&arc_path, t, mtime_sec, mtime_nsec)?;
            } else if meta.file_type().is_file() {
                let app_c = app.clone();
                let bytes_offset = bytes_done;
                let path_for_progress = arc_path.clone();
                let files_done_snapshot = files_done;
                let on_emit: Box<dyn FnMut(u64) + Send> = Box::new(move |within_file| {
                    let _ = app_c.emit(
                        "progress",
                        Progress {
                            phase: "compress",
                            current_bytes: bytes_offset + within_file,
                            total_bytes,
                            current_file: Some(path_for_progress.clone()),
                            files_done: files_done_snapshot,
                            files_total: total_files,
                        },
                    );
                });
                let inner = BufReader::new(File::open(path)?);
                let progress_reader = ProgressReader {
                    inner,
                    bytes_so_far: 0,
                    last_emit: 0,
                    threshold: PROGRESS_CHUNK,
                    on_emit,
                    cancel: Arc::clone(cancel),
                };
                writer.add_file(
                    &arc_path,
                    mode_bits,
                    mtime_sec,
                    mtime_nsec,
                    progress_reader,
                )?;
                bytes_done += meta.len();
                files_done += 1;
                let _ = app.emit(
                    "progress",
                    Progress {
                        phase: "compress",
                        current_bytes: bytes_done,
                        total_bytes,
                        current_file: Some(arc_path.clone()),
                        files_done,
                        files_total: total_files,
                    },
                );
            }
        }
    }

    let _inner = writer.finish()?;
    let elapsed_secs = start.elapsed().as_secs_f64();
    let bytes_out = fs::metadata(&archive)?.len();
    let ratio_pct = if bytes_done == 0 {
        0.0
    } else {
        100.0 * bytes_out as f64 / bytes_done as f64
    };
    Ok(CompressResult {
        files: files_done,
        bytes_in: bytes_done,
        bytes_out,
        elapsed_secs,
        ratio_pct,
        archive: archive.to_string_lossy().to_string(),
    })
}

#[tauri::command]
async fn extract(
    app: AppHandle,
    cancel: State<'_, Cancel>,
    archive: String,
    output_dir: String,
    password: Option<String>,
) -> Result<ExtractResult, ErrorPayload> {
    cancel.reset();
    let cancel_flag = cancel.flag();
    let archive = PathBuf::from(&archive);
    let output = PathBuf::from(&output_dir);
    let start = Instant::now();
    fs::create_dir_all(&output)?;

    let f = File::open(&archive)?;
    let pw = password.unwrap_or_default();
    let mut reader = Reader::open(BufReader::new(f), || {
        if pw.is_empty() {
            Err(hangar_core::Error::PasswordRequired)
        } else {
            Ok(pw.clone())
        }
    })?;
    let entries = reader.entries.clone();
    let total_bytes: u64 = entries
        .iter()
        .filter(|e| e.kind == EntryKind::File)
        .map(|e| e.uncompressed_size)
        .sum();
    let total_files: u64 = entries
        .iter()
        .filter(|e| e.kind == EntryKind::File)
        .count() as u64;

    let _ = app.emit(
        "progress",
        Progress {
            phase: "extract",
            current_bytes: 0,
            total_bytes,
            current_file: None,
            files_done: 0,
            files_total: total_files,
        },
    );

    // Dirs + symlinks first (no decompression needed).
    for e in &entries {
        let dest = output.join(&e.path);
        match e.kind {
            EntryKind::Dir => {
                fs::create_dir_all(&dest)?;
            }
            EntryKind::Symlink => {
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent).ok();
                }
                #[cfg(unix)]
                {
                    let target = e.link_target.as_deref().unwrap_or("");
                    let _ = fs::remove_file(&dest);
                    std::os::unix::fs::symlink(target, &dest)?;
                }
            }
            EntryKind::File => {}
        }
    }

    // Files via for_each_file so each shared frame decodes once.
    let output_for_cb = output.clone();
    let app_for_cb = app.clone();
    let cancel_for_cb = cancel_flag.clone();
    let mut bytes_done: u64 = 0;
    let mut files_done: u64 = 0;
    let res = reader.for_each_file(|_, entry, bytes| {
        if cancel_for_cb.load(Ordering::Relaxed) {
            return Err(hangar_core::Error::Io(io::Error::new(
                io::ErrorKind::Interrupted,
                "cancelled",
            )));
        }
        let dest = output_for_cb.join(&entry.path);
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
        bytes_done += entry.uncompressed_size;
        files_done += 1;
        let _ = app_for_cb.emit(
            "progress",
            Progress {
                phase: "extract",
                current_bytes: bytes_done,
                total_bytes,
                current_file: Some(entry.path.clone()),
                files_done,
                files_total: total_files,
            },
        );
        Ok(())
    });

    if cancel.is_set() {
        return Err(ErrorPayload::coded("errCancelled", "operation cancelled"));
    }
    res?;

    Ok(ExtractResult {
        entries: entries.len() as u64,
        elapsed_secs: start.elapsed().as_secs_f64(),
        output_dir: output.to_string_lossy().to_string(),
    })
}

#[tauri::command]
fn list(archive: String, password: Option<String>) -> Result<ListResult, ErrorPayload> {
    let path = PathBuf::from(&archive);
    let archive_size = fs::metadata(&path)?.len();
    let f = File::open(&path)?;
    let pw = password.unwrap_or_default();
    let reader = Reader::open(BufReader::new(f), || {
        if pw.is_empty() {
            Err(hangar_core::Error::PasswordRequired)
        } else {
            Ok(pw.clone())
        }
    })?;
    let mut total_uncompressed = 0u64;
    let entries = reader
        .entries
        .iter()
        .map(|e| {
            total_uncompressed += e.uncompressed_size;
            ListEntry {
                path: e.path.clone(),
                kind: match e.kind {
                    EntryKind::File => "file",
                    EntryKind::Dir => "dir",
                    EntryKind::Symlink => "link",
                }
                .to_string(),
                size: e.uncompressed_size,
                frame_size: e.frame_compressed_size,
                frame_offset: e.frame_offset,
            }
        })
        .collect();
    Ok(ListResult {
        entries,
        total_uncompressed,
        archive_size,
    })
}

fn main() {
    tauri::Builder::default()
        .manage(Cancel::default())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            compress, extract, list, probe, cancel_op
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
