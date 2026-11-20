//! SQLite access: connections, pragmas, schema migrations and queries.
//!
//! Every function here deals in already encrypted bytes (`*_enc` columns); the
//! owner of the [`crate::persist::crypto::Crypto`] decides when to encrypt and
//! decrypt. That keeps SQL free of crypto and lets the writer thread encrypt in
//! one place.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use rusqlite::{Connection, OpenFlags, params};

use crate::model::{ClipKind, OcrStatus};

/// File name of the database inside the data directory.
pub const DB_FILE: &str = "history.db";

/// Marker error: `history.db` opens but its pages are inconsistent
/// (`PRAGMA quick_check` said so). Together with SQLite's own "malformed" and
/// "not a database" codes it is what `persist::recovery` treats as a damaged file.
#[derive(Debug)]
pub struct DatabaseDamaged;

impl std::fmt::Display for DatabaseDamaged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("history.db failed its integrity check")
    }
}

impl std::error::Error for DatabaseDamaged {}

/// Is `e` (or anything it wraps) a sign that the database FILE is damaged, as
/// opposed to a busy, full or unreadable disk, which must never make anyone move
/// the history aside?
pub fn is_damage(e: &anyhow::Error) -> bool {
    use rusqlite::ErrorCode::{DatabaseCorrupt, NotADatabase};
    e.downcast_ref::<DatabaseDamaged>().is_some()
        || e.chain().any(|c| {
            matches!(
                c.downcast_ref::<rusqlite::Error>(),
                Some(rusqlite::Error::SqliteFailure(f, _))
                    if matches!(f.code, DatabaseCorrupt | NotADatabase)
            )
        })
}

/// Databases up to this size get a `quick_check` before opening (a few
/// milliseconds for a normal history); a bigger one is only probed (header and
/// schema), so a huge file cannot make startup slow. Damage deeper inside it is
/// met, and handled the same way, when the history is read.
pub const QUICK_CHECK_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// Look at an existing `history.db` before the writer touches it, through a
/// READ ONLY connection. That matters: closing the last read/write connection of a
/// damaged WAL database makes SQLite try a checkpoint and delete the `-wal`, and
/// the point of moving a damaged history aside is to keep all of it. A read only
/// connection changes nothing on disk.
///
/// A missing or empty file is fine (a first run). Damage returns an error that
/// [`is_damage`] recognizes: SQLite's "not a database" or "malformed", or
/// [`DatabaseDamaged`] from `PRAGMA quick_check(1)`, which walks every page once
/// (cheaper than `integrity_check`: no index against table comparison).
pub fn preflight(path: &Path) -> Result<()> {
    let len = match std::fs::metadata(path) {
        Ok(m) => m.len(),
        Err(_) => return Ok(()),
    };
    if len == 0 {
        return Ok(());
    }
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("cannot open database {} for checking", path.display()))?;
    conn.busy_timeout(Duration::from_secs(5))?;
    if len > QUICK_CHECK_MAX_BYTES {
        conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| {
            r.get::<_, i64>(0)
        })?;
        return Ok(());
    }
    let verdict: String = conn.query_row("PRAGMA quick_check(1)", [], |r| r.get(0))?;
    if verdict.eq_ignore_ascii_case("ok") {
        Ok(())
    } else {
        // The verdict names pages and tables, never row content.
        Err(anyhow::Error::new(DatabaseDamaged).context(format!("quick_check: {verdict}")))
    }
}

