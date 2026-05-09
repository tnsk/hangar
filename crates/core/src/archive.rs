//! Streaming archive writer + random-access reader for the HGR1 format.

use crate::codec::{CountingWriter, HashingReader, DEFAULT_LEVEL};
use crate::crypto::{Encryption, FRAME_OVERHEAD, NONCE_SIZE, TAG_SIZE};
use crate::error::{Error, Result};
use crate::format::{
    self, validate_archive_path, Entry, EntryKind, EncryptionHeader, Footer, Header,
    FLAG_ENCRYPTED, FOOTER_SIZE, HEADER_SIZE,
};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::time::{SystemTime, UNIX_EPOCH};
use xxhash_rust::xxh3::{xxh3_64, Xxh3};

/// How files are packed into zstd frames.
#[derive(Debug, Clone, Copy)]
pub enum WriterMode {
    /// Each file becomes its own frame.
    PerFile,
    /// Files are concatenated into shared frames of up to `target_bytes` raw
    /// each. Better ratio on archives with similar files; extracting one
    /// file decodes its full block.
    Solid { target_bytes: u64 },
}

impl Default for WriterMode {
    fn default() -> Self {
        Self::PerFile
    }
}

fn now_unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

struct OpenBlock {
    frame_offset: u64,
    raw_bytes: u64,
    encoder: zstd::stream::Encoder<'static, Vec<u8>>,
    pending: Vec<usize>,
}

/// Writes a header on construction and zstd frames as files are added; the
/// index and footer are appended by `finish()`. The caller must call
/// `finish()` — without it the archive is unreadable.
pub struct Writer<W: Write + Seek> {
    inner: W,
    entries: Vec<Entry>,
    level: i32,
    workers: u32,
    long_range: bool,
    mode: WriterMode,
    open_block: Option<OpenBlock>,
    encryption: Option<Encryption>,
}

impl<W: Write + Seek> Writer<W> {
    pub fn new(
        mut inner: W,
        level: Option<i32>,
        workers: u32,
        long_range: bool,
        mode: WriterMode,
        encryption: Option<Encryption>,
    ) -> Result<Self> {
        let mut header = Header::new(now_unix_seconds());
        if encryption.is_some() {
            header.flags |= FLAG_ENCRYPTED;
        }
        header.write_to(&mut inner)?;
        if let Some(enc) = encryption.as_ref() {
            let eh = EncryptionHeader {
                cipher_algo: crate::crypto::CipherAlgo::XChaCha20Poly1305,
                kdf_algo: crate::crypto::KdfAlgo::Argon2id,
                argon2: enc.argon2,
                salt: enc.salt,
            };
            eh.write_to(&mut inner)?;
        }
        Ok(Self {
            inner,
            entries: Vec::new(),
            level: level.unwrap_or(DEFAULT_LEVEL),
            workers,
            long_range,
            mode,
            open_block: None,
            encryption,
        })
    }

    fn write_frame(&mut self, buf: &mut Vec<u8>) -> Result<u64> {
        if let Some(enc) = self.encryption.as_ref() {
            let (nonce, tag) = enc.encrypt(buf)?;
            self.inner.write_all(&nonce)?;
            self.inner.write_all(buf)?;
            self.inner.write_all(&tag)?;
            Ok((NONCE_SIZE + buf.len() + TAG_SIZE) as u64)
        } else {
            self.inner.write_all(buf)?;
            Ok(buf.len() as u64)
        }
    }

    fn configure_encoder<EW: Write>(
        encoder: &mut zstd::stream::Encoder<'static, EW>,
        workers: u32,
        long_range: bool,
    ) -> Result<()> {
        if long_range {
            use zstd::stream::raw::CParameter;
            encoder.set_parameter(CParameter::WindowLog(27))?;
            encoder.set_parameter(CParameter::EnableLongDistanceMatching(true))?;
        }
        if workers > 0 {
            encoder.multithread(workers)?;
        }
        Ok(())
    }

    fn ensure_open_block(&mut self) -> Result<&mut OpenBlock> {
        if self.open_block.is_none() {
            let frame_offset = self.inner.stream_position()?;
            let buf: Vec<u8> = Vec::new();
            let mut encoder = zstd::stream::Encoder::new(buf, self.level)?;
            Self::configure_encoder(&mut encoder, self.workers, self.long_range)?;
            self.open_block = Some(OpenBlock {
                frame_offset,
                raw_bytes: 0,
                encoder,
                pending: Vec::new(),
            });
        }
        Ok(self.open_block.as_mut().unwrap())
    }

