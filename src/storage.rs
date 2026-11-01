use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    fs::File,
    fs::OpenOptions,
    path::{Component, Path, PathBuf},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use crate::{
    contracts::{
        GraphEdge, IndexSnapshot, IndexSyncStatus, LinkKind, MatchKind, SearchCandidate,
        SessionEventRecord, SourceRef, StoredDocument, TextChunk, VectorEmbedding,
    },
    error::{VrgError, VrgResult},
};

const SCHEMA_VERSION: i64 = 1;
const MAX_QUERY_RESULTS: usize = 1000;
const SYNC_LOCK_FILE: &str = "vrg-sync.lock";

#[derive(Clone)]
pub struct DerivedStore {
    connection: std::sync::Arc<Mutex<Connection>>,
    data_dir: PathBuf,
}

#[derive(Clone, Debug)]
pub struct StoredVector {
    pub chunk: TextChunk,
    pub title: String,
    pub model_id: String,
    pub values: Vec<f32>,
}

#[derive(Clone, Debug, Default)]
pub struct RecoveryReport {
    pub committed: usize,
    pub aborted: usize,
}

impl DerivedStore {
    /// Opens only the configured derived-state location and initializes its schema.
    /// The caller must first prove that this directory is outside every source root.
    pub fn open(data_dir: impl AsRef<Path>) -> VrgResult<Self> {
        let data_dir = data_dir.as_ref();
        if !data_dir.is_absolute() || data_dir.parent().is_none() {
            return Err(VrgError::InvalidConfig(
                "derived data directory must be an absolute non-root path".into(),
            ));
        }
        fs::create_dir_all(data_dir).map_err(storage_io)?;
        let canonical = fs::canonicalize(data_dir).map_err(storage_io)?;
        if canonical != data_dir {
            return Err(VrgError::InvalidConfig(
                "derived data directory must be canonical and contain no symlink aliases".into(),
            ));
        }
        restrict_permissions(&canonical, 0o700)?;

        let database = data_dir.join("vrg.sqlite3");
        for path in [
            database.clone(),
            data_dir.join("vrg.sqlite3-wal"),
            data_dir.join("vrg.sqlite3-shm"),
        ] {
            match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(VrgError::SymlinkRejected(
                        "derived database files may not be symlinks".into(),
                    ));
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(storage_io(error)),
            }
        }
        let connection = Connection::open(database).map_err(storage_error)?;
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .map_err(storage_error)?;
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(storage_error)?;
        connection
            .pragma_update(None, "synchronous", "FULL")
            .map_err(storage_error)?;
        initialize_schema(&connection)?;
        restrict_permissions(&canonical.join("vrg.sqlite3"), 0o600)?;
        for name in ["vrg.sqlite3-wal", "vrg.sqlite3-shm"] {
            let path = canonical.join(name);
            if path.exists() {
                restrict_permissions(&path, 0o600)?;
            }
        }