/// Schema v1, verbatim from Section 3 of PLAN.md.
const SCHEMA_V1: &str = "
CREATE TABLE items (
  id            INTEGER PRIMARY KEY,
  hash          BLOB NOT NULL UNIQUE,   -- xxh3-128 of normalized content
  kind          INTEGER NOT NULL,       -- 0 text, 1 image, 2 files
  created_at    INTEGER NOT NULL,       -- unix ms
  last_used_at  INTEGER NOT NULL,
  use_count     INTEGER NOT NULL DEFAULT 0,
  pinned        INTEGER NOT NULL DEFAULT 0,
  preview_enc   BLOB NOT NULL,          -- encrypted short preview
  text_enc      BLOB,                   -- encrypted full text (text items)
  ocr_enc       BLOB,                   -- encrypted OCR text (image items)
  ocr_status    INTEGER NOT NULL DEFAULT 0, -- 0 pending, 1 done, 2 failed, 3 unavailable
  files_enc     BLOB,                   -- encrypted JSON [{path,size,is_dir}]
  blob_hash     TEXT,                   -- image blob file name
  width         INTEGER, height INTEGER,
  byte_size     INTEGER NOT NULL,
  source_app    TEXT                    -- exe name of clipboard owner
);
CREATE INDEX idx_items_created ON items(created_at);
CREATE INDEX idx_items_pinned ON items(pinned);
";

/// Schema v2: the pieces an LLM split a text into, encrypted, one row per text
/// (by its content hash). Rows exist only while an item with that hash does.
const SCHEMA_V2: &str = "
CREATE TABLE splits (
  hash        BLOB PRIMARY KEY,   -- content hash of the text item
  data_enc    BLOB NOT NULL,      -- encrypted JSON [{label,value}]
  created_at  INTEGER NOT NULL    -- unix ms
);
";

/// Ordered migrations. `PRAGMA user_version` is the number already applied, so
/// to change the schema append a script here; never edit an old one.
const MIGRATIONS: &[&str] = &[SCHEMA_V1, SCHEMA_V2];

/// A full `items` row, with every sensitive column still encrypted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemRow {
    pub id: i64,
    /// xxh3-128 of the normalized content, big endian.
    pub hash: [u8; 16],
    pub kind: ClipKind,
    pub created_at: i64,
    pub last_used_at: i64,
    pub use_count: u32,
    pub pinned: bool,
    pub preview_enc: Vec<u8>,
    pub text_enc: Option<Vec<u8>>,
    pub ocr_enc: Option<Vec<u8>>,
    pub ocr_status: OcrStatus,
    pub files_enc: Option<Vec<u8>>,
    pub blob_hash: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub byte_size: u64,
    pub source_app: Option<String>,
}

/// What startup needs from a row. Same as [`ItemRow`] except for the text.
///
/// Speed note: `text_enc` can be megabytes per item. To keep the cold start
/// under budget, `load_all_meta` returns it only when it is at most the given
/// size limit (the "keep full text in RAM" threshold). For larger texts
/// `has_text` is true and the caller fetches it with [`load_text`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MetaRow {
    pub id: i64,
    pub hash: [u8; 16],
    pub kind: ClipKind,
    pub created_at: i64,
    pub last_used_at: i64,
    pub use_count: u32,
    pub pinned: bool,
    pub preview_enc: Vec<u8>,
    pub has_text: bool,
    pub text_enc: Option<Vec<u8>>,
    pub ocr_enc: Option<Vec<u8>>,
    pub ocr_status: OcrStatus,
    pub files_enc: Option<Vec<u8>>,
    pub blob_hash: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub byte_size: u64,
    pub source_app: Option<String>,
}

/// Open the read/write connection used by the single writer thread (and by
/// startup integrity checks). Creates the file and applies migrations.
pub fn open_writer(path: &Path) -> Result<Connection> {
    let mut conn = Connection::open(path)
        .with_context(|| format!("cannot open database {}", path.display()))?;
    conn.busy_timeout(Duration::from_secs(5))?;
    apply_pragmas(&conn)?;
    migrate(&mut conn)?;
    Ok(conn)
}

/// Open a read only connection for lookups on caller threads. In WAL mode a
/// reader never blocks the writer and sees every committed transaction. The
/// database must already exist (the writer connection creates it).
pub fn open_reader(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("cannot open database {} for reading", path.display()))?;
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    Ok(conn)
}

