//! XChaCha20-Poly1305 AEAD with Argon2id key derivation.
//!
//! Encrypted frame on disk: `nonce(24) || ciphertext || tag(16)`. The 24-byte
//! nonce is safe to draw randomly per frame; `Entry::frame_compressed_size`
//! covers all three regions.

use crate::error::{Error, Result};
use chacha20poly1305::aead::generic_array::GenericArray;
use chacha20poly1305::aead::AeadInPlace;
use chacha20poly1305::{KeyInit, XChaCha20Poly1305};
use zeroize::Zeroizing;

pub const KEY_SIZE: usize = 32;
pub const NONCE_SIZE: usize = 24;
pub const TAG_SIZE: usize = 16;
pub const SALT_SIZE: usize = 16;

/// AEAD overhead added to every encrypted frame.
pub const FRAME_OVERHEAD: usize = NONCE_SIZE + TAG_SIZE;

/// Algorithm identifier persisted in the archive header.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CipherAlgo {
    XChaCha20Poly1305 = 1,
}

impl CipherAlgo {
    pub fn from_u8(b: u8) -> Result<Self> {
        match b {
            1 => Ok(Self::XChaCha20Poly1305),
            _ => Err(Error::Corrupt("unknown cipher algorithm")),
        }
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KdfAlgo {
    Argon2id = 1,
}

impl KdfAlgo {
    pub fn from_u8(b: u8) -> Result<Self> {
        match b {
            1 => Ok(Self::Argon2id),
            _ => Err(Error::Corrupt("unknown KDF algorithm")),
        }
    }
}

/// Argon2id parameters. Defaults take ~500 ms on a current laptop.
#[derive(Debug, Clone, Copy)]
pub struct Argon2Params {
    pub m_cost_kib: u32,
    pub t_cost: u32,
    pub p_cost: u32,
}

impl Default for Argon2Params {
    fn default() -> Self {
        Self {
            m_cost_kib: 64 * 1024, // 64 MiB
            t_cost: 3,
            p_cost: 4,
        }
    }
}

/// Holds the derived 32-byte key plus the cipher and KDF parameters needed
/// to re-derive it on read. The key is zeroized on drop.
pub struct Encryption {
    pub key: Zeroizing<[u8; KEY_SIZE]>,
    pub salt: [u8; SALT_SIZE],
    pub argon2: Argon2Params,
    cipher: XChaCha20Poly1305,
}

impl Encryption {
    /// Derive a fresh key from a password with a new random salt.
    pub fn from_new_password(password: &str, argon2: Argon2Params) -> Result<Self> {
        let mut salt = [0u8; SALT_SIZE];
        getrandom::getrandom(&mut salt).map_err(|e| Error::Crypto(e.to_string()))?;
        Self::from_password(password, salt, argon2)
    }

    /// Re-derive the key with a previously-stored salt and parameters.
    pub fn from_password(
        password: &str,
        salt: [u8; SALT_SIZE],
        argon2: Argon2Params,
    ) -> Result<Self> {
        let mut key = Zeroizing::new([0u8; KEY_SIZE]);
        let params = argon2::Params::new(
            argon2.m_cost_kib,
            argon2.t_cost,
            argon2.p_cost,
            Some(KEY_SIZE),
        )
        .map_err(|e| Error::Crypto(format!("argon2 params: {e}")))?;
        let kdf = argon2::Argon2::new(
            argon2::Algorithm::Argon2id,
            argon2::Version::V0x13,
            params,
        );
        kdf.hash_password_into(password.as_bytes(), &salt, &mut *key)
            .map_err(|e| Error::Crypto(format!("argon2 derive: {e}")))?;
        let cipher = XChaCha20Poly1305::new(GenericArray::from_slice(&key[..]));
        Ok(Self {
            key,
            salt,
            argon2,
            cipher,
        })
    }

    /// Encrypt `buf` in place under a fresh random nonce. Returns the nonce
    /// and the Poly1305 tag.
    pub fn encrypt(&self, buf: &mut [u8]) -> Result<([u8; NONCE_SIZE], [u8; TAG_SIZE])> {
        let mut nonce = [0u8; NONCE_SIZE];
        getrandom::getrandom(&mut nonce).map_err(|e| Error::Crypto(e.to_string()))?;
        let nonce_arr = GenericArray::from_slice(&nonce);
        let tag = self
            .cipher
            .encrypt_in_place_detached(nonce_arr, b"", buf)
            .map_err(|_| Error::Crypto("AEAD encrypt failed".into()))?;
        let mut tag_bytes = [0u8; TAG_SIZE];
        tag_bytes.copy_from_slice(&tag);
        Ok((nonce, tag_bytes))
    }

    /// Decrypt `buf` in place. A tag mismatch surfaces as `WrongPasswordOrTampered`.
    pub fn decrypt(
        &self,
        nonce: &[u8; NONCE_SIZE],
        tag: &[u8; TAG_SIZE],
        buf: &mut [u8],
    ) -> Result<()> {
        let nonce_arr = GenericArray::from_slice(nonce);
        let tag_arr = GenericArray::from_slice(tag);
        self.cipher
            .decrypt_in_place_detached(nonce_arr, b"", buf, tag_arr)
            .map_err(|_| Error::WrongPasswordOrTampered)
    }
}