        Ok(Self {
            connection: std::sync::Arc::new(Mutex::new(connection)),
            data_dir: canonical,
        })
    }

    pub fn current_generation_id(&self) -> VrgResult<Option<String>> {
        let connection = self.lock()?;
        active_generation(&connection)
    }

    /// Acquire the process-wide derived-index publication lock. The OS releases
    /// this lock automatically if a hook process exits or is terminated.
    pub fn try_sync_lock(&self) -> VrgResult<Option<File>> {
        self.try_file_lock(SYNC_LOCK_FILE)
    }

    pub fn try_monitor_lock(&self) -> VrgResult<Option<File>> {
        self.try_file_lock("vrg-watch.lock")
    }

    pub fn monitor_is_active(&self) -> VrgResult<bool> {
        Ok(self.try_monitor_lock()?.is_none())
    }

    pub fn mark_index_stale(&self) -> VrgResult<()> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        transaction
            .execute(
                "UPDATE source_change_revision SET revision = revision + 1 WHERE singleton = 1",
                [],
            )
            .map_err(storage_error)?;
        transaction.execute(
            "UPDATE index_sync_status SET state = 'stale', checked_unix_ms = ?1, error = NULL WHERE singleton = 1 AND state != 'indexing'",
            [now_unix_ms()?],
        ).map_err(storage_error)?;
        transaction.commit().map_err(storage_error)
    }

    pub fn source_change_revision(&self) -> VrgResult<i64> {
        let connection = self.lock()?;
        connection
            .query_row(
                "SELECT revision FROM source_change_revision WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(storage_error)
    }

    pub fn finish_index_sync(
        &self,
        expected_change_revision: i64,
        generation_id: Option<&str>,
        checked_unix_ms: i64,
        event_key: Option<&str>,
    ) -> VrgResult<IndexSyncStatus> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let current_revision: i64 = transaction
            .query_row(
                "SELECT revision FROM source_change_revision WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(storage_error)?;
        let state = if current_revision == expected_change_revision {
            "up_to_date"
        } else {
            "stale"
        };
        transaction.execute(
            "UPDATE index_sync_status SET state = ?1, generation_id = ?2, checked_unix_ms = ?3, error = NULL, event_key = COALESCE(?4, event_key) WHERE singleton = 1",
            rusqlite::params![state, generation_id, checked_unix_ms, event_key],
        ).map_err(storage_error)?;
        if let Some(event_key) = event_key {
            transaction.execute(
                "INSERT INTO index_sync_events(event_key, state, updated_unix_ms) VALUES (?1, ?2, ?3) ON CONFLICT(event_key) DO UPDATE SET state = excluded.state, updated_unix_ms = excluded.updated_unix_ms",
                rusqlite::params![event_key, state, checked_unix_ms],
            ).map_err(storage_error)?;
            transaction.execute(
                "DELETE FROM index_sync_events WHERE event_key IN (SELECT event_key FROM index_sync_events ORDER BY updated_unix_ms DESC LIMIT -1 OFFSET 256)",
                [],
            ).map_err(storage_error)?;
        }
        transaction.commit().map_err(storage_error)?;
        drop(connection);
        self.index_sync_status()
    }

    fn try_file_lock(&self, name: &str) -> VrgResult<Option<File>> {
        let lock_path = self.data_dir.join(name);
        match fs::symlink_metadata(&lock_path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(VrgError::SymlinkRejected(
                    "sync lock may not be a symlink".into(),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(storage_io(error)),
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(lock_path)
            .map_err(storage_io)?;
        restrict_permissions(&self.data_dir.join(name), 0o600)?;
        match file.try_lock() {
            Ok(()) => Ok(Some(file)),
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(std::fs::TryLockError::Error(error)) => Err(storage_io(error)),
        }
    }

    pub fn index_sync_status(&self) -> VrgResult<IndexSyncStatus> {
        let connection = self.lock()?;
        let (state, generation, checked, error): (String, Option<String>, Option<i64>, Option<String>) = connection
            .query_row(
                "SELECT state, generation_id, checked_unix_ms, error FROM index_sync_status WHERE singleton = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .map_err(storage_error)?;
        if state == "indexing" {
            drop(connection);
            if let Some(_lock) = self.try_sync_lock()? {
                let reason =
                    "previous index synchronization ended before publishing a final status";
                self.set_index_sync_status(
                    "failed",
                    generation.as_deref(),
                    checked,
                    Some(reason),
                    None,
                )?;
                return Ok(IndexSyncStatus::Failed {
                    generation_id: generation,
                    reason: reason.into(),
                    failed_unix_ms: now_unix_ms()?.max(0) as u64,
                });
            }
            return Ok(IndexSyncStatus::Indexing {
                generation_id: generation,
            });
        }
        match state.as_str() {
            "up_to_date" => Ok(IndexSyncStatus::UpToDate {
                generation_id: generation,
                checked_unix_ms: checked.unwrap_or_default().max(0) as u64,
            }),
            "failed" => Ok(IndexSyncStatus::Failed {
                generation_id: generation,
                reason: error.unwrap_or_else(|| "index synchronization failed".into()),
                failed_unix_ms: checked.unwrap_or_default().max(0) as u64,
            }),
            _ => Ok(IndexSyncStatus::Stale {
                generation_id: generation,
            }),
        }
    }

    pub fn set_index_sync_status(
        &self,
        state: &str,
        generation_id: Option<&str>,
        checked_unix_ms: Option<i64>,
        error: Option<&str>,
        event_key: Option<&str>,
    ) -> VrgResult<()> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        transaction.execute(
            "UPDATE index_sync_status SET state = ?1, generation_id = ?2, checked_unix_ms = ?3, error = ?4, event_key = COALESCE(?5, event_key) WHERE singleton = 1",
            rusqlite::params![state, generation_id, checked_unix_ms, error, event_key],
        ).map_err(storage_error)?;
        if let Some(event_key) = event_key {
            transaction.execute(
                "INSERT INTO index_sync_events(event_key, state, updated_unix_ms) VALUES (?1, ?2, ?3) ON CONFLICT(event_key) DO UPDATE SET state = excluded.state, updated_unix_ms = excluded.updated_unix_ms",
                rusqlite::params![event_key, state, checked_unix_ms.unwrap_or_default()],
            ).map_err(storage_error)?;
            transaction.execute(
                "DELETE FROM index_sync_events WHERE event_key IN (SELECT event_key FROM index_sync_events ORDER BY updated_unix_ms DESC LIMIT -1 OFFSET 256)",
                [],
            ).map_err(storage_error)?;
        }
        transaction.commit().map_err(storage_error)
    }

    pub fn sync_event_state(&self, event_key: &str) -> VrgResult<Option<String>> {
        let connection = self.lock()?;
        connection
            .query_row(
                "SELECT state FROM index_sync_events WHERE event_key = ?1",
                [event_key],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage_error)
    }

    pub fn record_session_event(
        &self,
        session_id: &str,
        turn_id: &str,
        project_key: &str,
        event_name: &str,
        observed_unix_ms: i64,
        retention_days: u16,
    ) -> VrgResult<()> {
        let connection = self.lock()?;
        let cutoff = observed_unix_ms.saturating_sub(i64::from(retention_days) * 86_400_000);
        let transaction = connection.unchecked_transaction().map_err(storage_error)?;
        transaction.execute(
            "INSERT OR IGNORE INTO session_events(session_id, turn_id, project_key, event_name, observed_unix_ms, evidence_state) VALUES (?1, ?2, ?3, ?4, ?5, 'observed')",
            rusqlite::params![session_id, turn_id, project_key, event_name, observed_unix_ms],
        ).map_err(storage_error)?;
        transaction
            .execute(
                "DELETE FROM session_events WHERE observed_unix_ms < ?1",
                [cutoff],
            )
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)
    }

    pub fn session_event_count(&self) -> VrgResult<u64> {
        let connection = self.lock()?;
        connection
            .query_row("SELECT COUNT(*) FROM session_events", [], |row| {
                row.get::<_, i64>(0)
            })
            .map(|count| count.max(0) as u64)
            .map_err(storage_error)
    }

    pub fn recent_session_events(
        &self,
        project_key: &str,
        limit: usize,
    ) -> VrgResult<Vec<SessionEventRecord>> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT session_id, turn_id, project_key, event_name, observed_unix_ms, evidence_state FROM session_events WHERE project_key = ?1 ORDER BY observed_unix_ms DESC LIMIT ?2",
        ).map_err(storage_error)?;
        let rows = statement
            .query_map(
                rusqlite::params![project_key, limit.min(100) as i64],
                |row| {
                    Ok(SessionEventRecord {
                        session_id: row.get(0)?,
                        turn_id: row.get(1)?,
                        project_key: row.get(2)?,
                        event_name: row.get(3)?,
                        observed_unix_ms: row.get(4)?,
                        evidence_state: row.get(5)?,
                    })
                },
            )
            .map_err(storage_error)?;
        rows.map(|row| row.map_err(storage_error)).collect()
    }

    pub fn clear_session_events(&self) -> VrgResult<u64> {
        let connection = self.lock()?;
        let removed = connection
            .execute("DELETE FROM session_events", [])
            .map_err(storage_error)?;
        Ok(removed as u64)
    }

    pub fn prune_session_events(&self, cutoff_unix_ms: i64) -> VrgResult<u64> {
        let connection = self.lock()?;
        let removed = connection
            .execute(
                "DELETE FROM session_events WHERE observed_unix_ms < ?1",
                [cutoff_unix_ms],
            )
            .map_err(storage_error)?;
        Ok(removed as u64)
    }

    pub fn load_current_snapshot(&self) -> VrgResult<Option<IndexSnapshot>> {
        let connection = self.lock()?;
        let Some(generation_id) = active_generation(&connection)? else {
            return Ok(None);
        };
        load_snapshot(&connection, &generation_id).map(Some)
    }

    /// Publishes a complete immutable generation in one SQLite transaction.
    /// A pending journal row makes a crash around the transaction recoverable.
    pub fn commit_generation(
        &self,
        snapshot: IndexSnapshot,
        expected_parent: Option<&str>,
    ) -> VrgResult<()> {
        self.recover_journal()?;
        validate_snapshot(&snapshot)?;
        let mut connection = self.lock()?;
        let before = active_generation(&connection)?;
        if before.as_deref() != expected_parent {
            return Err(VrgError::Conflict(format!(
                "expected parent {:?}, found {:?}",
                expected_parent, before
            )));
        }
        if before.as_deref() == Some(snapshot.generation_id.as_str()) {
            return Err(VrgError::Conflict(
                "new generation must differ from the active generation".into(),
            ));
        }
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM generations WHERE generation_id = ?1)",
                [&snapshot.generation_id],
                |row| row.get(0),
            )
            .map_err(storage_error)?;
        if exists {
            return Err(VrgError::Conflict(
                "generation ID already exists; generation IDs are immutable".into(),
            ));
        }

        let journal_id = begin_journal(
            &connection,
            "publish",
            before.as_deref(),
            &snapshot.generation_id,
        )?;

        f4_crash_after_journal();

        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let transaction_parent = active_generation(&transaction)?;
        if transaction_parent != before {
            drop(transaction);
            finish_journal(&connection, journal_id, "ABORTED")?;
            return Err(VrgError::Conflict(
                "active generation changed before publication".into(),
            ));
        }
        persist_snapshot(&transaction, &snapshot, before.as_deref())?;
        set_active_generation(&transaction, &snapshot.generation_id)?;
        transaction.commit().map_err(storage_error)?;

        finish_journal(&connection, journal_id, "COMMITTED")?;
        Ok(())
    }

    /// Reconciles pending journal rows by comparing the active pointer with the
    /// before/after generations. Rows are never guessed through ambiguous state.
    pub fn recover_journal(&self) -> VrgResult<RecoveryReport> {
        let connection = self.lock()?;
        let active = active_generation(&connection)?;
        let mut statement = connection
            .prepare(
                "SELECT journal_id, before_generation, after_generation \
                 FROM journal WHERE state = 'PREPARING' ORDER BY journal_id",
            )
            .map_err(storage_error)?;
        let pending = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)?;
        drop(statement);

        let mut report = RecoveryReport::default();
        for (journal_id, before, after) in pending {
            let state = if active.as_deref() == Some(after.as_str()) {
                report.committed += 1;
                "COMMITTED"
            } else if active == before {
                report.aborted += 1;
                "ABORTED"
            } else {
                return Err(VrgError::Recovery(format!(
                    "journal row {journal_id} refers to generations inconsistent with the active pointer"
                )));
            };
            finish_journal(&connection, journal_id, state)?;
        }
        Ok(report)
    }

    /// Restores only the derived-state pointer to a retained immutable generation.
    pub fn rollback_generation(
        &self,
        target_generation: &str,
        expected_current: &str,
    ) -> VrgResult<()> {
        self.recover_journal()?;
        let mut connection = self.lock()?;
        let current = active_generation(&connection)?;
        if current.as_deref() != Some(expected_current) {
            return Err(VrgError::Conflict(
                "active generation changed before rollback".into(),
            ));
        }
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM generations WHERE generation_id = ?1)",
                [target_generation],
                |row| row.get(0),
            )
            .map_err(storage_error)?;
        if !exists {
            return Err(VrgError::Recovery(
                "rollback target is not a retained generation".into(),
            ));
        }

        let journal_id = begin_journal(
            &connection,
            "rollback",
            current.as_deref(),
            target_generation,
        )?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        if active_generation(&transaction)?.as_deref() != Some(expected_current) {
            drop(transaction);
            finish_journal(&connection, journal_id, "ABORTED")?;
            return Err(VrgError::Conflict(
                "active generation changed during rollback".into(),
            ));
        }
        set_active_generation(&transaction, target_generation)?;
        transaction
            .execute(
                "UPDATE source_change_revision SET revision = revision + 1 WHERE singleton = 1",
                [],
            )
            .map_err(storage_error)?;
        transaction.execute(
            "UPDATE index_sync_status SET state = 'stale', generation_id = ?1, checked_unix_ms = ?2, error = NULL WHERE singleton = 1",
            rusqlite::params![target_generation, now_unix_ms()?],
        ).map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
        finish_journal(&connection, journal_id, "COMMITTED")?;
        Ok(())
    }

    pub fn exact_search_rows(
        &self,
        query: &str,
        root_ids: &[String],
        limit: usize,
    ) -> VrgResult<Vec<SearchCandidate>> {
        let connection = self.lock()?;
        let Some(generation) = active_generation(&connection)? else {
            return Ok(Vec::new());
        };
        let normalized = query.trim().trim_start_matches("./");
        if normalized.is_empty() {
            return Ok(Vec::new());
        }
        if limit == 0 {
            return Ok(Vec::new());
        }
        let path_with_extension = if normalized.ends_with(".md") {
            normalized.to_owned()
        } else {
            format!("{normalized}.md")
        };
        let mut statement = connection
            .prepare(
                "SELECT c.chunk_id, c.root_id, c.relative_path, c.revision, c.ordinal, \
                        c.start_byte, c.end_byte, c.body, d.title, \
                        CASE WHEN lower(c.relative_path) = lower(?2) \
                                  OR lower(c.relative_path) = lower(?3) \
                             THEN 1 ELSE 0 END \
                 FROM chunks c JOIN documents d USING (generation_id, root_id, relative_path) \
                 WHERE c.generation_id = ?1 AND \
                   (lower(c.relative_path) = lower(?2) OR lower(c.relative_path) = lower(?3) \
                    OR lower(d.title) = lower(?2)) \
                 ORDER BY c.relative_path, c.ordinal LIMIT ?4",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(
                params![
                    generation,
                    normalized,
                    path_with_extension,
                    MAX_QUERY_RESULTS as i64
                ],
                |row| {
                    let mut candidate = candidate_from_row(row)?;
                    candidate.path_match = row.get::<_, i64>(9)? != 0;
                    Ok(candidate)
                },
            )
            .map_err(storage_error)?;
        let mut candidates = Vec::new();
        for row in rows {
            let candidate = row.map_err(storage_error)?;
            if root_ids.is_empty()
                || root_ids
                    .iter()
                    .any(|root| root == &candidate.chunk.source.root_id)
            {
                candidates.push(SearchCandidate {
                    chunk: candidate.chunk,
                    title: candidate.title,
                    score: 1.0,
                    kinds: vec![if candidate.path_match {
                        MatchKind::ExactPath
                    } else {
                        MatchKind::ExactTitle
                    }],
                });
                if candidates.len() >= bounded_limit_usize(limit) {
                    break;
                }
            }
        }
        Ok(candidates)
    }

    pub fn fts_search_rows(
        &self,
        query: &str,
        root_ids: &[String],
        limit: usize,
    ) -> VrgResult<Vec<SearchCandidate>> {
        let connection = self.lock()?;
        let Some(generation) = active_generation(&connection)? else {
            return Ok(Vec::new());
        };
        let fts_query = fts_query(query);
        if fts_query.is_empty() {
            return Ok(Vec::new());
        }
        let mut statement = connection
            .prepare(
                "SELECT c.chunk_id, c.root_id, c.relative_path, c.revision, c.ordinal, \
                        c.start_byte, c.end_byte, c.body, d.title, bm25(chunk_fts) \
                 FROM chunk_fts JOIN chunks c \
                   ON chunk_fts.generation_id = c.generation_id \
                  AND chunk_fts.chunk_id = c.chunk_id \
                 JOIN documents d \
                   ON d.generation_id = c.generation_id \
                  AND d.root_id = c.root_id AND d.relative_path = c.relative_path \
                 WHERE chunk_fts MATCH ?1 AND c.generation_id = ?2 \
                 ORDER BY bm25(chunk_fts) LIMIT ?3",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(
                params![fts_query, generation, bounded_limit(limit)],
                |row| {
                    let mut candidate = candidate_from_row(row)?;
                    let bm25: f64 = row.get(9)?;
                    candidate.score = (-bm25).max(0.0) as f32;
                    Ok(candidate)
                },
            )
            .map_err(storage_error)?;
        let mut candidates = Vec::new();
        for row in rows {
            let candidate = row.map_err(storage_error)?;
            if root_ids.is_empty()
                || root_ids
                    .iter()
                    .any(|root| root == &candidate.chunk.source.root_id)
            {
                candidates.push(SearchCandidate {
                    chunk: candidate.chunk,
                    title: candidate.title,
                    score: candidate.score,
                    kinds: vec![MatchKind::FullText],
                });
            }
        }
        Ok(candidates)
    }

    pub fn vectors_for_model(
        &self,
        model_id: &str,
        root_ids: &[String],
    ) -> VrgResult<Vec<StoredVector>> {
        let connection = self.lock()?;
        let Some(generation) = active_generation(&connection)? else {
            return Ok(Vec::new());
        };
        let mut statement = connection
            .prepare(
                "SELECT c.chunk_id, c.root_id, c.relative_path, c.revision, c.ordinal, \
                        c.start_byte, c.end_byte, c.body, d.title, v.dimensions, v.vector_data \
                 FROM vectors v JOIN chunks c \
                   ON v.generation_id = c.generation_id AND v.chunk_id = c.chunk_id \
                 JOIN documents d \
                   ON d.generation_id = c.generation_id \
                  AND d.root_id = c.root_id AND d.relative_path = c.relative_path \
                 WHERE v.generation_id = ?1 AND v.model_id = ?2 \
                 ORDER BY c.root_id, c.relative_path, c.ordinal",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(params![generation, model_id], |row| {
                let chunk = chunk_from_row(row)?;
                let title = row.get(8)?;
                let dimensions: i64 = row.get(9)?;
                let bytes: Vec<u8> = row.get(10)?;
                let values = decode_vector(&bytes, dimensions as u32).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        10,
                        rusqlite::types::Type::Blob,
                        Box::new(error),
                    )
                })?;
                Ok((chunk, title, values))
            })
            .map_err(storage_error)?;
        let mut vectors = Vec::new();
        for row in rows {
            let (chunk, title, values) = row.map_err(storage_error)?;
            if root_ids.is_empty() || root_ids.iter().any(|root| root == &chunk.source.root_id) {
                vectors.push(StoredVector {
                    chunk,
                    title,
                    model_id: model_id.to_owned(),
                    values,
                });
            }
        }
        Ok(vectors)
    }

    pub fn graph_edges_from(&self, source: &SourceRef) -> VrgResult<Vec<GraphEdge>> {
        let connection = self.lock()?;
        let Some(generation) = active_generation(&connection)? else {
            return Ok(Vec::new());
        };
        let mut statement = connection
            .prepare(
                "SELECT from_root, from_path, to_root, to_path, unresolved_target, kind, \
                        start_byte, end_byte \
                 FROM edges WHERE generation_id = ?1 AND from_root = ?2 AND from_path = ?3 \
                 ORDER BY start_byte, end_byte",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(
                params![generation, source.root_id, source.relative_path],
                edge_from_row,
            )
            .map_err(storage_error)?;
        rows.map(|row| row.map_err(storage_error)).collect()
    }

    pub fn all_graph_edges(&self, limit: usize) -> VrgResult<Vec<GraphEdge>> {
        let connection = self.lock()?;
        let Some(generation) = active_generation(&connection)? else {
            return Ok(Vec::new());
        };
        let mut statement = connection
            .prepare(
                "SELECT from_root, from_path, to_root, to_path, unresolved_target, kind, \
                        start_byte, end_byte \
                 FROM edges WHERE generation_id = ?1 \
                 ORDER BY from_root, from_path, start_byte LIMIT ?2",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(params![generation, bounded_limit(limit)], edge_from_row)
            .map_err(storage_error)?;
        rows.map(|row| row.map_err(storage_error)).collect()
    }

    pub fn load_document(&self, source: &SourceRef) -> VrgResult<Option<StoredDocument>> {
        let connection = self.lock()?;
        let Some(generation) = active_generation(&connection)? else {
            return Ok(None);
        };
        let row = connection
            .query_row(
                "SELECT root_id, relative_path, revision, title, frontmatter_json, modified_unix_ms \
                 FROM documents WHERE generation_id = ?1 AND root_id = ?2 AND relative_path = ?3",
                params![generation, source.root_id, source.relative_path],
                document_from_row,
            )
            .optional()
            .map_err(storage_error)?;
        Ok(row)
    }

    fn lock(&self) -> VrgResult<std::sync::MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| VrgError::Storage("database lock is poisoned".into()))
    }
}

