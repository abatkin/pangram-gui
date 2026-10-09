//! Local scan history in SQLite.
//!
//! [`Repository`] pairs an optional on-disk database with an in-memory one. Scans are written to
//! disk when history saving is enabled; otherwise (or if the disk write fails) they live only in
//! memory for the session. Both use the same schema and code path.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;

use crate::api::DetectionResult;
use crate::cost;

/// Schema version written to `PRAGMA user_version`.
pub const SCHEMA_VERSION: i64 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ScanState {
    Submitting,
    Polling,
    Completed,
    Failed,
    Paused,
    SubmissionUnknown,
}

impl ScanState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Submitting => "submitting",
            Self::Polling => "polling",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Paused => "paused",
            Self::SubmissionUnknown => "submissionUnknown",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "submitting" => Self::Submitting,
            "polling" => Self::Polling,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "paused" => Self::Paused,
            "submissionUnknown" => Self::SubmissionUnknown,
            _ => return None,
        })
    }

    pub fn is_active(self) -> bool {
        matches!(self, Self::Submitting | Self::Polling)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanRecord {
    pub id: String,
    /// Unix milliseconds.
    pub created_at: i64,
    pub updated_at: i64,
    pub title: String,
    /// Exactly what was submitted.
    pub input_text: String,
    pub requested_model: String,
    pub task_id: Option<String>,
    pub state: ScanState,
    pub error: Option<String>,
    /// Text as returned (possibly normalised) by Pangram; highlight ranges address this.
    pub returned_text: Option<String>,
    pub returned_version: Option<String>,
    pub prediction_short: Option<String>,
    pub fraction_ai: Option<f64>,
    pub raw_response: Option<String>,
    /// The scan this one was edited from, if any.
    pub source_scan_id: Option<String>,
    /// Raw `POST /task` response body.
    pub submit_response: Option<String>,
    /// Billing estimate recorded when the scan completed.
    pub billed_words: Option<i64>,
    pub credits: Option<i64>,
    pub cost_usd: Option<f64>,
    /// Price per credit when the scan was submitted; its cost is computed with this price.
    pub usd_per_credit: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanSummary {
    pub id: String,
    pub created_at: i64,
    pub title: String,
    pub state: ScanState,
    pub prediction_short: Option<String>,
    pub fraction_ai: Option<f64>,
    pub preview: String,
    /// False for session-only scans.
    pub saved: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("history database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("the history database was created by a newer version of this app (schema {0})")]
    TooNew(i64),
    #[error("could not create the history directory: {0}")]
    Io(#[from] std::io::Error),
    #[error("corrupt history record: {0}")]
    Corrupt(String),
}

/// One completed scan's charge. Kept separately from scans (and without any text) so the cost
/// summary survives deleting history.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageRecord {
    pub scan_id: String,
    pub created_at: i64,
    pub model: String,
    pub version: Option<String>,
    pub words: i64,
    pub credits: i64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsagePeriod {
    Day,
    Week,
    Month,
}

impl UsagePeriod {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "day" => Self::Day,
            "week" => Self::Week,
            "month" => Self::Month,
            _ => return None,
        })
    }

    /// SQLite expression grouping `created_at` (Unix ms) by local calendar period. Weeks start
    /// on Monday and are keyed by that Monday's date.
    fn group_expr(self) -> &'static str {
        match self {
            Self::Day => "date(created_at / 1000, 'unixepoch', 'localtime')",
            Self::Week => {
                "date(created_at / 1000, 'unixepoch', 'localtime', 'weekday 0', '-6 days')"
            }
            Self::Month => "strftime('%Y-%m', created_at / 1000, 'unixepoch', 'localtime')",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageRow {
    /// `YYYY-MM-DD` (day, or Monday of the week) or `YYYY-MM`.
    pub period: String,
    pub scans: i64,
    pub words: i64,
    pub credits: i64,
    pub cost_usd: f64,
}

pub struct Db {
    conn: Connection,
}

const COLUMNS: &str = "id, created_at, updated_at, title, input_text, requested_model, task_id, \
    state, error, returned_text, returned_version, prediction_short, fraction_ai, raw_response, \
    source_scan_id, submit_response, billed_words, credits, cost_usd, usd_per_credit";

impl Db {
    pub fn open(path: &Path) -> Result<Self, StorageError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self, StorageError> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self, StorageError> {
        let mut db = Self { conn };
        db.migrate()?;
        db.reconcile_usage()?;
        Ok(db)
    }

    pub fn schema_version(&self) -> Result<i64, StorageError> {
        Ok(self
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))?)
    }

    fn migrate(&mut self) -> Result<(), StorageError> {
        let version = self.schema_version()?;
        if version > SCHEMA_VERSION {
            return Err(StorageError::TooNew(version));
        }
        let tx = self.conn.transaction()?;
        if version < 1 {
            tx.execute_batch(
                "CREATE TABLE scans (
                    id TEXT PRIMARY KEY NOT NULL,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL,
                    title TEXT NOT NULL,
                    input_text TEXT NOT NULL,
                    requested_model TEXT NOT NULL,
                    task_id TEXT,
                    state TEXT NOT NULL,
                    error TEXT,
                    returned_text TEXT,
                    returned_version TEXT,
                    prediction_short TEXT,
                    fraction_ai REAL,
                    raw_response TEXT,
                    source_scan_id TEXT
                );
                CREATE INDEX scans_created_at ON scans(created_at DESC);
                CREATE INDEX scans_state ON scans(state);",
            )?;
        }
        if version < 2 {
            tx.execute_batch(
                "ALTER TABLE scans ADD COLUMN submit_response TEXT;
                ALTER TABLE scans ADD COLUMN billed_words INTEGER;
                ALTER TABLE scans ADD COLUMN credits INTEGER;
                ALTER TABLE scans ADD COLUMN cost_usd REAL;
                CREATE TABLE usage (
                    scan_id TEXT PRIMARY KEY NOT NULL,
                    created_at INTEGER NOT NULL,
                    model TEXT NOT NULL,
                    version TEXT,
                    words INTEGER NOT NULL,
                    credits INTEGER NOT NULL,
                    cost_usd REAL NOT NULL
                );
                CREATE INDEX usage_created_at ON usage(created_at);",
            )?;
            backfill_charges(&tx)?;
        }
        if version < 3 {
            tx.execute_batch(
                "ALTER TABLE scans ADD COLUMN usd_per_credit REAL;
                ALTER TABLE scans ADD COLUMN usage_recorded INTEGER NOT NULL DEFAULT 0;
                UPDATE scans SET usage_recorded = 1 WHERE id IN (SELECT scan_id FROM usage);",
            )?;
        }
        // Future migrations: `if version < 4 { ... }`.
        tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        tx.commit()?;
        Ok(())
    }

    pub fn upsert(&self, r: &ScanRecord) -> Result<(), StorageError> {
        upsert_on(&self.conn, r)
    }

    /// Writes a completed scan and its usage row in one transaction, so a crash can't leave a
    /// completed scan without its charge.
    pub fn save_completed(&self, r: &ScanRecord, u: &UsageRecord) -> Result<(), StorageError> {
        let tx = self.conn.unchecked_transaction()?;
        upsert_on(&tx, r)?;
        record_usage_on(&tx, u)?;
        tx.execute("UPDATE scans SET usage_recorded = 1 WHERE id = ?1", [&r.id])?;
        tx.commit()?;
        Ok(())
    }

    /// Adds usage for completed scans whose charge was never recorded (e.g. a crash between
    /// writes in an older version). Scans whose usage the user cleared stay cleared.
    fn reconcile_usage(&self) -> Result<(), StorageError> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT OR REPLACE INTO usage (scan_id, created_at, model, version, words, credits, cost_usd)
             SELECT id, created_at, requested_model, returned_version, billed_words, credits, cost_usd
             FROM scans WHERE state = 'completed' AND usage_recorded = 0 AND billed_words IS NOT NULL
               AND credits IS NOT NULL AND cost_usd IS NOT NULL",
            [],
        )?;
        tx.execute(
            "UPDATE scans SET usage_recorded = 1 WHERE state = 'completed' AND usage_recorded = 0
               AND billed_words IS NOT NULL AND credits IS NOT NULL AND cost_usd IS NOT NULL",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }
}

