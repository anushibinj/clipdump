//! Windows DPAPI key protector (`CryptProtectData` / `CryptUnprotectData`).
//!
//! Scope is the current Windows user: the blob can only be unprotected by the
//! same user account on the same machine (or a roaming profile with the same
//! DPAPI master key). That is exactly the threat model for clipboard history:
//! other users, other machines and offline disk copies cannot read it.
//!
//! Speed note: this runs once at startup (load key) and once on the very first
//! run (create key). Never on the hotkey or copy path.

use ::windows::Win32::Foundation::{HLOCAL, LocalFree};
use ::windows::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
};
use ::windows::core::w;
use anyhow::{Context, Result, bail};
use zeroize::Zeroizing;

use crate::platform::KeyProtector;

/// Application specific entropy. It is not a secret (it is in the binary); it
/// only stops other programs of the same user from unprotecting the blob by
/// accident with a plain `CryptUnprotectData` call, and binds the blob format
/// to this version of the key file.
const ENTROPY: &[u8] = b"clipdump/key.bin/v1";

/// DPAPI backed [`KeyProtector`], current user scope.
pub struct DpapiProtector;

fn blob_of(data: &[u8]) -> Result<CRYPT_INTEGER_BLOB> {
    Ok(CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(data.len()).context("blob larger than 4 GiB")?,
        // DPAPI only reads through this pointer for input blobs.
        pbData: data.as_ptr() as *mut u8,
    })
}

/// Copy a DPAPI owned output buffer into a `Vec`, then free it. When
/// `wipe` is set the OS buffer is zeroed first (it held key material).
///
/// # Safety
/// `out` must have been filled in by a successful `CryptProtectData` or
/// `CryptUnprotectData` call and not freed yet.
unsafe fn take_output(out: CRYPT_INTEGER_BLOB, wipe: bool) -> Vec<u8> {
    if out.pbData.is_null() {
        return Vec::new();
    }
    // SAFETY: per the contract, `pbData` points to `cbData` valid bytes
    // allocated with LocalAlloc.
    let bytes = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) }.to_vec();
    if wipe {
        // SAFETY: same buffer, still owned by us until LocalFree below.
        unsafe { std::ptr::write_bytes(out.pbData, 0, out.cbData as usize) };
    }
    // SAFETY: the buffer was allocated by DPAPI with LocalAlloc and is freed
    // exactly once. A failing free only leaks, so the result is ignored.
    unsafe {
        let _ = LocalFree(Some(HLOCAL(out.pbData.cast())));
    }
    bytes
}

impl KeyProtector for DpapiProtector {
    fn name(&self) -> &'static str {
        "dpapi"
    }

    fn protect(&self, plain: &[u8]) -> Result<Vec<u8>> {
        let input = blob_of(plain)?;
        let entropy = blob_of(ENTROPY)?;
        let mut out = CRYPT_INTEGER_BLOB::default();
        // SAFETY: `input` and `entropy` point to live slices for the whole call;
        // `out` is a valid destination. UI is forbidden so the call can never
        // block on a prompt. No CRYPTPROTECT_LOCAL_MACHINE flag: user scope.
        unsafe {
            CryptProtectData(
                &input,
                w!("clipdump data key"),
                Some(&entropy),
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        }
        .context("CryptProtectData failed")?;
        // SAFETY: filled in by the successful call above. The output is
        // ciphertext, so no wipe is needed.
        Ok(unsafe { take_output(out, false) })
    }

    fn unprotect(&self, blob: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        if blob.is_empty() {
            bail!("key blob is empty");
        }
        let input = blob_of(blob)?;
        let entropy = blob_of(ENTROPY)?;
        let mut out = CRYPT_INTEGER_BLOB::default();
        // SAFETY: as in `protect`. The description output is not requested.
        unsafe {
            CryptUnprotectData(
                &input,
                None,
                Some(&entropy),
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        }
        .context("CryptUnprotectData failed (different user, machine or corrupt key.bin)")?;
        // SAFETY: filled in by the successful call above. The output is the
        // plaintext key, so wipe the OS buffer before freeing it.
        Ok(Zeroizing::new(unsafe { take_output(out, true) }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run with `cargo test` on Windows. Not runnable on the Linux dev
    /// container (it only type-checks there).
    #[test]
    fn round_trip() {
        let p = DpapiProtector;
        let secret = [7u8; 32];
        let blob = p.protect(&secret).unwrap();
        assert_ne!(blob, secret, "blob must not be the plaintext");
        assert!(blob.len() > secret.len());
        assert_eq!(&**p.unprotect(&blob).unwrap(), &secret);
    }

    #[test]
    fn tampered_blob_is_rejected() {
        let p = DpapiProtector;
        let mut blob = p.protect(&[1u8; 32]).unwrap();
        let last = blob.len() - 1;
        blob[last] ^= 0xff;
        assert!(p.unprotect(&blob).is_err());
    }

    #[test]
    fn garbage_is_rejected() {
        assert!(DpapiProtector.unprotect(b"definitely not a dpapi blob").is_err());
        assert!(DpapiProtector.unprotect(b"").is_err());
    }
}
