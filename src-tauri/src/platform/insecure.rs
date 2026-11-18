//! Development and test key protector. **NOT SECURE.**
//!
//! It stores the data key as it is, behind a fixed marker. Anyone who can read
//! `key.bin` can decrypt the whole history. It exists only so that the
//! persistence layer builds and can be tested on Linux and macOS, where no
//! keystore integration exists yet.
//!
//! It is never used on Windows: `default_key_protector` returns the DPAPI
//! implementation there. On Windows this module is compiled for unit tests only.

use anyhow::{Result, bail};
use zeroize::Zeroizing;

use super::KeyProtector;

/// Marker so that a real protected blob (for example a DPAPI blob copied over
/// from another machine) is never mistaken for a stub blob, and the other way
/// round.
const MAGIC: &[u8] = b"CLIPDUMP-INSECURE-KEY-V1\0";

/// Identity "protection". See the module docs: not secure.
pub struct InsecureKeyProtector;

impl KeyProtector for InsecureKeyProtector {
    fn name(&self) -> &'static str {
        "insecure-stub"
    }

    fn protect(&self, plain: &[u8]) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(MAGIC.len() + plain.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(plain);
        Ok(out)
    }

    fn unprotect(&self, blob: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        let Some(rest) = blob.strip_prefix(MAGIC) else {
            bail!("key blob was not written by the insecure stub protector");
        };
        Ok(Zeroizing::new(rest.to_vec()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let p = InsecureKeyProtector;
        let blob = p.protect(b"secret").unwrap();
        assert_eq!(&**p.unprotect(&blob).unwrap(), b"secret");
    }

    #[test]
    fn rejects_foreign_blob() {
        assert!(InsecureKeyProtector.unprotect(b"not ours").is_err());
    }
}
