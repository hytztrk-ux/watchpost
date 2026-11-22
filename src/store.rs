//! Check history in an embedded SQLite database.
//!
//! Three tables:
//!
//! * `checks`    – every check result (pruned after `retention_days`),
//! * `hourly`    – per-monitor hourly counters used for uptime percentages
//!   (kept at least 31 days so 30-day uptime always works),
//! * `incidents` – down periods, open while `ended_at` is NULL.
//!
//! The connection is shared behind a mutex; async code goes through
//! [`Store::run`], which moves the work to the blocking thread pool.

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::monitor::Status;

const HOUR: i64 = 3600;
const DAY: i64 = 86_400;
const SCHEMA_VERSION: i64 = 1;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("cannot create database directory {path}: {source}")]
    Dir {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "database was created by a newer watchpost (schema {found}, supported {SCHEMA_VERSION})"
    )]
    Schema { found: i64 },
    #[error("database task failed: {0}")]
    Join(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// One stored check.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CheckRow {
    pub ts: i64,
    pub status: Status,
    pub latency_ms: Option<i64>,
    pub message: String,
}

/// A down period.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Incident {
    pub id: i64,
    pub monitor: String,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub message: String,
}

impl Incident {
    pub fn duration_secs(&self, now: i64) -> i64 {
        self.ended_at.unwrap_or(now) - self.started_at
    }
}

/// Uptime ratios in `0.0..=1.0`; `None` when there is no data in the window.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize)]
pub struct Uptime {
    pub h24: Option<f64>,
    pub d7: Option<f64>,
    pub d30: Option<f64>,
}

#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
}

impl Store {
    /// Open (or create) the database file.
    pub fn open(path: &Path) -> Result<Store> {
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() && !dir.exists() {
                std::fs::create_dir_all(dir).map_err(|source| StoreError::Dir {
                    path: dir.display().to_string(),
                    source,
                })?;
            }
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        Self::init(conn)
    }

    /// Temporary in-memory database (tests, `watchpost check`).
    pub fn open_in_memory() -> Result<Store> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Store> {
        // Keep the page cache small: the working set is tiny.
        conn.pragma_update(None, "cache_size", -1024)?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version > SCHEMA_VERSION {
            return Err(StoreError::Schema { found: version });
        }
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS checks (
                 id         INTEGER PRIMARY KEY,
                 monitor    TEXT    NOT NULL,
                 ts         INTEGER NOT NULL,
                 status     INTEGER NOT NULL,
                 latency_ms INTEGER,
                 message    TEXT    NOT NULL
             );
             CREATE INDEX IF NOT EXISTS checks_monitor_ts ON checks (monitor, ts);
             CREATE TABLE IF NOT EXISTS hourly (
                 monitor TEXT    NOT NULL,
                 hour    INTEGER NOT NULL,
                 total   INTEGER NOT NULL,
                 up      INTEGER NOT NULL,
                 PRIMARY KEY (monitor, hour)
             ) WITHOUT ROWID;
             CREATE TABLE IF NOT EXISTS incidents (
                 id         INTEGER PRIMARY KEY,
                 monitor    TEXT    NOT NULL,
                 started_at INTEGER NOT NULL,
                 ended_at   INTEGER,
                 message    TEXT    NOT NULL
             );
             CREATE INDEX IF NOT EXISTS incidents_started ON incidents (started_at);",
        )?;
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        Ok(Store {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        // A panic while holding the lock cannot leave SQLite in a broken
        // state, so a poisoned mutex is safe to reuse.
        self.conn.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Run a store operation on the blocking thread pool.
    pub async fn run<T, F>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Store) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let store = self.clone();
        tokio::task::spawn_blocking(move || f(&store))
            .await
            .map_err(|e| StoreError::Join(e.to_string()))?
    }