/// Test-only deterministic interruption point for the approved F4-04 journal
/// recovery procedure. The hook is compiled out of normal builds and exits
/// only when the feature and exact environment flag are both enabled.
#[cfg(feature = "f4-instrumentation")]
fn f4_crash_after_journal() {
    if std::env::var("VRG_F4_CRASH_AFTER_JOURNAL").as_deref() == Ok("1") {
        std::process::exit(86);
    }
}

#[cfg(not(feature = "f4-instrumentation"))]
fn f4_crash_after_journal() {}

struct CandidateRow {
    chunk: TextChunk,
    title: String,
    score: f32,
    path_match: bool,
}

fn initialize_schema(connection: &Connection) -> VrgResult<()> {
    connection
        .execute_batch(
            "BEGIN IMMEDIATE;
             CREATE TABLE IF NOT EXISTS schema_info (
                 singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                 version INTEGER NOT NULL
             );
             INSERT OR IGNORE INTO schema_info(singleton, version) VALUES (1, 1);
             CREATE TABLE IF NOT EXISTS generations (
                 generation_id TEXT PRIMARY KEY,
                 parent_id TEXT,
                 created_unix_ms INTEGER NOT NULL,
                 document_count INTEGER NOT NULL,
                 chunk_count INTEGER NOT NULL,
                 schema_version INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS active_generation (
                 singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                 generation_id TEXT NOT NULL REFERENCES generations(generation_id)
             );
             CREATE TABLE IF NOT EXISTS documents (
                 generation_id TEXT NOT NULL REFERENCES generations(generation_id),
                 root_id TEXT NOT NULL,
                 relative_path TEXT NOT NULL,
                 revision TEXT NOT NULL,
                 title TEXT NOT NULL,
                 frontmatter_json TEXT NOT NULL,
                 modified_unix_ms INTEGER,
                 PRIMARY KEY (generation_id, root_id, relative_path)
             );
             CREATE TABLE IF NOT EXISTS chunks (
                 generation_id TEXT NOT NULL,
                 chunk_id TEXT NOT NULL,
                 root_id TEXT NOT NULL,
                 relative_path TEXT NOT NULL,
                 revision TEXT NOT NULL,
                 ordinal INTEGER NOT NULL,
                 start_byte INTEGER NOT NULL,
                 end_byte INTEGER NOT NULL,
                 body TEXT NOT NULL,
                 PRIMARY KEY (generation_id, chunk_id),
                 FOREIGN KEY (generation_id, root_id, relative_path)
                     REFERENCES documents(generation_id, root_id, relative_path)
             );
             CREATE VIRTUAL TABLE IF NOT EXISTS chunk_fts USING fts5(
                 generation_id UNINDEXED,
                 chunk_id UNINDEXED,
                 root_id UNINDEXED,
                 relative_path UNINDEXED,
                 title UNINDEXED,
                 body,
                 tokenize = 'unicode61'
             );
             CREATE TABLE IF NOT EXISTS edges (
                 generation_id TEXT NOT NULL REFERENCES generations(generation_id),
                 from_root TEXT NOT NULL,
                 from_path TEXT NOT NULL,
                 to_root TEXT,
                 to_path TEXT,
                 unresolved_target TEXT,
                 kind TEXT NOT NULL,
                 start_byte INTEGER NOT NULL,
                 end_byte INTEGER NOT NULL,
                 CHECK ((to_root IS NOT NULL AND to_path IS NOT NULL AND unresolved_target IS NULL)
                     OR (to_root IS NULL AND to_path IS NULL AND unresolved_target IS NOT NULL))
             );
             CREATE INDEX IF NOT EXISTS edges_from_idx
                 ON edges(generation_id, from_root, from_path);
             CREATE INDEX IF NOT EXISTS edges_to_idx
                 ON edges(generation_id, to_root, to_path);
             CREATE TABLE IF NOT EXISTS vectors (
                 generation_id TEXT NOT NULL,
                 chunk_id TEXT NOT NULL,
                 model_id TEXT NOT NULL,
                 dimensions INTEGER NOT NULL,
                 vector_data BLOB NOT NULL,
                 PRIMARY KEY (generation_id, chunk_id, model_id),
                 FOREIGN KEY (generation_id, chunk_id)
                     REFERENCES chunks(generation_id, chunk_id)
             );
             CREATE TABLE IF NOT EXISTS journal (
                 journal_id INTEGER PRIMARY KEY AUTOINCREMENT,
                 operation TEXT NOT NULL,
                 state TEXT NOT NULL CHECK (state IN ('PREPARING', 'COMMITTED', 'ABORTED')),
                 before_generation TEXT,
                 after_generation TEXT NOT NULL,
                 created_unix_ms INTEGER NOT NULL,
                 finished_unix_ms INTEGER
             );
             CREATE TABLE IF NOT EXISTS index_sync_status (
                 singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                 state TEXT NOT NULL CHECK (state IN ('stale', 'indexing', 'up_to_date', 'failed')),
                 generation_id TEXT,
                 checked_unix_ms INTEGER,
                 error TEXT,
                 event_key TEXT
             );
             INSERT OR IGNORE INTO index_sync_status(singleton, state) VALUES (1, 'stale');
             CREATE TABLE IF NOT EXISTS index_sync_events (
                 event_key TEXT PRIMARY KEY,
                 state TEXT NOT NULL,
                 updated_unix_ms INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS session_events (
                 session_id TEXT NOT NULL,
                 turn_id TEXT NOT NULL,
                 project_key TEXT NOT NULL,
                 event_name TEXT NOT NULL,
                 observed_unix_ms INTEGER NOT NULL,
                 evidence_state TEXT NOT NULL CHECK (evidence_state IN ('observed','confirmed','proposed','failed','blocked','superseded')),
                 PRIMARY KEY(session_id, turn_id, event_name)
             );
             CREATE TABLE IF NOT EXISTS source_change_revision (
                 singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                 revision INTEGER NOT NULL
             );
             INSERT OR IGNORE INTO source_change_revision(singleton, revision) VALUES (1, 0);
             COMMIT;",
        )
        .map_err(storage_error)?;
    let version: i64 = connection
        .query_row(
            "SELECT version FROM schema_info WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .map_err(storage_error)?;
    if version != SCHEMA_VERSION {
        return Err(VrgError::Storage(format!(
            "unsupported derived-store schema version {version}"
        )));
    }
    Ok(())
}

fn validate_snapshot(snapshot: &IndexSnapshot) -> VrgResult<()> {
    if snapshot.generation_id.trim().is_empty() || snapshot.generation_id.len() > 128 {
        return Err(VrgError::InvalidConfig(
            "generation ID must be non-empty and at most 128 bytes".into(),
        ));
    }
    let mut documents = BTreeMap::new();
    for document in &snapshot.documents {
        validate_source_ref(&document.source)?;
        let key = source_key(&document.source);
        if documents.insert(key, document).is_some() {
            return Err(VrgError::InvalidConfig(
                "snapshot contains duplicate source identities".into(),
            ));
        }
        if document.revision.is_empty() {
            return Err(VrgError::InvalidConfig(
                "source revisions must be present".into(),
            ));
        }
    }

    let mut chunks = BTreeSet::new();
    for chunk in &snapshot.chunks {
        validate_source_ref(&chunk.source)?;
        let Some(document) = documents.get(&source_key(&chunk.source)) else {
            return Err(VrgError::InvalidConfig(
                "chunk references a source absent from the snapshot".into(),
            ));
        };
        if document.revision != chunk.revision
            || chunk.start_byte > chunk.end_byte
            || chunk.id.is_empty()
            || !chunks.insert(chunk.id.as_str())
        {
            return Err(VrgError::InvalidConfig(
                "chunk identity, revision, or byte range is invalid".into(),
            ));
        }
    }

    for edge in &snapshot.edges {
        validate_source_ref(&edge.from)?;
        if !documents.contains_key(&source_key(&edge.from)) {
            return Err(VrgError::InvalidConfig(
                "graph edge source is absent from the snapshot".into(),
            ));
        }
        match (&edge.to, &edge.unresolved_target) {
            (Some(to), None) if documents.contains_key(&source_key(to)) => {
                validate_source_ref(to)?;
            }
            (None, Some(target)) if !target.trim().is_empty() => {}
            _ => {
                return Err(VrgError::InvalidConfig(
                    "graph edge must have exactly one resolved or unresolved target".into(),
                ));
            }
        }
        if edge.start_byte > edge.end_byte {
            return Err(VrgError::InvalidConfig(
                "graph edge byte range is invalid".into(),
            ));
        }
    }

    let mut vector_keys = BTreeSet::new();
    for vector in &snapshot.vectors {
        if !chunks.contains(vector.chunk_id.as_str())
            || vector.model_id.trim().is_empty()
            || vector.dimensions == 0
            || vector.values.len() != vector.dimensions as usize
            || vector.values.iter().any(|value| !value.is_finite())
            || !vector_keys.insert((vector.chunk_id.as_str(), vector.model_id.as_str()))
        {
            return Err(VrgError::InvalidConfig(
                "embedding identity, dimensions, values, or model ID is invalid".into(),
            ));
        }
    }
    Ok(())
}

fn persist_snapshot(
    transaction: &Transaction<'_>,
    snapshot: &IndexSnapshot,
    parent: Option<&str>,
) -> VrgResult<()> {
    transaction
        .execute(
            "INSERT INTO generations \
             (generation_id, parent_id, created_unix_ms, document_count, chunk_count, schema_version) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                snapshot.generation_id,
                parent,
                now_unix_ms()?,
                snapshot.documents.len() as i64,
                snapshot.chunks.len() as i64,
                SCHEMA_VERSION,
            ],
        )
        .map_err(storage_error)?;

    {
        let mut statement = transaction
            .prepare(
                "INSERT INTO documents \
                 (generation_id, root_id, relative_path, revision, title, frontmatter_json, modified_unix_ms) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )
            .map_err(storage_error)?;
        for document in &snapshot.documents {
            let frontmatter = serde_json::to_string(&document.frontmatter)
                .map_err(|error| VrgError::Storage(error.to_string()))?;
            statement
                .execute(params![
                    snapshot.generation_id,
                    document.source.root_id,
                    document.source.relative_path,
                    document.revision,
                    document.title,
                    frontmatter,
                    optional_u64(document.modified_unix_ms)?,
                ])
                .map_err(storage_error)?;
        }
    }

    {
        let mut chunk_statement = transaction
            .prepare(
                "INSERT INTO chunks \
                 (generation_id, chunk_id, root_id, relative_path, revision, ordinal, start_byte, end_byte, body) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )
            .map_err(storage_error)?;
        let mut fts_statement = transaction
            .prepare(
                "INSERT INTO chunk_fts \
                 (generation_id, chunk_id, root_id, relative_path, title, body) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )
            .map_err(storage_error)?;
        let titles: BTreeMap<_, _> = snapshot
            .documents
            .iter()
            .map(|document| (source_key(&document.source), document.title.as_str()))
            .collect();
        for chunk in &snapshot.chunks {
            chunk_statement
                .execute(params![
                    snapshot.generation_id,
                    chunk.id,
                    chunk.source.root_id,
                    chunk.source.relative_path,
                    chunk.revision,
                    i64::from(chunk.ordinal),
                    i64::from(chunk.start_byte),
                    i64::from(chunk.end_byte),
                    chunk.text,
                ])
                .map_err(storage_error)?;
            let title = titles
                .get(&source_key(&chunk.source))
                .copied()
                .ok_or_else(|| VrgError::Storage("chunk title disappeared".into()))?;
            fts_statement
                .execute(params![
                    snapshot.generation_id,
                    chunk.id,
                    chunk.source.root_id,
                    chunk.source.relative_path,
                    title,
                    chunk.text,
                ])
                .map_err(storage_error)?;
        }
    }

    {
        let mut statement = transaction
            .prepare(
                "INSERT INTO edges \
                 (generation_id, from_root, from_path, to_root, to_path, unresolved_target, kind, start_byte, end_byte) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )
            .map_err(storage_error)?;
        for edge in &snapshot.edges {
            statement
                .execute(params![
                    snapshot.generation_id,
                    edge.from.root_id,
                    edge.from.relative_path,
                    edge.to.as_ref().map(|source| source.root_id.as_str()),
                    edge.to.as_ref().map(|source| source.relative_path.as_str()),
                    edge.unresolved_target,
                    link_kind_name(edge.kind),
                    i64::from(edge.start_byte),
                    i64::from(edge.end_byte),
                ])
                .map_err(storage_error)?;
        }
    }

    {
        let mut statement = transaction
            .prepare(
                "INSERT INTO vectors (generation_id, chunk_id, model_id, dimensions, vector_data) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )
            .map_err(storage_error)?;
        for vector in &snapshot.vectors {
            statement
                .execute(params![
                    snapshot.generation_id,
                    vector.chunk_id,
                    vector.model_id,
                    i64::from(vector.dimensions),
                    encode_vector(&vector.values),
                ])
                .map_err(storage_error)?;
        }
    }
    Ok(())
}

fn load_snapshot(connection: &Connection, generation: &str) -> VrgResult<IndexSnapshot> {
    let mut documents = Vec::new();
    {
        let mut statement = connection
            .prepare(
                "SELECT root_id, relative_path, revision, title, frontmatter_json, modified_unix_ms \
                 FROM documents WHERE generation_id = ?1 ORDER BY root_id, relative_path",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map([generation], document_from_row)
            .map_err(storage_error)?;
        for row in rows {
            documents.push(row.map_err(storage_error)?);
        }
    }

    let mut chunks = Vec::new();
    {
        let mut statement = connection
            .prepare(
                "SELECT chunk_id, root_id, relative_path, revision, ordinal, start_byte, end_byte, body \
                 FROM chunks WHERE generation_id = ?1 ORDER BY root_id, relative_path, ordinal",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map([generation], chunk_from_row)
            .map_err(storage_error)?;
        for row in rows {
            chunks.push(row.map_err(storage_error)?);
        }
    }

    let mut edges = Vec::new();
    {
        let mut statement = connection
            .prepare(
                "SELECT from_root, from_path, to_root, to_path, unresolved_target, kind, start_byte, end_byte \
                 FROM edges WHERE generation_id = ?1 ORDER BY from_root, from_path, start_byte",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map([generation], edge_from_row)
            .map_err(storage_error)?;
        for row in rows {
            edges.push(row.map_err(storage_error)?);
        }
    }

    let mut vectors = Vec::new();
    {
        let mut statement = connection
            .prepare(
                "SELECT chunk_id, model_id, dimensions, vector_data FROM vectors \
                 WHERE generation_id = ?1 ORDER BY chunk_id, model_id",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map([generation], |row| {
                let chunk_id: String = row.get(0)?;
                let model_id: String = row.get(1)?;
                let dimensions: i64 = row.get(2)?;
                let bytes: Vec<u8> = row.get(3)?;
                let values = decode_vector(&bytes, dimensions as u32).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        3,
                        rusqlite::types::Type::Blob,
                        Box::new(error),
                    )
                })?;
                Ok(VectorEmbedding {
                    chunk_id,
                    model_id,
                    dimensions: dimensions as u32,
                    values,
                })
            })
            .map_err(storage_error)?;
        for row in rows {
            vectors.push(row.map_err(storage_error)?);
        }
    }

    Ok(IndexSnapshot {
        generation_id: generation.to_owned(),
        documents,
        chunks,
        edges,
        vectors,
    })
}

fn begin_journal(
    connection: &Connection,
    operation: &str,
    before: Option<&str>,
    after: &str,
) -> VrgResult<i64> {
    connection
        .execute(
            "INSERT INTO journal(operation, state, before_generation, after_generation, created_unix_ms) \
             VALUES (?1, 'PREPARING', ?2, ?3, ?4)",
            params![operation, before, after, now_unix_ms()?],
        )
        .map_err(storage_error)?;
    Ok(connection.last_insert_rowid())
}

fn finish_journal(connection: &Connection, journal_id: i64, state: &str) -> VrgResult<()> {
    let changed = connection
        .execute(
            "UPDATE journal SET state = ?1, finished_unix_ms = ?2 \
             WHERE journal_id = ?3 AND state = 'PREPARING'",
            params![state, now_unix_ms()?, journal_id],
        )
        .map_err(storage_error)?;
    if changed != 1 {
        return Err(VrgError::Recovery(format!(
            "journal row {journal_id} was not pending"
        )));
    }
    Ok(())
}

fn active_generation(connection: &Connection) -> VrgResult<Option<String>> {
    connection
        .query_row(
            "SELECT generation_id FROM active_generation WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage_error)
}

fn set_active_generation(connection: &Connection, generation: &str) -> VrgResult<()> {
    connection
        .execute(
            "INSERT INTO active_generation(singleton, generation_id) VALUES (1, ?1) \
             ON CONFLICT(singleton) DO UPDATE SET generation_id = excluded.generation_id",
            [generation],
        )
        .map_err(storage_error)?;
    Ok(())
}

fn document_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredDocument> {
    let root_id: String = row.get(0)?;
    let relative_path: String = row.get(1)?;
    let revision: String = row.get(2)?;
    let title: String = row.get(3)?;
    let frontmatter: String = row.get(4)?;
    let modified: Option<i64> = row.get(5)?;
    let frontmatter = serde_json::from_str(&frontmatter).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(StoredDocument {
        source: SourceRef {
            root_id,
            relative_path,
        },
        revision,
        title,
        frontmatter,
        modified_unix_ms: modified.and_then(|value| u64::try_from(value).ok()),
    })
}

fn chunk_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TextChunk> {
    Ok(TextChunk {
        id: row.get(0)?,
        source: SourceRef {
            root_id: row.get(1)?,
            relative_path: row.get(2)?,
        },
        revision: row.get(3)?,
        ordinal: row.get::<_, i64>(4)?.try_into().map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                4,
                rusqlite::types::Type::Integer,
                Box::new(error),
            )
        })?,
        start_byte: row.get::<_, i64>(5)?.try_into().map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                5,
                rusqlite::types::Type::Integer,
                Box::new(error),
            )
        })?,
        end_byte: row.get::<_, i64>(6)?.try_into().map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                6,
                rusqlite::types::Type::Integer,
                Box::new(error),
            )
        })?,
        text: row.get(7)?,
    })
}