    fn close_open_block(&mut self) -> Result<()> {
        let Some(block) = self.open_block.take() else {
            return Ok(());
        };
        let mut compressed_buf = block.encoder.finish()?;
        let frame_compressed_size = self.write_frame(&mut compressed_buf)?;
        for idx in block.pending {
            self.entries[idx].frame_compressed_size = frame_compressed_size;
        }
        Ok(())
    }

    pub fn add_file<R: Read>(
        &mut self,
        path: &str,
        mode: u32,
        mtime_sec: i64,
        mtime_nsec: u32,
        reader: R,
    ) -> Result<()> {
        validate_archive_path(path)?;
        match self.mode {
            WriterMode::PerFile => self.add_file_per_file(path, mode, mtime_sec, mtime_nsec, reader),
            WriterMode::Solid { target_bytes } => {
                self.add_file_solid(path, mode, mtime_sec, mtime_nsec, reader, target_bytes)
            }
        }
    }

    fn add_file_per_file<R: Read>(
        &mut self,
        path: &str,
        mode: u32,
        mtime_sec: i64,
        mtime_nsec: u32,
        reader: R,
    ) -> Result<()> {
        let frame_offset = self.inner.stream_position()?;
        let level = self.level;
        let workers = self.workers;
        let long_range = self.long_range;
        let mut hashing = HashingReader::new(reader);

        let frame_compressed_size = if self.encryption.is_some() {
            let mut buf: Vec<u8> = Vec::new();
            {
                let mut encoder = zstd::stream::Encoder::new(&mut buf, level)?;
                Self::configure_encoder(&mut encoder, workers, long_range)?;
                std::io::copy(&mut hashing, &mut encoder)?;
                encoder.finish()?;
            }
            self.write_frame(&mut buf)?
        } else {
            let mut counting = CountingWriter::new(&mut self.inner);
            {
                let mut encoder = zstd::stream::Encoder::new(&mut counting, level)?;
                Self::configure_encoder(&mut encoder, workers, long_range)?;
                std::io::copy(&mut hashing, &mut encoder)?;
                encoder.finish()?;
            }
            counting.bytes_written
        };

        let (uncompressed_size, content_xxh3) = hashing.finish();
        self.entries.push(Entry {
            path: path.to_string(),
            kind: EntryKind::File,
            mode,
            mtime_sec,
            mtime_nsec,
            uncompressed_size,
            frame_compressed_size,
            frame_offset,
            block_offset: 0,
            content_xxh3,
            link_target: None,
        });
        Ok(())
    }

    fn add_file_solid<R: Read>(
        &mut self,
        path: &str,
        mode: u32,
        mtime_sec: i64,
        mtime_nsec: u32,
        reader: R,
        target_bytes: u64,
    ) -> Result<()> {
        let entry_idx = self.entries.len();

        let (frame_offset, block_offset, file_bytes, content_xxh3) = {
            let block = self.ensure_open_block()?;
            let frame_offset = block.frame_offset;
            let block_offset = block.raw_bytes;

            let mut hasher = Xxh3::new();
            let mut buf = [0u8; 64 * 1024];
            let mut reader = reader;
            let mut file_bytes: u64 = 0;
            loop {
                let n = reader.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
                block.encoder.write_all(&buf[..n])?;
                block.raw_bytes += n as u64;
                file_bytes += n as u64;
            }
            block.pending.push(entry_idx);
            (frame_offset, block_offset, file_bytes, hasher.digest())
        };

        self.entries.push(Entry {
            path: path.to_string(),
            kind: EntryKind::File,
            mode,
            mtime_sec,
            mtime_nsec,
            uncompressed_size: file_bytes,
            frame_compressed_size: 0,
            frame_offset,
            block_offset,
            content_xxh3,
            link_target: None,
        });

        let raw = self.open_block.as_ref().map(|b| b.raw_bytes).unwrap_or(0);
        if raw >= target_bytes {
            self.close_open_block()?;
        }
        Ok(())
    }

    pub fn add_dir(&mut self, path: &str, mode: u32, mtime_sec: i64, mtime_nsec: u32) -> Result<()> {
        validate_archive_path(path)?;
        self.entries.push(Entry {
            path: path.to_string(),
            kind: EntryKind::Dir,
            mode,
            mtime_sec,
            mtime_nsec,
            uncompressed_size: 0,
            frame_compressed_size: 0,
            frame_offset: 0,
            block_offset: 0,
            content_xxh3: 0,
            link_target: None,
        });
        Ok(())
    }