    /// Record one check and update the hourly counters.
    pub fn record_check(
        &self,
        monitor: &str,
        ts: i64,
        status: Status,
        latency_ms: Option<i64>,
        message: &str,
    ) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO checks (monitor, ts, status, latency_ms, message) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![monitor, ts, status.as_i64(), latency_ms, message],
        )?;
        let up = i64::from(status != Status::Down);
        tx.execute(
            "INSERT INTO hourly (monitor, hour, total, up) VALUES (?1, ?2, 1, ?3)
             ON CONFLICT (monitor, hour) DO UPDATE SET total = total + 1, up = up + excluded.up",
            params![monitor, ts.div_euclid(HOUR) * HOUR, up],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Most recent check of a monitor.
    pub fn last_check(&self, monitor: &str) -> Result<Option<CheckRow>> {
        let conn = self.conn();
        let row = conn
            .query_row(
                "SELECT ts, status, latency_ms, message FROM checks
                 WHERE monitor = ?1 ORDER BY ts DESC, id DESC LIMIT 1",
                params![monitor],
                map_check,
            )
            .optional()?;
        Ok(row)
    }

    /// The last `limit` checks of a monitor, oldest first.
    pub fn recent_checks(&self, monitor: &str, limit: usize) -> Result<Vec<CheckRow>> {
        let conn = self.conn();
        let mut stmt = conn.prepare_cached(
            "SELECT ts, status, latency_ms, message FROM checks
             WHERE monitor = ?1 ORDER BY ts DESC, id DESC LIMIT ?2",
        )?;
        let mut rows = stmt
            .query_map(params![monitor, limit as i64], map_check)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows.reverse();
        Ok(rows)
    }

    /// Uptime over the last 24 hours, 7 days and 30 days.
    ///
    /// Computed from hourly buckets: a window of N hours covers the current
    /// (partial) hour plus the N-1 hours before it. Uptime is the share of
    /// checks that were not down.
    pub fn uptime(&self, monitor: &str, now: i64) -> Result<Uptime> {
        let current = now.div_euclid(HOUR) * HOUR;
        let start = |hours: i64| current - (hours - 1) * HOUR;
        let conn = self.conn();
        let mut stmt = conn.prepare_cached(
            "SELECT
                 SUM(CASE WHEN hour >= ?2 THEN total ELSE 0 END),
                 SUM(CASE WHEN hour >= ?2 THEN up    ELSE 0 END),
                 SUM(CASE WHEN hour >= ?3 THEN total ELSE 0 END),
                 SUM(CASE WHEN hour >= ?3 THEN up    ELSE 0 END),
                 SUM(total),
                 SUM(up)
             FROM hourly WHERE monitor = ?1 AND hour >= ?4",
        )?;
        let sums: [Option<i64>; 6] = stmt.query_row(
            params![monitor, start(24), start(24 * 7), start(24 * 30)],
            |r| {
                Ok([
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ])
            },
        )?;
        let ratio = |total: Option<i64>, up: Option<i64>| match (total, up) {
            (Some(t), Some(u)) if t > 0 => Some(u as f64 / t as f64),
            _ => None,
        };
        Ok(Uptime {
            h24: ratio(sums[0], sums[1]),
            d7: ratio(sums[2], sums[3]),
            d30: ratio(sums[4], sums[5]),
        })
    }

    /// Open a down incident.
    pub fn open_incident(&self, monitor: &str, started_at: i64, message: &str) -> Result<i64> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO incidents (monitor, started_at, message) VALUES (?1, ?2, ?3)",
            params![monitor, started_at, message],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Close all open incidents of a monitor. Returns how many were closed.
    pub fn close_incidents(&self, monitor: &str, ended_at: i64) -> Result<usize> {
        let conn = self.conn();
        Ok(conn.execute(
            "UPDATE incidents SET ended_at = ?2 WHERE monitor = ?1 AND ended_at IS NULL",
            params![monitor, ended_at],
        )?)
    }

    /// The open incident of a monitor, if any.
    pub fn open_incident_for(&self, monitor: &str) -> Result<Option<Incident>> {
        let conn = self.conn();
        Ok(conn
            .query_row(
                "SELECT id, monitor, started_at, ended_at, message FROM incidents
                 WHERE monitor = ?1 AND ended_at IS NULL ORDER BY started_at DESC LIMIT 1",
                params![monitor],
                map_incident,
            )
            .optional()?)
    }

    /// Close open incidents of monitors that no longer exist in the config.
    pub fn close_orphaned_incidents(&self, known: &[String], now: i64) -> Result<usize> {
        let conn = self.conn();
        let mut stmt =
            conn.prepare("SELECT DISTINCT monitor FROM incidents WHERE ended_at IS NULL")?;
        let open: Vec<String> = stmt
            .query_map([], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        let mut closed = 0;
        for name in open.iter().filter(|n| !known.contains(n)) {
            closed += conn.execute(
                "UPDATE incidents SET ended_at = ?2 WHERE monitor = ?1 AND ended_at IS NULL",
                params![name, now],
            )?;
        }
        Ok(closed)
    }

    /// Most recent incidents across all monitors, newest first.
    pub fn recent_incidents(&self, limit: usize) -> Result<Vec<Incident>> {
        let conn = self.conn();
        let mut stmt = conn.prepare_cached(
            "SELECT id, monitor, started_at, ended_at, message FROM incidents
             ORDER BY started_at DESC, id DESC LIMIT ?1",
        )?;
        let rows = stmt
            .query_map(params![limit as i64], map_incident)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Delete data older than the retention period. Returns deleted checks.
    pub fn prune(&self, now: i64, retention_days: u32) -> Result<usize> {
        let cutoff = now - i64::from(retention_days) * DAY;
        let hourly_cutoff = now - i64::from(retention_days.max(31)) * DAY;
        let conn = self.conn();
        let checks = conn.execute("DELETE FROM checks WHERE ts < ?1", params![cutoff])?;
        conn.execute("DELETE FROM hourly WHERE hour < ?1", params![hourly_cutoff])?;
        conn.execute(
            "DELETE FROM incidents WHERE ended_at IS NOT NULL AND ended_at < ?1",
            params![cutoff],
        )?;
        Ok(checks)
    }

    /// Flush the write-ahead log into the main database file.
    pub fn checkpoint(&self) -> Result<()> {
        let conn = self.conn();
        conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
            .optional()?;
        Ok(())
    }
}

fn map_check(r: &rusqlite::Row<'_>) -> rusqlite::Result<CheckRow> {
    Ok(CheckRow {
        ts: r.get(0)?,
        status: Status::from_i64(r.get(1)?),
        latency_ms: r.get(2)?,
        message: r.get(3)?,
    })
}

fn map_incident(r: &rusqlite::Row<'_>) -> rusqlite::Result<Incident> {
    Ok(Incident {
        id: r.get(0)?,
        monitor: r.get(1)?,
        started_at: r.get(2)?,
        ended_at: r.get(3)?,
        message: r.get(4)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // An hour-aligned "now" plus 30 minutes, to make bucket maths obvious.
    const NOW: i64 = 1_800_000_000 - 1_800_000_000 % HOUR + 1800;

    fn store() -> Store {
        Store::open_in_memory().unwrap()
    }

    fn approx(a: Option<f64>, b: f64) {
        let a = a.expect("expected a value");
        assert!((a - b).abs() < 1e-9, "{a} != {b}");
    }

    #[test]
    fn empty_store_has_no_uptime() {
        let s = store();
        assert_eq!(s.uptime("x", NOW).unwrap(), Uptime::default());
        assert_eq!(s.last_check("x").unwrap(), None);
        assert!(s.recent_checks("x", 10).unwrap().is_empty());
    }

    #[test]
    fn records_and_reads_checks() {
        let s = store();
        s.record_check("web", NOW - 120, Status::Up, Some(12), "HTTP 200")
            .unwrap();
        s.record_check("web", NOW - 60, Status::Down, None, "refused")
            .unwrap();
        s.record_check("web", NOW, Status::Up, Some(15), "HTTP 200")
            .unwrap();
        s.record_check("other", NOW, Status::Down, None, "x")
            .unwrap();

        let last = s.last_check("web").unwrap().unwrap();
        assert_eq!(last.ts, NOW);
        assert_eq!(last.latency_ms, Some(15));

        let recent = s.recent_checks("web", 2).unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].status, Status::Down, "oldest first");
        assert_eq!(recent[1].status, Status::Up);
    }

    #[test]
    fn uptime_counts_warn_as_up() {
        let s = store();
        for i in 0..8 {
            s.record_check("m", NOW - i * 60, Status::Up, Some(1), "")
                .unwrap();
        }
        s.record_check("m", NOW - 600, Status::Warn, Some(1), "")
            .unwrap();
        s.record_check("m", NOW - 660, Status::Down, None, "")
            .unwrap();
        let u = s.uptime("m", NOW).unwrap();
        approx(u.h24, 0.9);
        approx(u.d7, 0.9);
        approx(u.d30, 0.9);
    }

    #[test]
    fn uptime_windows_are_separate() {
        let s = store();
        // Last 24h: 3 up, 1 down -> 75%.
        for i in 0..3 {
            s.record_check("m", NOW - i * HOUR, Status::Up, None, "")
                .unwrap();
        }
        s.record_check("m", NOW - 5 * HOUR, Status::Down, None, "")
            .unwrap();
        // 3 days ago: 4 down -> 7d = 3 / 8.
        for i in 0..4 {
            s.record_check("m", NOW - 3 * DAY + i, Status::Down, None, "")
                .unwrap();
        }
        // 20 days ago: 8 up -> 30d = 11 / 16.
        for i in 0..8 {
            s.record_check("m", NOW - 20 * DAY + i, Status::Up, None, "")
                .unwrap();
        }
        // 40 days ago: ignored everywhere.
        s.record_check("m", NOW - 40 * DAY, Status::Down, None, "")
            .unwrap();

        let u = s.uptime("m", NOW).unwrap();
        approx(u.h24, 0.75);
        approx(u.d7, 3.0 / 8.0);
        approx(u.d30, 11.0 / 16.0);
    }

    #[test]
    fn uptime_window_boundaries_follow_hour_buckets() {
        let s = store();
        let current_hour = NOW - NOW % HOUR;
        // Oldest bucket still inside the 24h window...
        s.record_check("m", current_hour - 23 * HOUR, Status::Down, None, "")
            .unwrap();
        // ...and the newest bucket just outside of it.
        s.record_check("m", current_hour - 24 * HOUR + 3599, Status::Up, None, "")
            .unwrap();
        let u = s.uptime("m", NOW).unwrap();
        approx(u.h24, 0.0);
        approx(u.d7, 0.5);
    }

    #[test]
    fn uptime_is_per_monitor() {
        let s = store();
        s.record_check("a", NOW, Status::Up, None, "").unwrap();
        s.record_check("b", NOW, Status::Down, None, "").unwrap();
        approx(s.uptime("a", NOW).unwrap().h24, 1.0);
        approx(s.uptime("b", NOW).unwrap().h24, 0.0);
    }

    #[test]
    fn incidents_open_and_close() {
        let s = store();
        assert!(s.open_incident_for("web").unwrap().is_none());
        s.open_incident("web", NOW - 300, "connection refused")
            .unwrap();
        let open = s.open_incident_for("web").unwrap().unwrap();
        assert_eq!(open.started_at, NOW - 300);
        assert_eq!(open.duration_secs(NOW), 300);

        assert_eq!(s.close_incidents("web", NOW).unwrap(), 1);
        assert!(s.open_incident_for("web").unwrap().is_none());
        assert_eq!(s.close_incidents("web", NOW).unwrap(), 0);

        let recent = s.recent_incidents(10).unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].ended_at, Some(NOW));
        assert_eq!(recent[0].duration_secs(NOW + 999), 300);
    }

    #[test]
    fn recent_incidents_newest_first_and_limited() {
        let s = store();
        for i in 0..5 {
            s.open_incident("m", NOW + i, &format!("#{i}")).unwrap();
        }
        let r = s.recent_incidents(3).unwrap();
        assert_eq!(
            r.iter().map(|i| i.message.as_str()).collect::<Vec<_>>(),
            ["#4", "#3", "#2"]
        );
    }

    #[test]
    fn orphaned_incidents_are_closed() {
        let s = store();
        s.open_incident("kept", NOW, "").unwrap();
        s.open_incident("removed", NOW, "").unwrap();
        let closed = s
            .close_orphaned_incidents(&["kept".to_string()], NOW + 10)
            .unwrap();
        assert_eq!(closed, 1);
        assert!(s.open_incident_for("kept").unwrap().is_some());
        assert!(s.open_incident_for("removed").unwrap().is_none());
    }

    #[test]
    fn prune_respects_retention() {
        let s = store();
        s.record_check("m", NOW - 10 * DAY, Status::Up, None, "old")
            .unwrap();
        s.record_check("m", NOW - 20 * DAY, Status::Up, None, "older")
            .unwrap();
        s.record_check("m", NOW, Status::Down, None, "new").unwrap();
        let id = s.open_incident("m", NOW - 9 * DAY, "").unwrap();
        s.close_incidents("m", NOW - 8 * DAY).unwrap();
        s.open_incident("m", NOW - 9 * DAY, "still open").unwrap();

        let deleted = s.prune(NOW, 7).unwrap();
        assert_eq!(deleted, 2);
        let rows = s.recent_checks("m", 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].message, "new");

        let incidents = s.recent_incidents(10).unwrap();
        assert_eq!(
            incidents.len(),
            1,
            "closed old incident pruned, open one kept"
        );
        assert_ne!(incidents[0].id, id);

        // Hourly counters survive for 31 days so 30-day uptime still works.
        approx(s.uptime("m", NOW).unwrap().d30, 2.0 / 3.0);
    }

    #[test]
    fn file_database_persists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/dir/watchpost.db");
        {
            let s = Store::open(&path).unwrap();
            s.record_check("m", NOW, Status::Up, Some(3), "ok").unwrap();
            s.checkpoint().unwrap();
        }
        let s = Store::open(&path).unwrap();
        assert_eq!(s.last_check("m").unwrap().unwrap().message, "ok");
    }

    #[tokio::test]
    async fn async_run_wrapper() {
        let s = store();
        s.run(|s| s.record_check("m", NOW, Status::Up, Some(1), "ok"))
            .await
            .unwrap();
        let n = s.run(|s| s.recent_checks("m", 5)).await.unwrap().len();
        assert_eq!(n, 1);
    }
}