fn candidate_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CandidateRow> {
    let chunk = chunk_from_row(row)?;
    let title: String = row.get(8)?;
    Ok(CandidateRow {
        chunk,
        title,
        score: 0.0,
        path_match: false,
    })
}

fn edge_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<GraphEdge> {
    let from = SourceRef {
        root_id: row.get(0)?,
        relative_path: row.get(1)?,
    };
    let to_root: Option<String> = row.get(2)?;
    let to_path: Option<String> = row.get(3)?;
    let unresolved_target: Option<String> = row.get(4)?;
    let kind: String = row.get(5)?;
    let start_byte: i64 = row.get(6)?;
    let end_byte: i64 = row.get(7)?;
    let to = match (to_root, to_path) {
        (Some(root_id), Some(relative_path)) => Some(SourceRef {
            root_id,
            relative_path,
        }),
        (None, None) => None,
        _ => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                2,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "inconsistent graph target columns",
                )),
            ));
        }
    };
    let kind = match kind.as_str() {
        "wiki" => LinkKind::Wiki,
        "embed" => LinkKind::Embed,
        "markdown" => LinkKind::Markdown,
        _ => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                5,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "unknown graph edge kind",
                )),
            ));
        }
    };
    Ok(GraphEdge {
        from,
        to,
        unresolved_target,
        kind,
        start_byte: start_byte.try_into().map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                6,
                rusqlite::types::Type::Integer,
                Box::new(error),
            )
        })?,
        end_byte: end_byte.try_into().map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                7,
                rusqlite::types::Type::Integer,
                Box::new(error),
            )
        })?,
    })
}

