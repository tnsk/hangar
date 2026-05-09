use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("not a hangar archive (bad magic)")]
    BadMagic,

    #[error("unsupported archive version {major}.{minor} (this build supports 1.x)")]
    UnsupportedVersion { major: u16, minor: u16 },

    #[error("archive is truncated or corrupted: {0}")]
    Corrupt(&'static str),

    #[error("index checksum mismatch")]
    IndexChecksum,

    #[error("entry content checksum mismatch for {path}")]
    ContentChecksum { path: String },

    #[error("invalid path: {0}")]
    InvalidPath(String),

    #[error("invalid utf-8 in archive metadata")]
    Utf8(#[from] std::string::FromUtf8Error),

    #[error("archive is encrypted; a password is required")]
    PasswordRequired,

    #[error("wrong password, or the archive has been tampered with")]
    WrongPasswordOrTampered,

    #[error("crypto error: {0}")]
    Crypto(String),
}

pub type Result<T> = std::result::Result<T, Error>;
