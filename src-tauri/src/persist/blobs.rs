//! Encrypted content-addressed files for image blobs and thumbnails.
//!
//! ```text
//! <data dir>/blobs/ab/<name>.bin    encrypted PNG
//! <data dir>/thumbs/ab/<name>.bin   encrypted WebP thumbnail
//! ```
//!
//! `ab` is the first two characters of the name, which keeps any one directory
//! small (NTFS gets slow with tens of thousands of entries in one folder).
//! Blobs are named by the `blob_hash` column; thumbnails by the item hash
//! (see [`hash_hex`]), so they exist for every item kind.
//!
//! Speed note: files are written to a temp file and atomically renamed, but not
//! fsynced. After a power cut a file may be missing or truncated; the startup
//! integrity check treats both as "blob lost" and drops the item, which costs
//! one clipboard entry instead of a disk flush on every image copy.

use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};

use super::crypto::{Crypto, OVERHEAD};

/// Which family of files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    /// Full size image, encrypted PNG.
    Blob,
    /// Small preview, encrypted WebP.
    Thumb,
}

impl FileKind {
    fn dir(self) -> &'static str {
        match self {
            Self::Blob => "blobs",
            Self::Thumb => "thumbs",
        }
    }
}

/// Lowercase hex of an item hash: the thumbnail file name (and by convention
/// the blob name too).
pub fn hash_hex(hash: &[u8; 16]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(32);
    for b in hash {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 15) as usize] as char);
    }
    s
}

/// A file found on disk by [`Blobs::scan`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub path: PathBuf,
    /// File name without `.bin`. Empty for temp files.
    pub name: String,
    /// A leftover `*.tmp` from an interrupted write.
    pub is_temp: bool,
}

pub struct Blobs {
    root: PathBuf,
    crypto: Arc<Crypto>,
    tmp_seq: AtomicU64,
}

/// Names become path components, so accept only a safe alphabet. Hex hashes
/// always pass; anything with a separator or dot does not.
fn check_name(name: &str) -> Result<()> {
    let ok = name.len() >= 2
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if !ok {
        bail!("invalid blob name {name:?}");
    }
    Ok(())
}

impl Blobs {
    /// `root` is the data directory (the parent of `blobs/` and `thumbs/`).
    /// Directories are created lazily on first write.
    pub fn new(root: &Path, crypto: Arc<Crypto>) -> Self {
        Self {
            root: root.to_path_buf(),
            crypto,
            tmp_seq: AtomicU64::new(0),
        }
    }

    pub fn path(&self, kind: FileKind, name: &str) -> Result<PathBuf> {
        check_name(name)?;
        // Names are ASCII (checked above), so slicing at 2 is on a boundary.
        Ok(self
            .root
            .join(kind.dir())
            .join(&name[..2])
            .join(format!("{name}.bin")))
    }

    /// Encrypt `plain` and store it under `name`.
    ///
    /// Content addressed: if a plausible file already exists the call is a
    /// no-op (the same name means the same content), which makes re-copying a
    /// screenshot free.
    pub fn write(&self, kind: FileKind, name: &str, plain: &[u8]) -> Result<()> {
        let path = self.path(kind, name)?;
        if self.is_valid(kind, name) {
            return Ok(());
        }
        let data = self.crypto.encrypt(plain)?;
        let dir = path.parent().expect("path has a shard directory");
        fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
        let seq = self.tmp_seq.fetch_add(1, Ordering::Relaxed);
        let tmp = dir.join(format!("{name}.{}.{seq}.tmp", std::process::id()));
        let result = (|| -> std::io::Result<()> {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(&data)?;
            drop(f);
            // Replaces an existing (truncated) file atomically on Windows and Unix.
            fs::rename(&tmp, &path)
        })();
        if let Err(e) = result {
            let _ = fs::remove_file(&tmp);
            return Err(e).with_context(|| format!("cannot write {}", path.display()));
        }
        Ok(())
    }

    /// Read and decrypt a file. Fails if it is missing, modified or truncated.
    pub fn read(&self, kind: FileKind, name: &str) -> Result<Vec<u8>> {
        let path = self.path(kind, name)?;
        let data = fs::read(&path).with_context(|| format!("cannot read {}", path.display()))?;
        self.crypto
            .decrypt(&data)
            .with_context(|| format!("cannot decrypt {}", path.display()))
    }