fn validate_source_ref(source: &SourceRef) -> VrgResult<()> {
    if source.root_id.trim().is_empty()
        || source.root_id == "."
        || source.root_id == ".."
        || source.root_id.contains('/')
        || source.root_id.contains('\\')
        || source.relative_path.trim().is_empty()
    {
        return Err(VrgError::InvalidConfig(
            "source identity requires an opaque root ID and relative path".into(),
        ));
    }
    let path = Path::new(&source.relative_path);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        || source.relative_path.starts_with('/')
        || source.relative_path.ends_with('/')
        || source
            .relative_path
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
        || source.relative_path.contains('\\')
        || source.relative_path.contains('\0')
    {
        return Err(VrgError::PathEscape(
            "persistent source path must be normalized and root-relative".into(),
        ));
    }
    Ok(())
}

fn source_key(source: &SourceRef) -> (String, String) {
    (source.root_id.clone(), source.relative_path.clone())
}

fn link_kind_name(kind: LinkKind) -> &'static str {
    match kind {
        LinkKind::Wiki => "wiki",
        LinkKind::Embed => "embed",
        LinkKind::Markdown => "markdown",
    }
}

fn encode_vector(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn decode_vector(bytes: &[u8], dimensions: u32) -> Result<Vec<f32>, std::io::Error> {
    if dimensions == 0 || bytes.len() != dimensions as usize * std::mem::size_of::<f32>() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "stored vector length does not match its dimension",
        ));
    }
    let values = bytes
        .chunks_exact(std::mem::size_of::<f32>())
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect::<Vec<_>>();
    if values.iter().any(|value| !value.is_finite()) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "stored vector contains non-finite values",
        ));
    }
    Ok(values)
}

