//! Small I/O adapters used by the archive layer.

use std::io::{self, Read, Write};
use xxhash_rust::xxh3::Xxh3;

pub const DEFAULT_LEVEL: i32 = 3;

/// Reader that hashes and counts every byte that flows through it.
pub struct HashingReader<R: Read> {
    inner: R,
    pub hasher: Xxh3,
    pub bytes_read: u64,
}

impl<R: Read> HashingReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            hasher: Xxh3::new(),
            bytes_read: 0,
        }
    }

    pub fn finish(self) -> (u64, u64) {
        (self.bytes_read, self.hasher.digest())
    }
}

impl<R: Read> Read for HashingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n > 0 {
            self.hasher.update(&buf[..n]);
            self.bytes_read += n as u64;
        }
        Ok(n)
    }
}

/// Writer that counts every byte that flows through it.
pub struct CountingWriter<W: Write> {
    inner: W,
    pub bytes_written: u64,
}

impl<W: Write> CountingWriter<W> {
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            bytes_written: 0,
        }
    }

    pub fn into_inner(self) -> W {
        self.inner
    }
}

impl<W: Write> Write for CountingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.bytes_written += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}
