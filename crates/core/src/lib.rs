//! Hangar archive core: `.hgr` format reader/writer.

pub mod error;
pub mod format;
pub mod codec;
pub mod crypto;
pub mod archive;

pub use error::{Error, Result};
pub use format::{Entry, EntryKind, Header, Footer, MAGIC, VERSION_MAJOR, VERSION_MINOR};
pub use archive::{Writer, Reader, WriterMode};
pub use crypto::{Argon2Params, Encryption};