fn apply_pragmas(conn: &Connection) -> Result<()> {
    // WAL: appends instead of rewriting pages, readers never block the writer.
    let mode: String = conn.pragma_update_and_check(None, "journal_mode", "WAL", |r| r.get(0))?;
    if !mode.eq_ignore_ascii_case("wal") {
        bail!("could not enable WAL mode (got {mode})");
    }
    // NORMAL: no fsync per commit. A power cut can lose the last few
    // transactions (never corrupt the file), which is the right trade for a
    // clipboard history where speed is the top requirement.
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    // Keep temp tables and sort buffers off disk (they could hold plaintext
    // ordering data and disk is slow).
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    Ok(())
}

/// Bring the database up to the latest schema using `PRAGMA user_version`.
pub fn migrate(conn: &mut Connection) -> Result<()> {
    migrate_with(conn, MIGRATIONS)
}

/// [`migrate`] with an explicit migration list, so tests can exercise upgrades.
pub(crate) fn migrate_with(conn: &mut Connection, migrations: &[&str]) -> Result<()> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let current = usize::try_from(current).map_err(|_| anyhow!("negative user_version"))?;
    if current > migrations.len() {
        bail!(
            "history.db has schema version {current}, but this build only knows up to {}",
            migrations.len()
        );
    }
    for (i, script) in migrations.iter().enumerate().skip(current) {
        // One transaction per migration: either fully applied with its version
        // bump, or not at all.
        let tx = conn.transaction()?;
        tx.execute_batch(script)
            .with_context(|| format!("migration to version {} failed", i + 1))?;
        // PRAGMA arguments cannot be bound parameters; `i + 1` is our own integer.
        tx.execute_batch(&format!("PRAGMA user_version = {}", i + 1))?;
        tx.commit()?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Value conversion helpers
// ---------------------------------------------------------------------------

fn to_i64(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

fn hash_from(v: Vec<u8>) -> rusqlite::Result<[u8; 16]> {
    <[u8; 16]>::try_from(v.as_slice()).map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(
            1,
            rusqlite::types::Type::Blob,
            "hash is not 16 bytes".into(),
        )
    })
}

fn bad_code(col: usize, what: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        col,
        rusqlite::types::Type::Integer,
        format!("unknown {what} code").into(),
    )
}

// ---------------------------------------------------------------------------
// Queries
// ---------------------------------------------------------------------------

/// Insert a new item. The id is chosen by the in-memory store (it must be able
/// to hand out ids without waiting for the writer thread), so callers get ids
/// from a counter seeded with [`max_id`]` + 1`.
///
/// Fails if the id or the hash already exists: dedup is the store's job, and
/// a violation should be loud rather than silently dropped.
pub fn insert_item(conn: &Connection, row: &ItemRow) -> Result<()> {
    let mut stmt = conn.prepare_cached(
        "INSERT INTO items (id, hash, kind, created_at, last_used_at, use_count, pinned,
                            preview_enc, text_enc, ocr_enc, ocr_status, files_enc, blob_hash,
                            width, height, byte_size, source_app)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
    )?;
    stmt.execute(params![
        row.id,
        &row.hash[..],
        row.kind.code(),
        row.created_at,
        row.last_used_at,
        row.use_count,
        row.pinned,
        row.preview_enc,
        row.text_enc,
        row.ocr_enc,
        row.ocr_status.code(),
        row.files_enc,
        row.blob_hash,
        row.width,
        row.height,
        to_i64(row.byte_size),
        row.source_app,
    ])?;
    Ok(())
}

