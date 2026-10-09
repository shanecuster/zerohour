//! Sync bookkeeping in SQLite: which `_rev` each document has on the server, what still needs
//! pushing, and the `_changes` checkpoint.
//!
//! These are extra methods on [`Db`], written here rather than in `db.rs` so local-only builds
//! don't compile them. (Rust lets one type have `impl` blocks in several files of the same
//! crate.)
//!
//! **How "needs pushing" works.** The `sync_docs` table (migration 2) has one row per document
//! with the `_rev` we last saw or wrote, and the `updated_at` of the version that rev holds.
//! - An item needs pushing if it has no row, or its `updated_at` differs from the row's
//!   `synced_updated_at`. Every local edit already changes `updated_at`, so none of the normal
//!   write paths need to know about sync at all.
//! - A completion needs pushing if it has no row (completions never change).

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};

use super::doc::{completion_doc_id, item_doc_id};
use super::keys;
use crate::db::{
    Db, Result, completions_where, get_item_on, insert_completion_on, replace_item_on,
    row_exists_on, ts_from_db, ts_to_db,
};
use crate::model::{Completion, Item};

/// One row of `sync_docs`: what we know about a document on the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncRow {
    /// The CouchDB `_rev` we last saw or wrote.
    pub rev: String,
    /// For items: the `updated_at` of the version stored under `rev`. `None` for completions.
    pub synced_updated_at: Option<DateTime<Utc>>,
}

impl Db {
    /// Runs `work` inside one SQLite transaction and commits it if `work` succeeds.
    ///
    /// Sync applies each page of server changes, plus the checkpoint for that page, in one
    /// transaction. A crash or error mid-page then rolls back the whole page *and* leaves the
    /// checkpoint where it was, so the next sync simply fetches that page again.
    ///
    /// `impl FnOnce(&SyncTx) -> Result<T>` = "any function or closure, called once, that takes
    /// a `SyncTx` and returns a `Result<T>`".
    pub(crate) fn sync_tx<T>(&mut self, work: impl FnOnce(&SyncTx<'_>) -> Result<T>) -> Result<T> {
        // IMMEDIATE takes the write lock right away. If the app's own connection is writing at
        // that moment, we wait (busy timeout) instead of failing halfway through the page.
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let value = work(&SyncTx { conn: &tx })?;
        tx.commit()?;
        Ok(value)
    }

    /// Every item that needs pushing, with the `_rev` to send (if the server has it already).
    pub(crate) fn items_to_push(&self) -> Result<Vec<(Item, Option<String>)>> {
        // LEFT JOIN keeps items that have no sync row (s.* is NULL for them). `IS NOT`
        // compares like `!=` but treats NULL as a normal value, so a NULL synced time counts
        // as "different".
        let mut stmt = self.conn.prepare(
            "SELECT i.id, s.rev FROM items i
             LEFT JOIN sync_docs s ON s.doc_id = 'item:' || i.id
             WHERE s.doc_id IS NULL OR s.synced_updated_at IS NOT i.updated_at
             ORDER BY i.updated_at",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, rev) = row?;
            if let Some(item) = get_item_on(&self.conn, &id)? {
                out.push((item, rev));
            }
        }
        Ok(out)
    }

    /// Every completion the server doesn't have yet.
    pub(crate) fn completions_to_push(&self) -> Result<Vec<Completion>> {
        completions_where(
            &self.conn,
            "NOT EXISTS (SELECT 1 FROM sync_docs s WHERE s.doc_id = 'completion:' || completions.id)",
        )
    }

    /// Forgets everything about the server: all `_rev`s and the checkpoint. Your data is not
    /// touched. The next sync then works like a first sync (pull everything, push everything),
    /// which is safe because merging is idempotent: applying the same version twice is a no-op.
    pub fn clear_sync_state(&mut self) -> Result<()> {
        self.sync_tx(|tx| {
            tx.conn.execute("DELETE FROM sync_docs", [])?;
            tx.conn
                .execute("DELETE FROM settings WHERE key = ?1", [keys::SINCE])?;
            Ok(())
        })
    }

    /// How many documents this device has synced. Shown in the settings screen.
    pub fn synced_doc_count(&self) -> Result<u64> {
        // SQLite integers are signed 64-bit; a count is never negative, so `unsigned_abs` is
        // just a lossless conversion.
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM sync_docs", [], |row| row.get(0))?;
        Ok(count.unsigned_abs())
    }
}

/// The database as seen from inside a sync transaction (see [`Db::sync_tx`]).
///
/// The `'a` is a *lifetime*: this struct borrows the transaction's connection, and Rust makes
/// sure it can't outlive that transaction.
pub(crate) struct SyncTx<'a> {
    conn: &'a Connection,
}

impl SyncTx<'_> {
    /// Loads an item (including deleted ones).
    pub fn get_item(&self, id: &str) -> Result<Option<Item>> {
        get_item_on(self.conn, id)
    }

    /// Writes an item, replacing any existing row with the same id.
    pub fn write_item(&self, item: &Item) -> Result<()> {
        replace_item_on(self.conn, item)
    }

    /// True if this completion is already in the local log.
    pub fn has_completion(&self, id: &str) -> Result<bool> {
        row_exists_on(self.conn, "completions", id)
    }

    /// Adds a completion to the local log.
    pub fn insert_completion(&self, completion: &Completion) -> Result<()> {
        insert_completion_on(self.conn, completion)
    }

    /// What we know about a document on the server, if anything.
    pub fn row(&self, doc_id: &str) -> Result<Option<SyncRow>> {
        let raw: Option<(String, Option<String>)> = self
            .conn
            .query_row(
                "SELECT rev, synced_updated_at FROM sync_docs WHERE doc_id = ?1",
                [doc_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        raw.map(|(rev, synced)| {
            Ok(SyncRow {
                rev,
                synced_updated_at: synced.as_deref().map(ts_from_db).transpose()?,
            })
        })
        .transpose()
    }

    /// Records the server's `_rev` for a document (and, for items, which version that is).
    pub fn set_row(
        &self,
        doc_id: &str,
        rev: &str,
        synced_updated_at: Option<DateTime<Utc>>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO sync_docs (doc_id, rev, synced_updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(doc_id) DO UPDATE SET rev = excluded.rev,
                                              synced_updated_at = excluded.synced_updated_at",
            params![doc_id, rev, synced_updated_at.map(ts_to_db)],
        )?;
        Ok(())
    }

    /// Forgets a document's `_rev`, so it gets pushed again as a new document.
    pub fn forget_row(&self, doc_id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM sync_docs WHERE doc_id = ?1", [doc_id])?;
        Ok(())
    }

    /// Records the `_rev` of an item version we just pushed.
    pub fn set_item_pushed(&self, item: &Item, rev: &str) -> Result<()> {
        self.set_row(&item_doc_id(&item.id), rev, Some(item.updated_at))
    }

    /// Records the `_rev` of a completion we just pushed (or found already on the server).
    pub fn set_completion_synced(&self, completion_id: &str, rev: &str) -> Result<()> {
        self.set_row(&completion_doc_id(completion_id), rev, None)
    }

    /// Writes a setting (the checkpoint, the last sync time...) inside the transaction.
    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }
}