    pub fn add_symlink(
        &mut self,
        path: &str,
        target: &str,
        mtime_sec: i64,
        mtime_nsec: u32,
    ) -> Result<()> {
        validate_archive_path(path)?;
        self.entries.push(Entry {
            path: path.to_string(),
            kind: EntryKind::Symlink,
            mode: 0,
            mtime_sec,
            mtime_nsec,
            uncompressed_size: 0,
            frame_compressed_size: 0,
            frame_offset: 0,
            block_offset: 0,
            content_xxh3: 0,
            link_target: Some(target.to_string()),
        });
        Ok(())
    }

    /// Write the index and footer, returning the inner writer.
    pub fn finish(mut self) -> Result<W> {
        self.close_open_block()?;
        let mut index_buf: Vec<u8> = Vec::new();
        format::write_index(&mut index_buf, &self.entries)?;
        let index_xxh3 = xxh3_64(&index_buf);
        let index_offset = self.inner.stream_position()?;
        let index_size = self.write_frame(&mut index_buf)?;

        let footer = Footer {
            index_offset,
            index_size,
            index_xxh3,
            flags: 0,
        };
        footer.write_to(&mut self.inner)?;
        self.inner.flush()?;
        Ok(self.inner)
    }
}

pub struct Reader<R: Read + Seek> {
    inner: R,
    pub header: Header,
    pub footer: Footer,
    pub entries: Vec<Entry>,
    encryption: Option<Encryption>,
}

impl<R: Read + Seek> Reader<R> {
    /// Read just the header (and EncryptionHeader, if present). Lets callers
    /// decide whether they need a password before committing to a full open.
    pub fn probe(mut inner: R) -> Result<(Header, Option<EncryptionHeader>)> {
        inner.seek(SeekFrom::Start(0))?;
        let header = Header::read_from(&mut inner)?;
        let enc_header = if header.flags & FLAG_ENCRYPTED != 0 {
            Some(EncryptionHeader::read_from(&mut inner)?)
        } else {
            None
        };
        Ok((header, enc_header))
    }

    /// Open an archive. `password_provider` is called only if the archive
    /// is encrypted.
    pub fn open<F>(mut inner: R, password_provider: F) -> Result<Self>
    where
        F: FnOnce() -> Result<String>,
    {
        let total = inner.seek(SeekFrom::End(0))?;
        if total < HEADER_SIZE + FOOTER_SIZE {
            return Err(Error::Corrupt("file too small for HGR1"));
        }

        // Read & validate header.
        inner.seek(SeekFrom::Start(0))?;
        let header = Header::read_from(&mut inner)?;

        // Read encryption header if present.
        let encryption = if header.flags & FLAG_ENCRYPTED != 0 {
            let eh = EncryptionHeader::read_from(&mut inner)?;
            let password = password_provider()?;
            Some(Encryption::from_password(&password, eh.salt, eh.argon2)?)
        } else {
            None
        };

        // Read footer.
        inner.seek(SeekFrom::Start(total - FOOTER_SIZE))?;
        let footer = Footer::read_from(&mut inner)?;
        if footer.index_offset + footer.index_size + FOOTER_SIZE != total {
            return Err(Error::Corrupt("footer index pointer inconsistent"));
        }

        inner.seek(SeekFrom::Start(footer.index_offset))?;
        let mut on_disk_index = vec![0u8; footer.index_size as usize];
        inner.read_exact(&mut on_disk_index)?;
        let plaintext_index = if let Some(enc) = encryption.as_ref() {
            if on_disk_index.len() < FRAME_OVERHEAD {
                return Err(Error::Corrupt("encrypted index too small"));
            }
            let mut nonce = [0u8; NONCE_SIZE];
            nonce.copy_from_slice(&on_disk_index[..NONCE_SIZE]);
            let mut tag = [0u8; TAG_SIZE];
            tag.copy_from_slice(&on_disk_index[on_disk_index.len() - TAG_SIZE..]);
            let ct_end = on_disk_index.len() - TAG_SIZE;
            let mut ct = on_disk_index[NONCE_SIZE..ct_end].to_vec();
            enc.decrypt(&nonce, &tag, &mut ct)?;
            ct
        } else {
            on_disk_index
        };
        if xxh3_64(&plaintext_index) != footer.index_xxh3 {
            return Err(Error::IndexChecksum);
        }
        let entries = format::read_index(&mut &plaintext_index[..])?;

        Ok(Self {
            inner,
            header,
            footer,
            entries,
            encryption,
        })
    }