fn upsert_on(conn: &Connection, r: &ScanRecord) -> Result<(), StorageError> {
    conn.execute(
            &format!(
                "INSERT INTO scans ({COLUMNS}) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20)
                 ON CONFLICT(id) DO UPDATE SET
                    submit_response=excluded.submit_response, billed_words=excluded.billed_words,
                    credits=excluded.credits, cost_usd=excluded.cost_usd, usd_per_credit=excluded.usd_per_credit,
                    updated_at=excluded.updated_at, title=excluded.title,
                    input_text=excluded.input_text, requested_model=excluded.requested_model,
                    task_id=excluded.task_id, state=excluded.state, error=excluded.error,
                    returned_text=excluded.returned_text, returned_version=excluded.returned_version,
                    prediction_short=excluded.prediction_short, fraction_ai=excluded.fraction_ai,
                    raw_response=excluded.raw_response, source_scan_id=excluded.source_scan_id"
            ),
            params![
                r.id,
                r.created_at,
                r.updated_at,
                r.title,
                r.input_text,
                r.requested_model,
                r.task_id,
                r.state.as_str(),
                r.error,
                r.returned_text,
                r.returned_version,
                r.prediction_short,
                r.fraction_ai,
                r.raw_response,
                r.source_scan_id,
                r.submit_response,
                r.billed_words,
                r.credits,
                r.cost_usd,
                r.usd_per_credit,
            ],
        )?;
    Ok(())
}