/// Load every item for the startup index, newest use first. See [`MetaRow`]
/// for what `small_text_limit` does (it is a limit on *encrypted* bytes).
pub fn load_all_meta(conn: &Connection, small_text_limit: usize) -> Result<Vec<MetaRow>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, hash, kind, created_at, last_used_at, use_count, pinned, preview_enc,
                text_enc IS NOT NULL,
                CASE WHEN length(text_enc) <= ?1 THEN text_enc END,
                ocr_enc, ocr_status, files_enc, blob_hash, width, height, byte_size, source_app
         FROM items
         ORDER BY last_used_at DESC, id DESC",
    )?;
    let rows = stmt.query_map([to_i64(small_text_limit as u64)], |r| {
        Ok(MetaRow {
            id: r.get(0)?,
            hash: hash_from(r.get(1)?)?,
            kind: ClipKind::from_code(r.get(2)?).ok_or_else(|| bad_code(2, "kind"))?,
            created_at: r.get(3)?,
            last_used_at: r.get(4)?,
            use_count: r.get(5)?,
            pinned: r.get(6)?,
            preview_enc: r.get(7)?,
            has_text: r.get(8)?,
            text_enc: r.get(9)?,
            ocr_enc: r.get(10)?,
            ocr_status: OcrStatus::from_code(r.get(11)?)
                .ok_or_else(|| bad_code(11, "ocr_status"))?,
            files_enc: r.get(12)?,
            blob_hash: r.get(13)?,
            width: r.get(14)?,
            height: r.get(15)?,
            byte_size: u64::try_from(r.get::<_, i64>(16)?).unwrap_or(0),
            source_app: r.get(17)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// The encrypted full text of one item (`None` for a missing item or an item
/// without text).
pub fn load_text(conn: &Connection, id: i64) -> Result<Option<Vec<u8>>> {
    let mut stmt = conn.prepare_cached("SELECT text_enc FROM items WHERE id = ?1")?;
    let mut rows = stmt.query([id])?;
    match rows.next()? {
        Some(row) => Ok(row.get(0)?),
        None => Ok(None),
    }
}

/// Store (or replace) the encrypted split of the text with content `hash`, but only
/// while an item with that hash exists: a split that finishes after its item was
/// deleted must not leave the pieces (often passwords) behind.
pub fn set_split(conn: &Connection, hash: &[u8; 16], data_enc: &[u8], at: i64) -> Result<bool> {
    let n = conn.execute(
        "INSERT OR REPLACE INTO splits (hash, data_enc, created_at)
         SELECT ?1, ?2, ?3 WHERE EXISTS (SELECT 1 FROM items WHERE hash = ?1)",
        rusqlite::params![hash.as_slice(), data_enc, at],
    )?;
    Ok(n > 0)
}

/// The encrypted split of the text with content `hash`, if one was stored.
pub fn load_split(conn: &Connection, hash: &[u8; 16]) -> Result<Option<Vec<u8>>> {
    let mut stmt = conn.prepare_cached("SELECT data_enc FROM splits WHERE hash = ?1")?;
    let mut rows = stmt.query([hash.as_slice()])?;
    match rows.next()? {
        Some(row) => Ok(Some(row.get(0)?)),
        None => Ok(None),
    }
}

/// Drop the splits whose item is gone (called after every delete).
pub fn prune_splits(conn: &Connection) -> Result<()> {
    conn.execute(
        "DELETE FROM splits WHERE hash NOT IN (SELECT hash FROM items)",
        [],
    )?;
    Ok(())
}

/// Largest id in use, or 0 for an empty table. Seeds the store's id counter.
pub fn max_id(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("SELECT COALESCE(MAX(id), 0) FROM items", [], |r| r.get(0))?)
}

/// Record that an item was copied again: bumps `use_count` and sets
/// `last_used_at` (unix ms). Returns false if the id does not exist.
pub fn update_used(conn: &Connection, id: i64, last_used_at: i64) -> Result<bool> {
    let mut stmt = conn.prepare_cached(
        "UPDATE items SET last_used_at = ?2, use_count = use_count + 1 WHERE id = ?1",
    )?;
    Ok(stmt.execute(params![id, last_used_at])? > 0)
}

/// Pin or unpin an item. Returns false if the id does not exist.
pub fn set_pinned(conn: &Connection, id: i64, pinned: bool) -> Result<bool> {
    let mut stmt = conn.prepare_cached("UPDATE items SET pinned = ?2 WHERE id = ?1")?;
    Ok(stmt.execute(params![id, pinned])? > 0)
}

/// Store the OCR result (already encrypted; `None` when there is no text, for
/// example on failure) and the new status. Returns false if the id does not exist.
pub fn set_ocr(
    conn: &Connection,
    id: i64,
    ocr_enc: Option<&[u8]>,
    status: OcrStatus,
) -> Result<bool> {
    let mut stmt =
        conn.prepare_cached("UPDATE items SET ocr_enc = ?2, ocr_status = ?3 WHERE id = ?1")?;
    Ok(stmt.execute(params![id, ocr_enc, status.code()])? > 0)
}

/// An item removed by a delete query. The caller removes the files that belong
/// to it after the transaction commits: the blob named `blob_hash` (if any) and
/// the thumbnail named after `hash`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Removed {
    pub id: i64,
    pub hash: [u8; 16],
    pub blob_hash: Option<String>,
}

fn collect_removed(
    stmt: &mut rusqlite::Statement<'_>,
    args: impl rusqlite::Params,
) -> Result<Vec<Removed>> {
    let rows = stmt.query_map(args, |r| {
        Ok(Removed {
            id: r.get(0)?,
            hash: hash_from(r.get(1)?)?,
            blob_hash: r.get(2)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// Delete one item (pinned or not: an explicit delete always wins). Returns
/// the removed row, `None` if the id does not exist.
pub fn delete_item(conn: &Connection, id: i64) -> Result<Option<Removed>> {
    let mut stmt =
        conn.prepare_cached("DELETE FROM items WHERE id = ?1 RETURNING id, hash, blob_hash")?;
    Ok(collect_removed(&mut stmt, [id])?.pop())
}

/// Delete every unpinned item ("clear history"). Pinned items are kept.
pub fn delete_unpinned(conn: &Connection) -> Result<Vec<Removed>> {
    let mut stmt =
        conn.prepare_cached("DELETE FROM items WHERE pinned = 0 RETURNING id, hash, blob_hash")?;
    collect_removed(&mut stmt, [])
}

/// Delete unpinned items created before `cutoff` (unix ms). Age is measured
/// from `created_at` (the indexed column), so copying an old item again does
/// not extend its life; pinning does.
pub fn delete_older_than(conn: &Connection, cutoff: i64) -> Result<Vec<Removed>> {
    let mut stmt = conn.prepare_cached(
        "DELETE FROM items WHERE pinned = 0 AND created_at < ?1 RETURNING id, hash, blob_hash",
    )?;
    collect_removed(&mut stmt, [cutoff])
}

/// The few columns the startup integrity check needs, without loading any
/// encrypted payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefRow {
    pub id: i64,
    pub hash: [u8; 16],
    pub kind: ClipKind,
    pub blob_hash: Option<String>,
}

/// Every item's id, hash, kind and blob name.
pub fn list_refs(conn: &Connection) -> Result<Vec<RefRow>> {
    let mut stmt = conn.prepare_cached("SELECT id, hash, kind, blob_hash FROM items")?;
    let rows = stmt.query_map([], |r| {
        Ok(RefRow {
            id: r.get(0)?,
            hash: hash_from(r.get(1)?)?,
            kind: ClipKind::from_code(r.get(2)?).ok_or_else(|| bad_code(2, "kind"))?,
            blob_hash: r.get(3)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn memory() -> Connection {
        let dir = tempfile::tempdir().unwrap();
        // Leak the dir on purpose: tests are short lived and the connection
        // needs the file for its whole life.
        let path = dir.keep().join(DB_FILE);
        open_writer(&path).unwrap()
    }

    /// A fully populated text item whose id and hash derive from `id`.
    pub fn sample(id: i64) -> ItemRow {
        let mut hash = [0u8; 16];
        hash[8..].copy_from_slice(&(id as u64).to_be_bytes());
        ItemRow {
            id,
            hash,
            kind: ClipKind::Text,
            created_at: 1_000 + id,
            last_used_at: 1_000 + id,
            use_count: 0,
            pinned: false,
            preview_enc: vec![1, 2, 3, id as u8],
            text_enc: Some(vec![9; 40]),
            ocr_enc: None,
            ocr_status: OcrStatus::Pending,
            files_enc: None,
            blob_hash: None,
            width: None,
            height: None,
            byte_size: 40,
            source_app: Some("notepad.exe".into()),
        }
    }

    fn user_version(conn: &Connection) -> i64 {
        conn.query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn open_applies_pragmas_and_schema() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_writer(&dir.path().join(DB_FILE)).unwrap();
        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
        // synchronous: 1 = NORMAL, temp_store: 2 = MEMORY
        let sync: i64 = conn
            .query_row("PRAGMA synchronous", [], |r| r.get(0))
            .unwrap();
        assert_eq!(sync, 1);
        let temp: i64 = conn
            .query_row("PRAGMA temp_store", [], |r| r.get(0))
            .unwrap();
        assert_eq!(temp, 2);
        assert_eq!(user_version(&conn), MIGRATIONS.len() as i64);
        let idx: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='index' AND name IN ('idx_items_created','idx_items_pinned')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(idx, 2);
    }

    #[test]
    fn reopen_keeps_data_and_does_not_remigrate() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DB_FILE);
        {
            let conn = open_writer(&path).unwrap();
            insert_item(&conn, &sample(1)).unwrap();
        }
        let conn = open_writer(&path).unwrap();
        assert_eq!(load_all_meta(&conn, 1 << 20).unwrap().len(), 1);
        assert_eq!(user_version(&conn), MIGRATIONS.len() as i64);
    }

    #[test]
    fn upgrade_runs_only_missing_migrations() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = Connection::open(dir.path().join("t.db")).unwrap();
        migrate_with(&mut conn, &[SCHEMA_V1]).unwrap();
        insert_item(&conn, &sample(1)).unwrap();
        // A later release adds a column: existing rows survive.
        let v2 = "ALTER TABLE items ADD COLUMN note TEXT;";
        migrate_with(&mut conn, &[SCHEMA_V1, v2]).unwrap();
        assert_eq!(user_version(&conn), 2);
        let n: i64 = conn
            .query_row("SELECT count(*) FROM items", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
        // Running again is a no-op (the ALTER would fail if repeated).
        migrate_with(&mut conn, &[SCHEMA_V1, v2]).unwrap();
        assert_eq!(user_version(&conn), 2);
    }

    #[test]
    fn failed_migration_rolls_back() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = Connection::open(dir.path().join("t.db")).unwrap();
        assert!(migrate_with(&mut conn, &[SCHEMA_V1, "THIS IS NOT SQL"]).is_err());
        // v1 was applied and kept, v2 left no trace.
        assert_eq!(user_version(&conn), 1);
    }

    #[test]
    fn refuses_a_database_from_the_future() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = Connection::open(dir.path().join("t.db")).unwrap();
        conn.execute_batch("PRAGMA user_version = 99").unwrap();
        let err = migrate(&mut conn).unwrap_err().to_string();
        assert!(err.contains("99"), "{err}");
    }

    #[test]
    fn insert_then_load_round_trips_every_column() {
        let conn = memory();
        let mut img = sample(2);
        img.kind = ClipKind::Image;
        img.text_enc = None;
        img.ocr_enc = Some(vec![4, 4]);
        img.ocr_status = OcrStatus::Done;
        img.blob_hash = Some("abcdef0123456789abcdef0123456789".into());
        img.width = Some(1920);
        img.height = Some(1080);
        img.byte_size = 5_000_000_000; // above u32
        img.pinned = true;
        img.use_count = 7;
        let mut files = sample(3);
        files.kind = ClipKind::Files;
        files.text_enc = None;
        files.files_enc = Some(vec![5, 5, 5]);
        insert_item(&conn, &sample(1)).unwrap();
        insert_item(&conn, &img).unwrap();
        insert_item(&conn, &files).unwrap();

        let meta = load_all_meta(&conn, 1 << 20).unwrap();
        assert_eq!(meta.len(), 3);
        // Newest first (last_used_at DESC).
        assert_eq!(meta.iter().map(|m| m.id).collect::<Vec<_>>(), vec![3, 2, 1]);
        let m = meta.iter().find(|m| m.id == 2).unwrap();
        assert_eq!(m.hash, img.hash);
        assert_eq!(m.kind, ClipKind::Image);
        assert_eq!((m.width, m.height), (Some(1920), Some(1080)));
        assert_eq!(m.byte_size, 5_000_000_000);
        assert!(m.pinned);
        assert_eq!(m.use_count, 7);
        assert_eq!(m.ocr_enc, Some(vec![4, 4]));
        assert_eq!(m.ocr_status, OcrStatus::Done);
        assert_eq!(
            m.blob_hash.as_deref(),
            Some("abcdef0123456789abcdef0123456789")
        );
        assert!(!m.has_text && m.text_enc.is_none());
        let t = meta.iter().find(|m| m.id == 1).unwrap();
        assert!(t.has_text);
        assert_eq!(t.text_enc, Some(vec![9; 40]));
        assert_eq!(t.source_app.as_deref(), Some("notepad.exe"));
        let f = meta.iter().find(|m| m.id == 3).unwrap();
        assert_eq!(f.files_enc, Some(vec![5, 5, 5]));
        assert_eq!(max_id(&conn).unwrap(), 3);
    }

    #[test]
    fn large_text_is_left_out_of_meta_and_loaded_on_demand() {
        let conn = memory();
        let mut big = sample(1);
        big.text_enc = Some(vec![7; 100_000]);
        insert_item(&conn, &big).unwrap();
        insert_item(&conn, &sample(2)).unwrap();
        let meta = load_all_meta(&conn, 65_536).unwrap();
        let b = meta.iter().find(|m| m.id == 1).unwrap();
        assert!(b.has_text);
        assert!(b.text_enc.is_none(), "over the limit, must not be loaded");
        assert!(meta.iter().find(|m| m.id == 2).unwrap().text_enc.is_some());
        assert_eq!(load_text(&conn, 1).unwrap(), Some(vec![7; 100_000]));
        assert_eq!(load_text(&conn, 2).unwrap(), Some(vec![9; 40]));
        assert_eq!(load_text(&conn, 99).unwrap(), None);
    }

    #[test]
    fn duplicate_hash_or_id_is_rejected() {
        let conn = memory();
        insert_item(&conn, &sample(1)).unwrap();
        assert!(insert_item(&conn, &sample(1)).is_err(), "same id");
        let mut dup_hash = sample(2);
        dup_hash.hash = sample(1).hash;
        assert!(insert_item(&conn, &dup_hash).is_err(), "same hash");
    }

    #[test]
    fn empty_table_has_max_id_zero() {
        assert_eq!(max_id(&memory()).unwrap(), 0);
    }

    fn meta_of(conn: &Connection, id: i64) -> MetaRow {
        load_all_meta(conn, 1 << 20)
            .unwrap()
            .into_iter()
            .find(|m| m.id == id)
            .unwrap()
    }

    #[test]
    fn update_used_bumps_count_and_time() {
        let conn = memory();
        insert_item(&conn, &sample(1)).unwrap();
        insert_item(&conn, &sample(2)).unwrap();
        assert!(update_used(&conn, 1, 50_000).unwrap());
        assert!(update_used(&conn, 1, 60_000).unwrap());
        let m = meta_of(&conn, 1);
        assert_eq!((m.use_count, m.last_used_at), (2, 60_000));
        assert_eq!(m.created_at, 1_001, "created_at never changes");
        assert_eq!(meta_of(&conn, 2).use_count, 0, "other rows untouched");
        // The copied item is now the newest.
        assert_eq!(load_all_meta(&conn, 1 << 20).unwrap()[0].id, 1);
        assert!(!update_used(&conn, 99, 1).unwrap(), "unknown id");
    }

    #[test]
    fn set_pinned_toggles_one_row() {
        let conn = memory();
        insert_item(&conn, &sample(1)).unwrap();
        insert_item(&conn, &sample(2)).unwrap();
        assert!(set_pinned(&conn, 1, true).unwrap());
        assert!(meta_of(&conn, 1).pinned);
        assert!(!meta_of(&conn, 2).pinned);
        assert!(set_pinned(&conn, 1, false).unwrap());
        assert!(!meta_of(&conn, 1).pinned);
        assert!(!set_pinned(&conn, 99, true).unwrap());
    }

    #[test]
    fn set_ocr_stores_text_and_status() {
        let conn = memory();
        let mut img = sample(1);
        img.kind = ClipKind::Image;
        insert_item(&conn, &img).unwrap();
        assert_eq!(meta_of(&conn, 1).ocr_status, OcrStatus::Pending);
        assert!(set_ocr(&conn, 1, Some(&[8, 8, 8]), OcrStatus::Done).unwrap());
        let m = meta_of(&conn, 1);
        assert_eq!(
            (m.ocr_enc, m.ocr_status),
            (Some(vec![8, 8, 8]), OcrStatus::Done)
        );
        // A failure clears the text and records why there is none.
        assert!(set_ocr(&conn, 1, None, OcrStatus::Failed).unwrap());
        let m = meta_of(&conn, 1);
        assert_eq!((m.ocr_enc, m.ocr_status), (None, OcrStatus::Failed));
        assert!(!set_ocr(&conn, 99, None, OcrStatus::Unavailable).unwrap());
    }

    fn ids(conn: &Connection) -> Vec<i64> {
        let mut v: Vec<i64> = load_all_meta(conn, 0)
            .unwrap()
            .iter()
            .map(|m| m.id)
            .collect();
        v.sort();
        v
    }

    #[test]
    fn delete_item_returns_files_to_clean_up() {
        let conn = memory();
        let mut img = sample(1);
        img.blob_hash = Some("aa11".into());
        insert_item(&conn, &img).unwrap();
        insert_item(&conn, &sample(2)).unwrap();
        let gone = delete_item(&conn, 1).unwrap().unwrap();
        assert_eq!(gone.id, 1);
        assert_eq!(gone.hash, img.hash);
        assert_eq!(gone.blob_hash.as_deref(), Some("aa11"));
        assert_eq!(ids(&conn), vec![2]);
        assert_eq!(delete_item(&conn, 1).unwrap(), None, "already gone");
    }

    #[test]
    fn delete_item_removes_pinned_too() {
        let conn = memory();
        insert_item(&conn, &sample(1)).unwrap();
        set_pinned(&conn, 1, true).unwrap();
        assert!(delete_item(&conn, 1).unwrap().is_some());
        assert!(ids(&conn).is_empty());
    }

    #[test]
    fn delete_unpinned_keeps_pinned() {
        let conn = memory();
        for id in 1..=4 {
            let mut r = sample(id);
            r.blob_hash = Some(format!("b{id}"));
            insert_item(&conn, &r).unwrap();
        }
        set_pinned(&conn, 2, true).unwrap();
        let mut removed = delete_unpinned(&conn).unwrap();
        removed.sort_by_key(|r| r.id);
        assert_eq!(
            removed.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![1, 3, 4]
        );
        assert_eq!(removed[0].blob_hash.as_deref(), Some("b1"));
        assert_eq!(ids(&conn), vec![2]);
        assert!(delete_unpinned(&conn).unwrap().is_empty());
    }

    #[test]
    fn delete_older_than_uses_created_at_and_spares_pinned() {
        let conn = memory();
        // created_at = 1000 + id
        for id in 1..=5 {
            insert_item(&conn, &sample(id)).unwrap();
        }
        set_pinned(&conn, 1, true).unwrap();
        // Re-using an old item must not save it.
        update_used(&conn, 2, 1_000_000).unwrap();
        // Cutoff 1004: ids 1..=3 are older; 1 is pinned. 4 is exactly 1004 (kept).
        let mut removed = delete_older_than(&conn, 1_004).unwrap();
        removed.sort_by_key(|r| r.id);
        assert_eq!(removed.iter().map(|r| r.id).collect::<Vec<_>>(), vec![2, 3]);
        assert_eq!(ids(&conn), vec![1, 4, 5]);
    }
}