    fn read_decoded_frame(&mut self, frame_offset: u64, frame_size: u64) -> Result<Vec<u8>> {
        self.inner.seek(SeekFrom::Start(frame_offset))?;
        let mut buf = vec![0u8; frame_size as usize];
        self.inner.read_exact(&mut buf)?;
        if let Some(enc) = self.encryption.as_ref() {
            if buf.len() < FRAME_OVERHEAD {
                return Err(Error::Corrupt("encrypted frame too small"));
            }
            let mut nonce = [0u8; NONCE_SIZE];
            nonce.copy_from_slice(&buf[..NONCE_SIZE]);
            let mut tag = [0u8; TAG_SIZE];
            tag.copy_from_slice(&buf[buf.len() - TAG_SIZE..]);
            let ct_end = buf.len() - TAG_SIZE;
            let mut ct = buf[NONCE_SIZE..ct_end].to_vec();
            enc.decrypt(&nonce, &tag, &mut ct)?;
            Ok(ct)
        } else {
            Ok(buf)
        }
    }

    /// Extract one entry. For bulk extraction prefer `for_each_file`, which
    /// decodes each shared frame only once.
    pub fn extract_entry<W: Write>(&mut self, idx: usize, out: &mut W) -> Result<u64> {
        let entry = self
            .entries
            .get(idx)
            .ok_or(Error::Corrupt("entry index out of range"))?
            .clone();
        if entry.kind != EntryKind::File {
            return Ok(0);
        }
        let zstd_input = self.read_decoded_frame(entry.frame_offset, entry.frame_compressed_size)?;
        let mut decoder = zstd::stream::Decoder::new(std::io::Cursor::new(zstd_input))?;
        if entry.block_offset > 0 {
            std::io::copy(&mut (&mut decoder).take(entry.block_offset), &mut std::io::sink())?;
        }
        let bounded = (&mut decoder).take(entry.uncompressed_size);
        let mut hashing = HashingReader::new(bounded);
        let n = std::io::copy(&mut hashing, out)?;
        let (decoded, hash) = hashing.finish();
        if decoded != entry.uncompressed_size || hash != entry.content_xxh3 {
            return Err(Error::ContentChecksum {
                path: entry.path.clone(),
            });
        }
        Ok(n)
    }

    /// Iterate every file entry, decompressing each frame once even when
    /// many entries share it. The callback receives the file's plaintext
    /// bytes, already verified against `content_xxh3`.
    pub fn for_each_file<F>(&mut self, mut callback: F) -> Result<()>
    where
        F: FnMut(usize, &Entry, &[u8]) -> Result<()>,
    {
        let mut groups: HashMap<u64, Vec<usize>> = HashMap::new();
        let mut order: Vec<u64> = Vec::new();
        for (i, e) in self.entries.iter().enumerate() {
            if e.kind != EntryKind::File {
                continue;
            }
            groups
                .entry(e.frame_offset)
                .or_insert_with(|| {
                    order.push(e.frame_offset);
                    Vec::new()
                })
                .push(i);
        }
        for frame_off in order {
            let indices = groups.remove(&frame_off).unwrap();
            let frame_compressed_size = self.entries[indices[0]].frame_compressed_size;
            let zstd_input = self.read_decoded_frame(frame_off, frame_compressed_size)?;
            let mut decoder = zstd::stream::Decoder::new(std::io::Cursor::new(zstd_input))?;
            let mut buf: Vec<u8> = Vec::new();
            std::io::copy(&mut decoder, &mut buf)?;
            let mut indices = indices;
            indices.sort_by_key(|i| self.entries[*i].block_offset);
            for i in indices {
                let entry = self.entries[i].clone();
                let start = entry.block_offset as usize;
                let end = start + entry.uncompressed_size as usize;
                if end > buf.len() {
                    return Err(Error::Corrupt(
                        "entry range exceeds decoded block size",
                    ));
                }
                let slice = &buf[start..end];
                if xxh3_64(slice) != entry.content_xxh3 {
                    return Err(Error::ContentChecksum {
                        path: entry.path.clone(),
                    });
                }
                callback(i, &entry, slice)?;
            }
        }
        Ok(())
    }

    /// Walk every file and verify its content hash. Discards decoded bytes.
    pub fn verify_all(&mut self) -> Result<()> {
        self.for_each_file(|_, _, _| Ok(()))
    }
}