impl Db {
    fn record_from_row(row: &Row<'_>) -> rusqlite::Result<Result<ScanRecord, StorageError>> {
        let state: String = row.get(7)?;
        let Some(state) = ScanState::parse(&state) else {
            return Ok(Err(StorageError::Corrupt(format!(
                "unknown state `{state}`"
            ))));
        };
        Ok(Ok(ScanRecord {
            id: row.get(0)?,
            created_at: row.get(1)?,
            updated_at: row.get(2)?,
            title: row.get(3)?,
            input_text: row.get(4)?,
            requested_model: row.get(5)?,
            task_id: row.get(6)?,
            state,
            error: row.get(8)?,
            returned_text: row.get(9)?,
            returned_version: row.get(10)?,
            prediction_short: row.get(11)?,
            fraction_ai: row.get(12)?,
            raw_response: row.get(13)?,
            source_scan_id: row.get(14)?,
            submit_response: row.get(15)?,
            billed_words: row.get(16)?,
            credits: row.get(17)?,
            cost_usd: row.get(18)?,
            usd_per_credit: row.get(19)?,
        }))
    }

    pub fn get(&self, id: &str) -> Result<Option<ScanRecord>, StorageError> {
        self.conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM scans WHERE id = ?1"),
                [id],
                Self::record_from_row,
            )
            .optional()?
            .transpose()
    }

    pub fn contains(&self, id: &str) -> Result<bool, StorageError> {
        Ok(self
            .conn
            .query_row("SELECT 1 FROM scans WHERE id = ?1", [id], |_| Ok(()))
            .optional()?
            .is_some())
    }

    /// Most recent first. `query` matches title, submitted text or returned text.
    pub fn list(
        &self,
        query: &str,
        limit: usize,
        saved: bool,
    ) -> Result<Vec<ScanSummary>, StorageError> {
        let pattern = format!(
            "%{}%",
            query
                .trim()
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, created_at, title, state, prediction_short, fraction_ai, substr(input_text, 1, 200)
             FROM scans
             WHERE ?1 = '%%' OR title LIKE ?1 ESCAPE '\\' OR input_text LIKE ?1 ESCAPE '\\'
                OR returned_text LIKE ?1 ESCAPE '\\'
             ORDER BY created_at DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![pattern, limit as i64], |row| {
            let state: String = row.get(3)?;
            let preview: String = row.get(6)?;
            Ok((
                ScanSummary {
                    id: row.get(0)?,
                    created_at: row.get(1)?,
                    title: row.get(2)?,
                    state: ScanState::Failed,
                    prediction_short: row.get(4)?,
                    fraction_ai: row.get(5)?,
                    preview: preview.split_whitespace().collect::<Vec<_>>().join(" "),
                    saved,
                },
                state,
            ))
        })?;
        rows.map(|r| {
            let (mut summary, state) = r?;
            summary.state = ScanState::parse(&state)
                .ok_or_else(|| StorageError::Corrupt(format!("unknown state `{state}`")))?;
            Ok(summary)
        })
        .collect()
    }

    /// Scans that were submitting or polling when the app last stopped.
    pub fn unfinished(&self) -> Result<Vec<ScanRecord>, StorageError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM scans WHERE state IN ('submitting', 'polling') ORDER BY created_at"
        ))?;
        let rows = stmt.query_map([], Self::record_from_row)?;
        rows.map(|r| r?).collect()
    }

    pub fn delete(&self, id: &str) -> Result<bool, StorageError> {
        Ok(self.conn.execute("DELETE FROM scans WHERE id = ?1", [id])? > 0)
    }

    pub fn delete_all(&self) -> Result<usize, StorageError> {
        let n = self.conn.execute("DELETE FROM scans", [])?;
        // Reclaim space so deleted documents don't linger in free pages.
        let _ = self.conn.execute_batch("VACUUM");
        Ok(n)
    }

    pub fn record_usage(&self, u: &UsageRecord) -> Result<(), StorageError> {
        record_usage_on(&self.conn, u)
    }
}