fn fts_query(query: &str) -> String {
    query
        .split_whitespace()
        .filter(|term| !term.is_empty())
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ")
}

fn bounded_limit(limit: usize) -> i64 {
    bounded_limit_usize(limit) as i64
}

fn bounded_limit_usize(limit: usize) -> usize {
    limit.min(MAX_QUERY_RESULTS)
}

fn optional_u64(value: Option<u64>) -> VrgResult<Option<i64>> {
    value
        .map(|value| {
            i64::try_from(value)
                .map_err(|_| VrgError::Storage("timestamp is outside SQLite integer range".into()))
        })
        .transpose()
}

fn now_unix_ms() -> VrgResult<i64> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| VrgError::Storage(error.to_string()))?;
    i64::try_from(elapsed.as_millis())
        .map_err(|_| VrgError::Storage("system time is outside SQLite integer range".into()))
}

fn storage_io(error: std::io::Error) -> VrgError {
    VrgError::Storage(error.to_string())
}

#[cfg(unix)]
fn restrict_permissions(path: &Path, mode: u32) -> VrgResult<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path).map_err(storage_io)?.permissions();
    permissions.set_mode(mode);
    fs::set_permissions(path, permissions).map_err(storage_io)
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path, _mode: u32) -> VrgResult<()> {
    Ok(())
}

fn storage_error(error: rusqlite::Error) -> VrgError {
    VrgError::Storage(error.to_string())
}
