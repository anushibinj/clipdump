//! XChaCha20-Poly1305 encryption of everything written to disk.
//!
//! Envelope: `nonce (24 bytes, random) || ciphertext || tag (16 bytes)`.
//!
//! The 192 bit nonce is large enough that picking it at random for every
//! message is safe for any realistic volume of clipboard items, so there is no
//! counter to persist or to get wrong after a crash.
//!
//! Speed note: encryption is done in place on a single allocation and runs on
//! the persist writer thread, never on the hotkey or copy path.

use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, bail};
use chacha20poly1305::aead::{AeadInOut, Generate, Key, KeyInit, Nonce, Tag};
use chacha20poly1305::XChaCha20Poly1305;
use zeroize::{Zeroize, Zeroizing};

use crate::platform::KeyProtector;

pub const KEY_LEN: usize = 32;
pub const NONCE_LEN: usize = 24;
pub const TAG_LEN: usize = 16;
/// Bytes added to every plaintext. The smallest valid envelope is exactly this
/// long (an empty plaintext).
pub const OVERHEAD: usize = NONCE_LEN + TAG_LEN;

/// File name of the protected data key inside the data directory.
pub const KEY_FILE: &str = "key.bin";

/// Holds the cipher (and with it the key). The key schedule is wiped when this
/// is dropped (`chacha20poly1305` is built with its `zeroize` feature).
pub struct Crypto {
    cipher: XChaCha20Poly1305,
}

impl Crypto {
    /// Build from a raw 32 byte key. The caller keeps ownership of `key` and
    /// should wipe it.
    pub fn from_key(key: &[u8; KEY_LEN]) -> Self {
        Self {
            cipher: XChaCha20Poly1305::new(&Key::<XChaCha20Poly1305>::from(*key)),
        }
    }

    /// Load `key.bin` from `dir`, or on first run generate a random key,
    /// protect it with `protector` and save it.
    ///
    /// A `key.bin` that exists but cannot be unprotected is an error, never a
    /// reason to silently generate a new key: that would make the existing
    /// history permanently unreadable. The caller decides what to do (Phase 13
    /// moves a history it cannot read aside and starts fresh).
    pub fn load_or_create(dir: &Path, protector: &dyn KeyProtector) -> Result<Self> {
        let path = dir.join(KEY_FILE);
        match fs::read(&path) {
            Ok(blob) => {
                let key = protector
                    .unprotect(&blob)
                    .with_context(|| format!("cannot unprotect {}", path.display()))?;
                let key: Zeroizing<[u8; KEY_LEN]> = Zeroizing::new(
                    key.as_slice()
                        .try_into()
                        .map_err(|_| anyhow::anyhow!("{} holds a key of the wrong size", KEY_FILE))?,
                );
                Ok(Self::from_key(&key))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let key = random_key();
                let blob = protector.protect(&key[..]).context("cannot protect new data key")?;
                write_atomic_synced(&path, &blob)?;
                Ok(Self::from_key(&key))
            }
            Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
        }
    }

    /// Encrypt `plain` into `nonce || ciphertext || tag`.
    ///
    /// Returns a `Result` instead of the bare `Vec<u8>` of the plan: the AEAD
    /// API is fallible (only for absurd lengths) and `panic = "abort"` in the
    /// release profile makes unwrapping it a bad idea.
    pub fn encrypt(&self, plain: &[u8]) -> Result<Vec<u8>> {
        let nonce = Nonce::<XChaCha20Poly1305>::generate();
        let mut out = Vec::with_capacity(OVERHEAD + plain.len());
        out.extend_from_slice(&nonce);
        out.extend_from_slice(plain);
        let tag = self
            .cipher
            .encrypt_inout_detached(&nonce, b"", (&mut out[NONCE_LEN..]).into())
            .map_err(|_| anyhow::anyhow!("encryption failed"))?;
        out.extend_from_slice(&tag);
        Ok(out)
    }

    /// Verify and decrypt an envelope made by [`Crypto::encrypt`].
    ///
    /// Fails (without saying why, on purpose) for short input, a modified
    /// byte anywhere, or a different key.
    pub fn decrypt(&self, data: &[u8]) -> Result<Vec<u8>> {
        if data.len() < OVERHEAD {
            bail!("ciphertext too short ({} bytes)", data.len());
        }
        let (nonce, rest) = data.split_at(NONCE_LEN);
        let (body, tag) = rest.split_at(rest.len() - TAG_LEN);
        let nonce = Nonce::<XChaCha20Poly1305>::try_from(nonce).expect("split at NONCE_LEN");
        let tag = Tag::<XChaCha20Poly1305>::try_from(tag).expect("split at TAG_LEN");
        let mut plain = body.to_vec();
        self.cipher
            .decrypt_inout_detached(&nonce, b"", plain.as_mut_slice().into(), &tag)
            .map_err(|_| anyhow::anyhow!("authentication failed (modified data or wrong key)"))?;
        Ok(plain)
    }
}

fn random_key() -> Zeroizing<[u8; KEY_LEN]> {
    let mut key = Key::<XChaCha20Poly1305>::generate();
    let mut out = Zeroizing::new([0u8; KEY_LEN]);
    out.copy_from_slice(&key);
    key.as_mut_slice().zeroize();
    out
}

/// Write `data` to `path` via a temp file and an atomic rename, and fsync it.
/// Used for `key.bin` only: it is written once, so durability beats speed, and
/// losing it would make the whole history unreadable.
fn write_atomic_synced(path: &Path, data: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut f = fs::File::create(&tmp)
            .with_context(|| format!("cannot create {}", tmp.display()))?;
        f.write_all(data)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path).with_context(|| format!("cannot rename onto {}", path.display()))?;
    Ok(())
}