fn record_usage_on(conn: &Connection, u: &UsageRecord) -> Result<(), StorageError> {
    conn.execute(
            "INSERT OR REPLACE INTO usage (scan_id, created_at, model, version, words, credits, cost_usd)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![u.scan_id, u.created_at, u.model, u.version, u.words, u.credits, u.cost_usd],
        )?;
    Ok(())
}

impl Db {
    /// Most recent period first.
    pub fn usage_summary(&self, period: UsagePeriod) -> Result<Vec<UsageRow>, StorageError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {g} AS p, count(*), sum(words), sum(credits), sum(cost_usd)
             FROM usage GROUP BY p ORDER BY p DESC",
            g = period.group_expr()
        ))?;
        let rows = stmt.query_map([], |row| {
            Ok(UsageRow {
                period: row.get(0)?,
                scans: row.get(1)?,
                words: row.get(2)?,
                credits: row.get(3)?,
                cost_usd: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn clear_usage(&self) -> Result<usize, StorageError> {
        Ok(self.conn.execute("DELETE FROM usage", [])?)
    }

    pub fn count(&self) -> Result<usize, StorageError> {
        let n: i64 = self
            .conn
            .query_row("SELECT count(*) FROM scans", [], |r| r.get(0))?;
        Ok(n as usize)
    }
}

/// Records charges for scans completed before billing was tracked (schema v1).
fn backfill_charges(conn: &Connection) -> Result<(), StorageError> {
    let mut stmt = conn.prepare(
        "SELECT id, created_at, requested_model, raw_response FROM scans
         WHERE state = 'completed' AND raw_response IS NOT NULL AND billed_words IS NULL",
    )?;
    let rows: Vec<(String, i64, String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<Result<_, _>>()?;
    for (id, created_at, model, raw) in rows {
        let Ok(result) = DetectionResult::from_json_str(&raw) else {
            continue;
        };
        let charge = cost::charge_for_result(&result, &model, cost::DEFAULT_USD_PER_CREDIT);
        conn.execute(
            "UPDATE scans SET billed_words = ?2, credits = ?3, cost_usd = ?4 WHERE id = ?1",
            params![id, charge.words, charge.credits, charge.usd],
        )?;
        conn.execute(
            "INSERT OR REPLACE INTO usage (scan_id, created_at, model, version, words, credits, cost_usd)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![id, created_at, model, result.version, charge.words, charge.credits, charge.usd],
        )?;
    }
    Ok(())
}

pub struct Repository {
    disk: Option<Db>,
    session: Db,
    save_to_disk: bool,
}

/// Where a record ended up after [`Repository::save`].
#[derive(Debug)]
pub enum SaveOutcome {
    Disk,
    Session,
    /// The record was saved, but its usage row couldn't be written to disk and is kept in
    /// memory for this session.
    UsageSessionAfterError(StorageError),
    /// Disk write failed; the record was kept in memory instead.
    SessionAfterError(StorageError),
}

impl Repository {
    pub fn new(disk: Option<Db>, save_to_disk: bool) -> Result<Self, StorageError> {
        Ok(Self {
            disk,
            session: Db::open_in_memory()?,
            save_to_disk,
        })
    }

    pub fn has_disk(&self) -> bool {
        self.disk.is_some()
    }

    pub fn set_save_to_disk(&mut self, enabled: bool) {
        self.save_to_disk = enabled;
    }

    /// The on-disk database for this record, if it belongs there: records stay in the store they
    /// started in, and new records follow the current setting.
    fn disk_for(&self, id: &str) -> Result<Option<&Db>, StorageError> {
        if self.session.contains(id)? {
            return Ok(None);
        }
        Ok(match &self.disk {
            Some(d) if self.save_to_disk || d.contains(id).unwrap_or(false) => Some(d),
            _ => None,
        })
    }

    pub fn save(&self, r: &ScanRecord) -> Result<SaveOutcome, StorageError> {
        let Some(disk) = self.disk_for(&r.id)? else {
            self.session.upsert(r)?;
            return Ok(SaveOutcome::Session);
        };
        match disk.upsert(r) {
            Ok(()) => Ok(SaveOutcome::Disk),
            Err(e) => {
                self.session.upsert(r)?;
                let _ = disk.delete(&r.id);
                Ok(SaveOutcome::SessionAfterError(e))
            }
        }
    }

    /// Saves a completed scan together with its usage row. When both live in the same database
    /// this is one transaction; a session-only scan records its usage first.
    pub fn save_completed(
        &self,
        r: &ScanRecord,
        u: &UsageRecord,
    ) -> Result<SaveOutcome, StorageError> {
        match self.disk_for(&r.id)? {
            Some(disk) => match disk.save_completed(r, u) {
                Ok(()) => Ok(SaveOutcome::Disk),
                Err(e) => {
                    let _ = disk.delete(&r.id);
                    self.session.save_completed(r, u)?;
                    Ok(SaveOutcome::SessionAfterError(e))
                }
            },
            None if self.disk.is_none() => {
                self.session.save_completed(r, u)?;
                Ok(SaveOutcome::Session)
            }
            None => {
                // The result comes first: a usage failure must not lose it.
                self.session.upsert(r)?;
                match self.usage_db().record_usage(u) {
                    Ok(()) => Ok(SaveOutcome::Session),
                    Err(e) => {
                        self.session.record_usage(u)?;
                        Ok(SaveOutcome::UsageSessionAfterError(e))
                    }
                }
            }
        }
    }

    pub fn get(&self, id: &str) -> Result<Option<ScanRecord>, StorageError> {
        if let Some(r) = self.session.get(id)? {
            return Ok(Some(r));
        }
        match &self.disk {
            Some(d) => d.get(id),
            None => Ok(None),
        }
    }

    pub fn list(&self, query: &str, limit: usize) -> Result<Vec<ScanSummary>, StorageError> {
        let mut all = self.session.list(query, limit, false)?;
        if let Some(d) = &self.disk {
            all.extend(d.list(query, limit, true)?);
        }
        all.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.id.cmp(&a.id))
        });
        all.truncate(limit);
        Ok(all)
    }

    pub fn unfinished(&self) -> Result<Vec<ScanRecord>, StorageError> {
        let mut all = self.session.unfinished()?;
        if let Some(d) = &self.disk {
            all.extend(d.unfinished()?);
        }
        Ok(all)
    }

    pub fn delete(&self, id: &str) -> Result<bool, StorageError> {
        let mut deleted = self.session.delete(id)?;
        if let Some(d) = &self.disk {
            deleted |= d.delete(id)?;
        }
        Ok(deleted)
    }

    pub fn delete_all(&self) -> Result<usize, StorageError> {
        let mut n = self.session.delete_all()?;
        if let Some(d) = &self.disk {
            n += d.delete_all()?;
        }
        Ok(n)
    }

    /// Usage goes to disk whenever a disk database exists: it holds no document text.
    fn usage_db(&self) -> &Db {
        self.disk.as_ref().unwrap_or(&self.session)
    }

    pub fn record_usage(&self, u: &UsageRecord) -> Result<(), StorageError> {
        self.usage_db().record_usage(u)
    }

    pub fn usage_summary(&self, period: UsagePeriod) -> Result<Vec<UsageRow>, StorageError> {
        let mut rows = self.session.usage_summary(period)?;
        if let Some(d) = &self.disk {
            for row in d.usage_summary(period)? {
                match rows.iter_mut().find(|r| r.period == row.period) {
                    Some(r) => {
                        r.scans += row.scans;
                        r.words += row.words;
                        r.credits += row.credits;
                        r.cost_usd += row.cost_usd;
                    }
                    None => rows.push(row),
                }
            }
        }
        rows.sort_by(|a, b| b.period.cmp(&a.period));
        Ok(rows)
    }

    pub fn clear_usage(&self) -> Result<usize, StorageError> {
        let mut n = self.session.clear_usage()?;
        if let Some(d) = &self.disk {
            n += d.clear_usage()?;
        }
        Ok(n)
    }

    pub fn saved_count(&self) -> usize {
        self.disk.as_ref().and_then(|d| d.count().ok()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn record(id: &str, created_at: i64, state: ScanState) -> ScanRecord {
        ScanRecord {
            id: id.to_owned(),
            created_at,
            updated_at: created_at,
            title: format!("Title {id}"),
            input_text: format!("Input text for {id}\nsecond line"),
            requested_model: "default".to_owned(),
            task_id: None,
            state,
            error: None,
            returned_text: None,
            returned_version: None,
            prediction_short: None,
            fraction_ai: None,
            raw_response: None,
            source_scan_id: None,
            submit_response: None,
            billed_words: None,
            credits: None,
            cost_usd: None,
            usd_per_credit: None,
        }
    }

    #[test]
    fn migrates_fresh_database() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn reopening_is_idempotent_and_keeps_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/history.sqlite3");
        Db::open(&path)
            .unwrap()
            .upsert(&record("a", 1, ScanState::Completed))
            .unwrap();
        let db = Db::open(&path).unwrap();
        assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
        assert!(db.get("a").unwrap().is_some());
    }

    #[test]
    fn refuses_newer_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("h.sqlite3");
        {
            let conn = Connection::open(&path).unwrap();
            conn.pragma_update(None, "user_version", SCHEMA_VERSION + 1)
                .unwrap();
        }
        assert!(matches!(Db::open(&path), Err(StorageError::TooNew(v)) if v == SCHEMA_VERSION + 1));
    }

    #[test]
    fn round_trips_every_field() {
        let db = Db::open_in_memory().unwrap();
        let mut r = record("x", 5, ScanState::Completed);
        r.task_id = Some("task-1".into());
        r.error = Some("none".into());
        r.returned_text = Some("Returned “text” 😀".into());
        r.returned_version = Some("4.0".into());
        r.prediction_short = Some("AI".into());
        r.fraction_ai = Some(0.75);
        r.raw_response = Some(r#"{"stage":"STAGE_SUCCESS"}"#.into());
        r.source_scan_id = Some("w".into());
        r.submit_response = Some(r#"{"task_id":"task-1"}"#.into());
        r.billed_words = Some(172);
        r.credits = Some(2);
        r.cost_usd = Some(0.1);
        db.upsert(&r).unwrap();
        assert_eq!(db.get("x").unwrap().unwrap(), r);

        r.state = ScanState::Failed;
        r.updated_at = 9;
        db.upsert(&r).unwrap();
        assert_eq!(db.get("x").unwrap().unwrap(), r);
        assert_eq!(db.count().unwrap(), 1);
    }

    #[test]
    fn lists_searches_and_deletes() {
        let db = Db::open_in_memory().unwrap();
        db.upsert(&record("old", 1, ScanState::Completed)).unwrap();
        db.upsert(&record("new", 2, ScanState::Polling)).unwrap();
        let mut odd = record("pct", 3, ScanState::Failed);
        odd.title = "100% sure_thing".into();
        db.upsert(&odd).unwrap();

        let all = db.list("", 10, true).unwrap();
        assert_eq!(
            all.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            ["pct", "new", "old"]
        );
        assert_eq!(all[1].preview, "Input text for new second line");
        assert_eq!(db.list("for old", 10, true).unwrap().len(), 1);
        assert_eq!(db.list("100%", 10, true).unwrap().len(), 1);
        assert_eq!(db.list("0%", 10, true).unwrap().len(), 1);
        assert_eq!(db.list("e_t", 10, true).unwrap().len(), 1);
        assert_eq!(db.list("", 1, true).unwrap().len(), 1);

        assert_eq!(db.unfinished().unwrap().len(), 1);
        assert!(db.delete("old").unwrap());
        assert!(!db.delete("old").unwrap());
        assert_eq!(db.delete_all().unwrap(), 2);
        assert_eq!(db.count().unwrap(), 0);
    }

    #[test]
    fn repository_respects_history_setting() {
        let disk = Db::open_in_memory().unwrap();
        let mut repo = Repository::new(Some(disk), true).unwrap();
        assert!(matches!(
            repo.save(&record("saved", 1, ScanState::Polling)).unwrap(),
            SaveOutcome::Disk
        ));

        repo.set_save_to_disk(false);
        assert!(matches!(
            repo.save(&record("temp", 2, ScanState::Polling)).unwrap(),
            SaveOutcome::Session
        ));
        // An existing on-disk record keeps being updated on disk.
        assert!(matches!(
            repo.save(&record("saved", 1, ScanState::Completed))
                .unwrap(),
            SaveOutcome::Disk
        ));

        repo.set_save_to_disk(true);
        // A session-only record stays in the session.
        assert!(matches!(
            repo.save(&record("temp", 2, ScanState::Completed)).unwrap(),
            SaveOutcome::Session
        ));

        let list = repo.list("", 10).unwrap();
        assert_eq!(list.len(), 2);
        assert!(!list[0].saved && list[1].saved);
        assert_eq!(repo.saved_count(), 1);
        assert_eq!(
            repo.get("temp").unwrap().unwrap().state,
            ScanState::Completed
        );
        assert!(repo.delete("temp").unwrap());
        assert_eq!(repo.delete_all().unwrap(), 1);
    }

    #[test]
    fn repository_without_disk_is_session_only() {
        let repo = Repository::new(None, true).unwrap();
        assert!(matches!(
            repo.save(&record("a", 1, ScanState::Submitting)).unwrap(),
            SaveOutcome::Session
        ));
        assert_eq!(repo.unfinished().unwrap().len(), 1);
    }

    #[test]
    fn migrates_v1_database_and_backfills_charges() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v1.sqlite3");
        {
            // A schema-1 database as written by the first release.
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE scans (id TEXT PRIMARY KEY NOT NULL, created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL, title TEXT NOT NULL, input_text TEXT NOT NULL,
                    requested_model TEXT NOT NULL, task_id TEXT, state TEXT NOT NULL, error TEXT,
                    returned_text TEXT, returned_version TEXT, prediction_short TEXT,
                    fraction_ai REAL, raw_response TEXT, source_scan_id TEXT);
                 CREATE INDEX scans_created_at ON scans(created_at DESC);
                 CREATE INDEX scans_state ON scans(state);
                 PRAGMA user_version = 1;",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO scans (id, created_at, updated_at, title, input_text, requested_model, state, raw_response)
                 VALUES ('old', 5, 5, 't', 'x', 'pangram-4', 'completed', ?1)",
                [r#"{"stage":"STAGE_SUCCESS","text":"x","version":"4.0","windows":[{"word_count":174}]}"#],
            )
            .unwrap();
        }
        let db = Db::open(&path).unwrap();
        assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
        let r = db.get("old").unwrap().unwrap();
        assert_eq!((r.billed_words, r.credits), (Some(174), Some(2)));
        assert!((r.cost_usd.unwrap() - 0.10).abs() < 1e-9);
        let usage = db.usage_summary(UsagePeriod::Month).unwrap();
        assert_eq!(usage.len(), 1);
        assert_eq!(
            (usage[0].scans, usage[0].words, usage[0].credits),
            (1, 174, 2)
        );
    }

    fn usage(id: &str, created_at: i64, words: i64) -> UsageRecord {
        let credits = cost::credits(words, 100);
        UsageRecord {
            scan_id: id.into(),
            created_at,
            model: "default".into(),
            version: Some("4.0".into()),
            words,
            credits,
            cost_usd: credits as f64 * 0.05,
        }
    }

    #[test]
    fn summarises_usage_by_period_and_survives_scan_deletion() {
        // Noon UTC on Wed 2026-10-14, Fri 2026-10-16 and Tue 2026-11-10: any time zone keeps the
        // first two in the same week and the third in a later month.
        const DAY: i64 = 86_400_000;
        let wed = 1_791_979_200_000;
        let disk = Db::open_in_memory().unwrap();
        let repo = Repository::new(Some(disk), false).unwrap();
        repo.record_usage(&usage("a", wed, 150)).unwrap();
        repo.record_usage(&usage("b", wed + 3_600_000, 50)).unwrap();
        repo.record_usage(&usage("c", wed + 2 * DAY, 101)).unwrap();
        repo.record_usage(&usage("d", wed + 27 * DAY, 10)).unwrap();
        repo.record_usage(&usage("d", wed + 27 * DAY, 10)).unwrap(); // idempotent per scan

        let days = repo.usage_summary(UsagePeriod::Day).unwrap();
        assert_eq!(days.iter().map(|r| r.scans).collect::<Vec<_>>(), [1, 1, 2]);
        assert_eq!(days[2].credits, 3);
        let weeks = repo.usage_summary(UsagePeriod::Week).unwrap();
        assert_eq!(weeks.iter().map(|r| r.scans).collect::<Vec<_>>(), [1, 3]);
        let months = repo.usage_summary(UsagePeriod::Month).unwrap();
        assert_eq!(months.len(), 2);
        assert_eq!(
            (months[1].scans, months[1].words, months[1].credits),
            (3, 301, 5)
        );
        assert!((months[1].cost_usd - 0.25).abs() < 1e-9);

        repo.delete_all().unwrap();
        assert_eq!(repo.usage_summary(UsagePeriod::Month).unwrap().len(), 2);
        assert_eq!(repo.clear_usage().unwrap(), 4);
        assert!(repo.usage_summary(UsagePeriod::Day).unwrap().is_empty());
    }

    fn completed(id: &str, created_at: i64) -> ScanRecord {
        let mut r = record(id, created_at, ScanState::Completed);
        r.billed_words = Some(150);
        r.credits = Some(2);
        r.cost_usd = Some(0.1);
        r.usd_per_credit = Some(0.05);
        r
    }

    #[test]
    fn completed_scans_and_usage_are_saved_together() {
        let repo = Repository::new(Some(Db::open_in_memory().unwrap()), true).unwrap();
        let r = completed("a", 1_791_979_200_000);
        let u = usage("a", r.created_at, 150);
        assert!(matches!(
            repo.save_completed(&r, &u).unwrap(),
            SaveOutcome::Disk
        ));
        assert_eq!(repo.get("a").unwrap().unwrap(), r);
        assert_eq!(repo.usage_summary(UsagePeriod::Month).unwrap()[0].scans, 1);

        // Session-only scans still record usage (on disk, without text).
        let mut repo = repo;
        repo.set_save_to_disk(false);
        let r = completed("b", 1_791_979_200_000);
        assert!(matches!(
            repo.save_completed(&r, &usage("b", r.created_at, 150))
                .unwrap(),
            SaveOutcome::Session
        ));
        assert_eq!(repo.usage_summary(UsagePeriod::Month).unwrap()[0].scans, 2);
    }

    #[test]
    fn reopening_repairs_missing_usage_but_respects_clearing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("h.sqlite3");
        // A completed scan whose usage write never happened (e.g. a crash between writes).
        Db::open(&path)
            .unwrap()
            .upsert(&completed("a", 1_791_979_200_000))
            .unwrap();
        let db = Db::open(&path).unwrap();
        let rows = db.usage_summary(UsagePeriod::Month).unwrap();
        assert_eq!((rows[0].scans, rows[0].words, rows[0].credits), (1, 150, 2));

        db.clear_usage().unwrap();
        drop(db);
        let db = Db::open(&path).unwrap();
        assert!(db.usage_summary(UsagePeriod::Month).unwrap().is_empty());
    }

    #[test]
    fn session_results_survive_a_failed_usage_write() {
        let disk = Db::open_in_memory().unwrap();
        let mut repo = Repository::new(Some(disk), false).unwrap();
        repo.set_save_to_disk(false);
        let mut r = completed("s", 1_791_979_200_000);
        r.state = ScanState::Polling;
        repo.save(&r).unwrap();
        // Make every disk write fail.
        repo.disk
            .as_ref()
            .unwrap()
            .conn
            .pragma_update(None, "query_only", true)
            .unwrap();

        let r = completed("s", 1_791_979_200_000);
        let outcome = repo
            .save_completed(&r, &usage("s", r.created_at, 150))
            .unwrap();
        assert!(
            matches!(outcome, SaveOutcome::UsageSessionAfterError(_)),
            "{outcome:?}"
        );
        assert_eq!(repo.get("s").unwrap().unwrap().state, ScanState::Completed);
        assert_eq!(repo.usage_summary(UsagePeriod::Month).unwrap()[0].scans, 1);
    }
}
