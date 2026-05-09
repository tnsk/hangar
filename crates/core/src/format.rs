use crate::crypto::{Argon2Params, CipherAlgo, KdfAlgo, SALT_SIZE};
use crate::error::{Error, Result};
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use std::io::{Read, Write};

pub const MAGIC: &[u8; 4] = b"HGR1";
pub const HEADER_SIZE: u64 = 32;
pub const FOOTER_SIZE: u64 = 32;
pub const ENCRYPTION_HEADER_SIZE: u64 = 40;
pub const VERSION_MAJOR: u16 = 1;
pub const VERSION_MINOR: u16 = 1; // bumped: encryption support
/// Header.flags bit 0: archive is encrypted.
pub const FLAG_ENCRYPTED: u32 = 1 << 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum EntryKind {
    File = 0,
    Dir = 1,
    Symlink = 2,
}

impl EntryKind {
    fn from_u8(b: u8) -> Result<Self> {
        match b {
            0 => Ok(Self::File),
            1 => Ok(Self::Dir),
            2 => Ok(Self::Symlink),
            _ => Err(Error::Corrupt("unknown entry kind")),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Header {
    pub version_major: u16,
    pub version_minor: u16,
    pub flags: u32,
    pub created_at: i64,
}

impl Header {
    pub fn new(created_at: i64) -> Self {
        Self {
            version_major: VERSION_MAJOR,
            version_minor: VERSION_MINOR,
            flags: 0,
            created_at,
        }
    }

    pub fn write_to<W: Write>(&self, w: &mut W) -> Result<()> {
        w.write_all(MAGIC)?;
        w.write_u16::<LittleEndian>(self.version_major)?;
        w.write_u16::<LittleEndian>(self.version_minor)?;
        w.write_u32::<LittleEndian>(self.flags)?;
        w.write_i64::<LittleEndian>(self.created_at)?;
        w.write_all(&[0u8; 12])?;
        Ok(())
    }

    pub fn read_from<R: Read>(r: &mut R) -> Result<Self> {
        let mut magic = [0u8; 4];
        r.read_exact(&mut magic)?;
        if &magic != MAGIC {
            return Err(Error::BadMagic);
        }
        let version_major = r.read_u16::<LittleEndian>()?;
        let version_minor = r.read_u16::<LittleEndian>()?;
        if version_major != VERSION_MAJOR {
            return Err(Error::UnsupportedVersion {
                major: version_major,
                minor: version_minor,
            });
        }
        let flags = r.read_u32::<LittleEndian>()?;
        let created_at = r.read_i64::<LittleEndian>()?;
        let mut reserved = [0u8; 12];
        r.read_exact(&mut reserved)?;
        Ok(Self {
            version_major,
            version_minor,
            flags,
            created_at,
        })
    }
}

/// 40 bytes written after the main header when the archive is encrypted.
/// Stores the cipher/KDF identifiers, Argon2 parameters, and salt.
#[derive(Debug, Clone)]
pub struct EncryptionHeader {
    pub cipher_algo: CipherAlgo,
    pub kdf_algo: KdfAlgo,
    pub argon2: Argon2Params,
    pub salt: [u8; SALT_SIZE],
}

impl EncryptionHeader {
    pub fn write_to<W: Write>(&self, w: &mut W) -> Result<()> {
        w.write_u8(self.cipher_algo as u8)?;
        w.write_u8(self.kdf_algo as u8)?;
        w.write_all(&[0u8; 2])?; // reserved
        w.write_u32::<LittleEndian>(self.argon2.m_cost_kib)?;
        w.write_u32::<LittleEndian>(self.argon2.t_cost)?;
        w.write_u32::<LittleEndian>(self.argon2.p_cost)?;
        w.write_all(&self.salt)?;
        w.write_all(&[0u8; 8])?; // reserved
        Ok(())
    }

    pub fn read_from<R: Read>(r: &mut R) -> Result<Self> {
        let cipher_algo = CipherAlgo::from_u8(r.read_u8()?)?;
        let kdf_algo = KdfAlgo::from_u8(r.read_u8()?)?;
        let mut reserved = [0u8; 2];
        r.read_exact(&mut reserved)?;
        let m_cost_kib = r.read_u32::<LittleEndian>()?;
        let t_cost = r.read_u32::<LittleEndian>()?;
        let p_cost = r.read_u32::<LittleEndian>()?;
        let mut salt = [0u8; SALT_SIZE];
        r.read_exact(&mut salt)?;
        let mut reserved2 = [0u8; 8];
        r.read_exact(&mut reserved2)?;
        Ok(Self {
            cipher_algo,
            kdf_algo,
            argon2: Argon2Params {
                m_cost_kib,
                t_cost,
                p_cost,
            },
            salt,
        })
    }
}

#[derive(Debug, Clone)]
pub struct Footer {
    pub index_offset: u64,
    pub index_size: u64,
    pub index_xxh3: u64,
    pub flags: u32,
}

impl Footer {
    pub fn write_to<W: Write>(&self, w: &mut W) -> Result<()> {
        w.write_u64::<LittleEndian>(self.index_offset)?;
        w.write_u64::<LittleEndian>(self.index_size)?;
        w.write_u64::<LittleEndian>(self.index_xxh3)?;
        w.write_u32::<LittleEndian>(self.flags)?;
        w.write_all(MAGIC)?;
        Ok(())
    }

    pub fn read_from<R: Read>(r: &mut R) -> Result<Self> {
        let index_offset = r.read_u64::<LittleEndian>()?;
        let index_size = r.read_u64::<LittleEndian>()?;
        let index_xxh3 = r.read_u64::<LittleEndian>()?;
        let flags = r.read_u32::<LittleEndian>()?;
        let mut magic = [0u8; 4];
        r.read_exact(&mut magic)?;
        if &magic != MAGIC {
            return Err(Error::BadMagic);
        }
        Ok(Self {
            index_offset,
            index_size,
            index_xxh3,
            flags,
        })
    }
}

/// One file in the archive index. In solid mode multiple entries share
/// `frame_offset` and `frame_compressed_size`; `block_offset` then locates
/// each file inside the decoded frame.
#[derive(Debug, Clone)]
pub struct Entry {
    pub path: String,
    pub kind: EntryKind,
    pub mode: u32,
    pub mtime_sec: i64,
    pub mtime_nsec: u32,
    pub uncompressed_size: u64,
    pub frame_compressed_size: u64,
    pub frame_offset: u64,
    pub block_offset: u64,
    pub content_xxh3: u64,
    pub link_target: Option<String>,
}

impl Entry {
    pub fn write_to<W: Write>(&self, w: &mut W) -> Result<()> {
        let path_bytes = self.path.as_bytes();
        if path_bytes.len() > u16::MAX as usize {
            return Err(Error::InvalidPath(format!(
                "path too long ({} bytes)",
                path_bytes.len()
            )));
        }
        w.write_u16::<LittleEndian>(path_bytes.len() as u16)?;
        w.write_all(path_bytes)?;
        w.write_u8(self.kind as u8)?;
        w.write_u32::<LittleEndian>(self.mode)?;
        w.write_i64::<LittleEndian>(self.mtime_sec)?;
        w.write_u32::<LittleEndian>(self.mtime_nsec)?;
        w.write_u64::<LittleEndian>(self.uncompressed_size)?;
        w.write_u64::<LittleEndian>(self.frame_compressed_size)?;
        w.write_u64::<LittleEndian>(self.frame_offset)?;
        w.write_u64::<LittleEndian>(self.block_offset)?;
        w.write_u64::<LittleEndian>(self.content_xxh3)?;
        if self.kind == EntryKind::Symlink {
            let t = self.link_target.as_deref().unwrap_or("");
            let tb = t.as_bytes();
            if tb.len() > u16::MAX as usize {
                return Err(Error::InvalidPath(format!(
                    "symlink target too long ({} bytes)",
                    tb.len()
                )));
            }
            w.write_u16::<LittleEndian>(tb.len() as u16)?;
            w.write_all(tb)?;
        }
        Ok(())
    }

    pub fn read_from<R: Read>(r: &mut R) -> Result<Self> {
        let path_len = r.read_u16::<LittleEndian>()? as usize;
        let mut path_buf = vec![0u8; path_len];
        r.read_exact(&mut path_buf)?;
        let path = String::from_utf8(path_buf)?;
        let kind = EntryKind::from_u8(r.read_u8()?)?;
        let mode = r.read_u32::<LittleEndian>()?;
        let mtime_sec = r.read_i64::<LittleEndian>()?;
        let mtime_nsec = r.read_u32::<LittleEndian>()?;
        let uncompressed_size = r.read_u64::<LittleEndian>()?;
        let frame_compressed_size = r.read_u64::<LittleEndian>()?;
        let frame_offset = r.read_u64::<LittleEndian>()?;
        let block_offset = r.read_u64::<LittleEndian>()?;
        let content_xxh3 = r.read_u64::<LittleEndian>()?;
        let link_target = if kind == EntryKind::Symlink {
            let t_len = r.read_u16::<LittleEndian>()? as usize;
            let mut t_buf = vec![0u8; t_len];
            r.read_exact(&mut t_buf)?;
            Some(String::from_utf8(t_buf)?)
        } else {
            None
        };
        Ok(Self {
            path,
            kind,
            mode,
            mtime_sec,
            mtime_nsec,
            uncompressed_size,
            frame_compressed_size,
            frame_offset,
            block_offset,
            content_xxh3,
            link_target,
        })
    }
}

pub fn write_index<W: Write>(w: &mut W, entries: &[Entry]) -> Result<()> {
    if entries.len() > u32::MAX as usize {
        return Err(Error::Corrupt("too many entries"));
    }
    w.write_u32::<LittleEndian>(entries.len() as u32)?;
    for e in entries {
        e.write_to(w)?;
    }
    Ok(())
}

pub fn read_index<R: Read>(r: &mut R) -> Result<Vec<Entry>> {
    let n = r.read_u32::<LittleEndian>()? as usize;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        out.push(Entry::read_from(r)?);
    }
    Ok(out)
}

/// Validate a path stored in the archive: relative, forward-slash, no traversal.
pub fn validate_archive_path(p: &str) -> Result<()> {
    if p.is_empty() {
        return Err(Error::InvalidPath("empty path".into()));
    }
    if p.starts_with('/') {
        return Err(Error::InvalidPath(format!("absolute path: {p}")));
    }
    if p.contains('\\') {
        return Err(Error::InvalidPath(format!("backslash in path: {p}")));
    }
    for seg in p.split('/') {
        if seg == ".." {
            return Err(Error::InvalidPath(format!("'..' segment in path: {p}")));
        }
    }
    Ok(())
}