    /// Delete a file. Returns false if it did not exist.
    pub fn delete(&self, kind: FileKind, name: &str) -> Result<bool> {
        let path = self.path(kind, name)?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e).with_context(|| format!("cannot delete {}", path.display())),
        }
    }

    /// Cheap check used at startup: the file exists and is at least as long as
    /// an empty envelope. Does not read or decrypt it (1,000 images would
    /// otherwise cost seconds).
    pub fn is_valid(&self, kind: FileKind, name: &str) -> bool {
        let Ok(path) = self.path(kind, name) else {
            return false;
        };
        fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() >= OVERHEAD as u64)
    }

    /// Every file under `blobs/` or `thumbs/`, including leftover temp files.
    /// Unreadable directories are skipped.
    pub fn scan(&self, kind: FileKind) -> Vec<Found> {
        let mut out = Vec::new();
        let Ok(shards) = fs::read_dir(self.root.join(kind.dir())) else {
            return out;
        };
        for shard in shards.flatten() {
            let Ok(files) = fs::read_dir(shard.path()) else {
                continue;
            };
            for f in files.flatten() {
                let path = f.path();
                let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if file_name.ends_with(".tmp") {
                    out.push(Found {
                        path: path.clone(),
                        name: String::new(),
                        is_temp: true,
                    });
                } else if let Some(stem) = file_name.strip_suffix(".bin") {
                    out.push(Found {
                        name: stem.to_string(),
                        path: path.clone(),
                        is_temp: false,
                    });
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, Blobs) {
        let dir = tempfile::tempdir().unwrap();
        let blobs = Blobs::new(dir.path(), Arc::new(Crypto::from_key(&[3u8; 32])));
        (dir, blobs)
    }

    const NAME: &str = "ab0123456789abcdef0123456789abcd";

    #[test]
    fn write_read_round_trip_with_expected_layout() {
        let (dir, b) = setup();
        let data = vec![42u8; 100_000];
        b.write(FileKind::Blob, NAME, &data).unwrap();
        b.write(FileKind::Thumb, NAME, b"thumb").unwrap();
        assert!(
            dir.path()
                .join("blobs/ab")
                .join(format!("{NAME}.bin"))
                .is_file()
        );
        assert!(
            dir.path()
                .join("thumbs/ab")
                .join(format!("{NAME}.bin"))
                .is_file()
        );
        assert_eq!(b.read(FileKind::Blob, NAME).unwrap(), data);
        assert_eq!(b.read(FileKind::Thumb, NAME).unwrap(), b"thumb");
    }

    #[test]
    fn files_are_encrypted_and_no_temp_is_left() {
        let (dir, b) = setup();
        let marker = b"ZXQ-BLOB-MARKER-ZXQ";
        b.write(FileKind::Blob, NAME, &marker.repeat(10)).unwrap();
        let raw = fs::read(b.path(FileKind::Blob, NAME).unwrap()).unwrap();
        assert!(!raw.windows(marker.len()).any(|w| w == marker));
        assert!(b.scan(FileKind::Blob).iter().all(|f| !f.is_temp));
        let shard = dir.path().join("blobs/ab");
        assert_eq!(fs::read_dir(shard).unwrap().count(), 1);
    }

    #[test]
    fn writing_the_same_name_again_is_a_no_op() {
        let (_d, b) = setup();
        b.write(FileKind::Blob, NAME, b"first").unwrap();
        let before = fs::read(b.path(FileKind::Blob, NAME).unwrap()).unwrap();
        b.write(FileKind::Blob, NAME, b"second").unwrap();
        let after = fs::read(b.path(FileKind::Blob, NAME).unwrap()).unwrap();
        assert_eq!(before, after, "content addressed: existing file is kept");
    }

    #[test]
    fn truncated_file_is_invalid_and_gets_rewritten() {
        let (_d, b) = setup();
        let path = b.path(FileKind::Blob, NAME).unwrap();
        b.write(FileKind::Blob, NAME, b"data").unwrap();
        fs::write(&path, b"short").unwrap(); // crash left a stub
        assert!(!b.is_valid(FileKind::Blob, NAME));
        assert!(b.read(FileKind::Blob, NAME).is_err());
        b.write(FileKind::Blob, NAME, b"data").unwrap();
        assert!(b.is_valid(FileKind::Blob, NAME));
        assert_eq!(b.read(FileKind::Blob, NAME).unwrap(), b"data");
    }

    #[test]
    fn tampered_file_fails_to_read() {
        let (_d, b) = setup();
        b.write(FileKind::Blob, NAME, b"data").unwrap();
        let path = b.path(FileKind::Blob, NAME).unwrap();
        let mut raw = fs::read(&path).unwrap();
        raw[30] ^= 1;
        fs::write(&path, raw).unwrap();
        assert!(b.read(FileKind::Blob, NAME).is_err());
    }

    #[test]
    fn delete_is_idempotent() {
        let (_d, b) = setup();
        b.write(FileKind::Blob, NAME, b"x").unwrap();
        assert!(b.delete(FileKind::Blob, NAME).unwrap());
        assert!(!b.delete(FileKind::Blob, NAME).unwrap());
        assert!(b.read(FileKind::Blob, NAME).is_err());
        assert!(!b.is_valid(FileKind::Blob, NAME));
    }

    #[test]
    fn unsafe_names_are_rejected() {
        let (_d, b) = setup();
        for bad in ["", "a", "../escape", "ab/cd", "ab\\cd", "ab.bin", "a b"] {
            assert!(b.write(FileKind::Blob, bad, b"x").is_err(), "{bad:?}");
            assert!(b.read(FileKind::Blob, bad).is_err(), "{bad:?}");
            assert!(b.delete(FileKind::Blob, bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn scan_lists_files_and_stale_temps() {
        let (dir, b) = setup();
        b.write(FileKind::Blob, NAME, b"x").unwrap();
        b.write(FileKind::Blob, "cd99", b"y").unwrap();
        fs::write(dir.path().join("blobs/ab/leftover.1.2.tmp"), b"half").unwrap();
        let mut found = b.scan(FileKind::Blob);
        found.sort_by(|a, b| a.path.cmp(&b.path));
        assert_eq!(found.len(), 3);
        let temps: Vec<_> = found.iter().filter(|f| f.is_temp).collect();
        assert_eq!(temps.len(), 1);
        let mut names: Vec<_> = found
            .iter()
            .filter(|f| !f.is_temp)
            .map(|f| f.name.as_str())
            .collect();
        names.sort();
        assert_eq!(names, vec![NAME, "cd99"]);
        assert!(b.scan(FileKind::Thumb).is_empty(), "no thumbs dir yet");
    }

    #[test]
    fn hash_hex_is_32_lowercase_hex_chars() {
        let mut h = [0u8; 16];
        h[0] = 0xab;
        h[15] = 0x0f;
        assert_eq!(hash_hex(&h), "ab00000000000000000000000000000f");
    }
}
