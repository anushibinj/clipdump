//! Encrypted, crash safe persistence for clipboard history (Section 3 of PLAN.md).
//!
//! Layout on disk, under the data directory:
//!
//! ```text
//! history.db            SQLite (WAL), text columns encrypted
//! blobs/ab/<hash>.bin   encrypted PNG
//! thumbs/ab/<hash>.bin  encrypted WebP thumbnail
//! key.bin               32 byte data key, protected by the platform keystore
//! ```

pub mod blobs;
pub mod crypto;
pub mod db;
