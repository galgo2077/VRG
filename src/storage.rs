use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    fs::File,
    fs::OpenOptions,
    path::{Component, Path, PathBuf},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use crate::{
    contracts::{
        GraphEdge, IndexSnapshot, IndexSyncStatus, LinkKind, MatchKind, SearchCandidate,
        SessionEventRecord, SourceRef, StoredDocument, TextChunk, VectorEmbedding,
    },
    error::{VrgError, VrgResult},
};

const SCHEMA_VERSION: i64 = 2;
const MAX_QUERY_RESULTS: usize = 1000;
const SYNC_LOCK_FILE: &str = "vrg-sync.lock";

// Resolve matching documents before touching their chunks. Starting from the
// chunks view makes an exact path lookup walk every chunk in the generation.
const EXACT_SEARCH_SQL: &str = "SELECT m.chunk_id, m.root_id, m.relative_path, m.revision, m.ordinal, \
            m.start_byte, m.end_byte, o.payload, d.title, \
            CASE WHEN lower(d.relative_path) = lower(?2) \
                      OR lower(d.relative_path) = lower(?3) \
                 THEN 1 ELSE 0 END \
     FROM documents d \
     CROSS JOIN chunk_manifests m INDEXED BY chunks_source_idx \
       ON m.generation_id = d.generation_id AND m.root_id = d.root_id \
      AND m.relative_path = d.relative_path \
     CROSS JOIN text_objects o ON o.object_id = m.object_id \
     WHERE d.generation_id = ?1 \
       AND (json_array_length(?5)=0 OR d.root_id IN (SELECT value FROM json_each(?5))) \
       AND (lower(d.relative_path) = lower(?2) OR lower(d.relative_path) = lower(?3) \
            OR lower(d.title) = lower(?2)) \
       AND (?6 IS NULL OR (d.root_id,d.relative_path) IN \
         (SELECT json_extract(value,'$.root_id'),json_extract(value,'$.relative_path') FROM json_each(?6))) \
     ORDER BY m.relative_path, m.ordinal LIMIT ?4";

// MATCH must drive the join once, rather than executing an FTS lookup for each
// active chunk. CROSS JOIN fixes only that join order; both root and source-scope
// restrictions and the original bm25 ordering still apply before LIMIT.
const FTS_SEARCH_SQL: &str = "SELECT m.chunk_id, m.root_id, m.relative_path, m.revision, m.ordinal, \
            m.start_byte, m.end_byte, o.payload, d.title, bm25(chunk_fts) \
     FROM chunk_fts \
     CROSS JOIN text_objects o ON o.rowid = chunk_fts.rowid \
     CROSS JOIN chunk_manifests m INDEXED BY chunks_object_idx \
       ON m.generation_id = ?2 AND m.object_id = o.object_id \
     CROSS JOIN documents d \
       ON d.generation_id = m.generation_id AND d.root_id = m.root_id \
      AND d.relative_path = m.relative_path \
     WHERE chunk_fts MATCH ?1 AND m.generation_id = ?2 \
       AND (json_array_length(?4)=0 OR m.root_id IN (SELECT value FROM json_each(?4))) \
       AND (?5 IS NULL OR (m.root_id,m.relative_path) IN \
         (SELECT json_extract(value,'$.root_id'),json_extract(value,'$.relative_path') FROM json_each(?5))) \
     ORDER BY bm25(chunk_fts) LIMIT ?3";

#[derive(Clone)]
pub struct DerivedStore {
    connection: std::sync::Arc<Mutex<Connection>>,
    data_dir: PathBuf,
    _schema_lease: std::sync::Arc<File>,
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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MaintenancePlan {
    pub expected_generation: Option<String>,
    pub retain: usize,
    pub remove_generations: Vec<String>,
    pub protection_fingerprint: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct StorageStats {
    pub schema_version: i64,
    pub generations: u64,
    pub documents: u64,
    pub chunks: u64,
    pub text_objects: u64,
    pub vector_objects: u64,
    pub logical_text_bytes: u64,
    pub payload_bytes: u64,
    pub database_bytes: u64,
    pub wal_bytes: u64,
    pub shared_memory_bytes: u64,
    pub freelist_bytes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MaintenanceReport {
    pub removed_generations: usize,
    pub removed_text_objects: usize,
    pub removed_vector_objects: usize,
    pub filesystem_bytes_released: u64,
    pub stats: StorageStats,
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
        let schema_lease = schema_lock(&canonical, false)?;
        let connection = Connection::open(database).map_err(storage_error)?;
        let existing: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='schema_info')",
                [],
                |r| r.get(0),
            )
            .map_err(storage_error)?;
        if existing {
            check_schema_connection(&connection)?;
        }
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .map_err(storage_error)?;
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(storage_error)?;
        connection
            .pragma_update(None, "synchronous", "FULL")
            .map_err(storage_error)?;
        connection
            .pragma_update(None, "cache_size", -2048)
            .map_err(storage_error)?;
        connection
            .pragma_update(None, "mmap_size", 16 * 1024 * 1024)
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
            _schema_lease: std::sync::Arc::new(schema_lease),
        })
    }

    fn check_schema(&self) -> VrgResult<()> {
        let connection = self.lock()?;
        check_schema_connection(&connection)
    }

    /// Metadata-only snapshot: retrieval must not eagerly decode every object.
    pub fn load_current_catalog(&self) -> VrgResult<Option<IndexSnapshot>> {
        let _reader = self.acquire_reader_lease()?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction().map_err(storage_error)?;
        let Some(generation) = active_generation(&transaction)? else {
            return Ok(None);
        };
        let mut statement = transaction.prepare("SELECT root_id,relative_path,revision,title,frontmatter_json,modified_unix_ms FROM documents WHERE generation_id=?1 ORDER BY root_id,relative_path").map_err(storage_error)?;
        let documents = statement
            .query_map([&generation], document_from_row)
            .map_err(storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)?;
        drop(statement);
        let mut statement = transaction.prepare("SELECT from_root,from_path,to_root,to_path,unresolved_target,kind,start_byte,end_byte FROM edges WHERE generation_id=?1 ORDER BY from_root,from_path,start_byte").map_err(storage_error)?;
        let edges = statement
            .query_map([&generation], edge_from_row)
            .map_err(storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)?;
        drop(statement);
        transaction.commit().map_err(storage_error)?;
        Ok(Some(IndexSnapshot {
            generation_id: generation,
            documents,
            chunks: Vec::new(),
            edges,
            vectors: Vec::new(),
        }))
    }

    /// Hold across a multi-call read to keep GC from retiring its generation.
    /// Locks are released by the OS after a crash; there is no clock-based TTL.
    pub fn acquire_reader_lease(&self) -> VrgResult<File> {
        named_lock(&self.data_dir, "vrg-gc.lock", false)
    }

    pub fn chunks_for_source(&self, source: &SourceRef) -> VrgResult<Vec<TextChunk>> {
        let _reader = self.acquire_reader_lease()?;
        let connection = self.lock()?;
        let Some(generation) = active_generation(&connection)? else {
            return Ok(Vec::new());
        };
        let mut statement=connection.prepare("SELECT chunk_id,root_id,relative_path,revision,ordinal,start_byte,end_byte,body FROM chunks WHERE generation_id=?1 AND root_id=?2 AND relative_path=?3 ORDER BY ordinal").map_err(storage_error)?;
        statement
            .query_map(
                params![generation, source.root_id, source.relative_path],
                chunk_from_row,
            )
            .map_err(storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    pub fn chunk_by_id(&self, id: &str) -> VrgResult<Option<TextChunk>> {
        let _reader = self.acquire_reader_lease()?;
        let connection = self.lock()?;
        let Some(generation) = active_generation(&connection)? else {
            return Ok(None);
        };
        connection.query_row("SELECT chunk_id,root_id,relative_path,revision,ordinal,start_byte,end_byte,body FROM chunks WHERE generation_id=?1 AND chunk_id=?2",params![generation,id],chunk_from_row).optional().map_err(storage_error)
    }

    pub fn current_chunk_ids(&self, root_ids: &[String]) -> VrgResult<BTreeSet<String>> {
        self.current_chunk_ids_scoped(root_ids, None)
    }

    /// Count active chunks and vectors for one exact model identity without
    /// loading vector payloads into the status path.
    pub fn semantic_coverage(&self, model_id: &str) -> VrgResult<(i64, i64)> {
        let connection = self.lock()?;
        let Some(generation) = active_generation(&connection)? else {
            return Ok((0, 0));
        };
        let chunks: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM chunk_manifests WHERE generation_id=?1",
                [&generation],
                |row| row.get(0),
            )
            .map_err(storage_error)?;
        let vectors: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM vector_manifests WHERE generation_id=?1 AND model_id=?2",
                params![generation, model_id],
                |row| row.get(0),
            )
            .map_err(storage_error)?;
        Ok((chunks, vectors))
    }

    pub fn current_chunk_ids_scoped(
        &self,
        root_ids: &[String],
        sources: Option<&BTreeSet<SourceRef>>,
    ) -> VrgResult<BTreeSet<String>> {
        let _reader = self.acquire_reader_lease()?;
        let connection = self.lock()?;
        let Some(generation) = active_generation(&connection)? else {
            return Ok(BTreeSet::new());
        };
        let mut statement = connection
            .prepare("SELECT chunk_id,root_id FROM chunk_manifests WHERE generation_id=?1 AND (?2 IS NULL OR (root_id,relative_path) IN (SELECT json_extract(value,'$.root_id'),json_extract(value,'$.relative_path') FROM json_each(?2)))")
            .map_err(storage_error)?;
        let rows = statement
            .query_map(params![generation, source_scope_json(sources)?], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)?;
        Ok(rows
            .into_iter()
            .filter(|(_, root)| root_ids.is_empty() || root_ids.contains(root))
            .map(|(id, _)| id)
            .collect())
    }

    pub fn load_adaptive_state(&self) -> VrgResult<Option<String>> {
        let connection = self.lock()?;
        connection
            .query_row(
                "SELECT state_json FROM adaptive_state WHERE singleton=1",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(storage_error)
    }

    /// Optimistic generation + replay guard are checked in the same transaction.
    pub fn apply_adaptive_state(
        &self,
        expected_generation: &str,
        proposal_id: &str,
        fingerprint: &str,
        state_json: &str,
    ) -> VrgResult<bool> {
        let _references = self.acquire_reader_lease()?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        check_schema_connection(&transaction)?;
        let receipt: Option<String> = transaction
            .query_row(
                "SELECT fingerprint FROM adaptive_receipts WHERE proposal_id=?1",
                [proposal_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(storage_error)?;
        if let Some(prior) = receipt {
            if prior == fingerprint {
                return Ok(false);
            }
            return Err(VrgError::Conflict(
                "proposal ID was already used with different content".into(),
            ));
        }
        if active_generation(&transaction)?.as_deref() != Some(expected_generation) {
            return Err(VrgError::Conflict(
                "adaptive proposal generation is stale".into(),
            ));
        }
        serde_json::from_str::<serde_json::Value>(state_json)
            .map_err(|e| VrgError::InvalidConfig(e.to_string()))?;
        transaction.execute("INSERT INTO adaptive_state(singleton,generation_id,proposal_id,fingerprint,state_json) VALUES(1,?1,?2,?3,?4) ON CONFLICT(singleton) DO UPDATE SET generation_id=excluded.generation_id,proposal_id=excluded.proposal_id,fingerprint=excluded.fingerprint,state_json=excluded.state_json",params![expected_generation,proposal_id,fingerprint,state_json]).map_err(storage_error)?;
        transaction
            .execute(
                "INSERT INTO adaptive_receipts(proposal_id,fingerprint) VALUES(?1,?2)",
                params![proposal_id, fingerprint],
            )
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
        Ok(true)
    }

    /// Every exact source revision still reachable through retained manifests or
    /// adaptive references. Archive GC must union this with currently authorized sources.
    pub fn retained_source_revisions(&self) -> VrgResult<Vec<(SourceRef, String)>> {
        let _reader = self.acquire_reader_lease()?;
        let connection = self.lock()?;
        retained_source_revisions_connection(&connection, &[])
    }

    /// Run archive reachability/deletion while cooperative publishers, pins and
    /// adaptive reference writers are excluded. Caller already owns sync lock;
    /// the callback must not re-enter this store or acquire a nested reader lease.
    pub fn with_retained_source_revisions<R>(
        &self,
        action: impl FnOnce(Vec<(SourceRef, String)>) -> VrgResult<R>,
    ) -> VrgResult<R> {
        let _exclusive = named_lock(&self.data_dir, "vrg-gc.lock", true)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        check_schema_connection(&transaction)?;
        let references = retained_source_revisions_connection(&transaction, &[])?;
        let result = action(references)?;
        transaction.commit().map_err(storage_error)?;
        Ok(result)
    }

    pub fn projected_source_revisions(
        &self,
        plan: &MaintenancePlan,
    ) -> VrgResult<Vec<(SourceRef, String)>> {
        let _reader = self.acquire_reader_lease()?;
        let connection = self.lock()?;
        if maintenance_plan_connection(&connection, plan.retain)? != *plan {
            return Err(VrgError::Conflict(
                "maintenance reference plan is stale".into(),
            ));
        }
        retained_source_revisions_connection(&connection, &plan.remove_generations)
    }

    pub fn storage_stats(&self) -> VrgResult<StorageStats> {
        let connection = self.lock()?;
        storage_stats_connection(&connection, &self.data_dir)
    }

    pub fn pin_generation(&self, generation: &str, pin_id: &str) -> VrgResult<()> {
        let _references = self.acquire_reader_lease()?;
        if pin_id.is_empty() || pin_id.len() > 256 {
            return Err(VrgError::InvalidConfig("invalid pin ID".into()));
        }
        let connection = self.lock()?;
        check_schema_connection(&connection)?;
        connection
            .execute(
                "INSERT OR IGNORE INTO generation_pins(generation_id,pin_id) VALUES(?1,?2)",
                params![generation, pin_id],
            )
            .map_err(storage_error)?;
        Ok(())
    }

    pub fn unpin_generation(&self, generation: &str, pin_id: &str) -> VrgResult<()> {
        let _references = self.acquire_reader_lease()?;
        let connection = self.lock()?;
        check_schema_connection(&connection)?;
        connection
            .execute(
                "DELETE FROM generation_pins WHERE generation_id=?1 AND pin_id=?2",
                params![generation, pin_id],
            )
            .map_err(storage_error)?;
        Ok(())
    }

    pub fn maintenance_plan(&self, retain: usize) -> VrgResult<MaintenancePlan> {
        let connection = self.lock()?;
        maintenance_plan_connection(&connection, retain)
    }

    pub fn apply_maintenance(&self, plan: &MaintenancePlan) -> VrgResult<MaintenanceReport> {
        // Match publication and COLD maintenance: sync, then GC, then SQLite.
        let _sync = named_lock(&self.data_dir, SYNC_LOCK_FILE, true)?;
        let _gc = named_lock(&self.data_dir, "vrg-gc.lock", true)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        check_schema_connection(&transaction)?;
        if maintenance_plan_connection(&transaction, plan.retain)? != *plan {
            return Err(VrgError::Conflict(
                "maintenance plan is stale; protected references changed".into(),
            ));
        }
        for generation in &plan.remove_generations {
            for table in ["vector_manifests", "chunk_manifests", "edges", "documents"] {
                transaction
                    .execute(
                        &format!("DELETE FROM {table} WHERE generation_id=?1"),
                        [generation],
                    )
                    .map_err(storage_error)?;
            }
            transaction
                .execute(
                    "DELETE FROM generations WHERE generation_id=?1",
                    [generation],
                )
                .map_err(storage_error)?;
        }
        let removed_vectors=transaction.execute("DELETE FROM vector_objects WHERE NOT EXISTS(SELECT 1 FROM vector_manifests m WHERE m.object_id=vector_objects.object_id)",[]).map_err(storage_error)?;
        transaction.execute("DELETE FROM chunk_fts WHERE rowid IN (SELECT o.rowid FROM text_objects o WHERE NOT EXISTS(SELECT 1 FROM chunk_manifests m WHERE m.object_id=o.object_id) AND NOT EXISTS(SELECT 1 FROM vector_objects v WHERE v.input_object_id=o.object_id))",[]).map_err(storage_error)?;
        let removed_text=transaction.execute("DELETE FROM text_objects WHERE NOT EXISTS(SELECT 1 FROM chunk_manifests m WHERE m.object_id=text_objects.object_id) AND NOT EXISTS(SELECT 1 FROM vector_objects v WHERE v.input_object_id=text_objects.object_id)",[]).map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
        let stats = storage_stats_connection(&connection, &self.data_dir)?;
        Ok(MaintenanceReport {
            removed_generations: plan.remove_generations.len(),
            removed_text_objects: removed_text,
            removed_vector_objects: removed_vectors,
            filesystem_bytes_released: 0,
            stats,
        })
    }

    /// Explicit offline migration. The original is backed up consistently and
    /// remains untouched until the validated staging database is atomically published.
    /// v1 processes do not understand the cooperative schema lock: stop them first.
    /// Snapshot an authorized store into a NEW directory and migrate only that
    /// copy. The source can remain online: SQLite supplies a consistent snapshot.
    /// Activation is a separate explicit profile change after validation.
    pub fn migrate_schema_copy(
        source: impl AsRef<Path>,
        destination: impl AsRef<Path>,
    ) -> VrgResult<bool> {
        let source = source.as_ref();
        let destination = destination.as_ref();
        if !source.is_absolute()
            || fs::canonicalize(source).map_err(storage_io)? != source
            || !destination.is_absolute()
            || source == destination
        {
            return Err(VrgError::InvalidConfig(
                "migration copy requires distinct canonical absolute directories".into(),
            ));
        }
        let parent = destination
            .parent()
            .ok_or_else(|| VrgError::InvalidConfig("migration destination has no parent".into()))?;
        if fs::canonicalize(parent).map_err(storage_io)? != parent
            || destination
                .components()
                .any(|c| !matches!(c, Component::RootDir | Component::Normal(_)))
        {
            return Err(VrgError::InvalidConfig(
                "migration destination parent must be canonical".into(),
            ));
        }
        let source_database = source.join("vrg.sqlite3");
        if fs::symlink_metadata(&source_database)
            .map_err(storage_io)?
            .file_type()
            .is_symlink()
        {
            return Err(VrgError::SymlinkRejected(
                "source database is a symlink".into(),
            ));
        }
        fs::create_dir(destination).map_err(storage_io)?;
        restrict_permissions(destination, 0o700)?;
        let connection = Connection::open_with_flags(
            &source_database,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(storage_error)?;
        connection
            .execute(
                "VACUUM INTO ?1",
                [destination.join("vrg.sqlite3").to_string_lossy().as_ref()],
            )
            .map_err(storage_error)?;
        drop(connection);
        restrict_permissions(&destination.join("vrg.sqlite3"), 0o600)?;
        migrate_schema(destination)
    }

    pub fn migrate_schema(data_dir: impl AsRef<Path>) -> VrgResult<bool> {
        migrate_schema(data_dir.as_ref())
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

    /// The current writer changed owned derived projections, not an external
    /// authoritative source. Preserve the independent filesystem-event revision.
    pub fn mark_owned_projection_dirty(&self) -> VrgResult<()> {
        self.lock()?.execute("UPDATE index_sync_status SET state='stale', checked_unix_ms=?1, error=NULL WHERE singleton=1 AND state!='indexing'",[now_unix_ms()?]).map_err(storage_error)?;
        Ok(())
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
        let _reader = self.acquire_reader_lease()?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction().map_err(storage_error)?;
        let Some(generation_id) = active_generation(&transaction)? else {
            return Ok(None);
        };
        let snapshot = load_snapshot(&transaction, &generation_id)?;
        transaction.commit().map_err(storage_error)?;
        Ok(Some(snapshot))
    }

    /// Publishes a complete immutable generation in one SQLite transaction.
    /// A pending journal row makes a crash around the transaction recoverable.
    pub fn commit_generation(
        &self,
        snapshot: IndexSnapshot,
        expected_parent: Option<&str>,
    ) -> VrgResult<()> {
        let _publication = named_lock(&self.data_dir, "vrg-gc.lock", true)?;
        self.check_schema()?;
        self.recover_journal_locked()?;
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
        adaptive_test_fault("publish-after-journal")?;

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
        adaptive_test_fault("publish-after-commit")?;

        finish_journal(&connection, journal_id, "COMMITTED")?;
        Ok(())
    }

    /// Reconciles pending journal rows by comparing the active pointer with the
    /// before/after generations. Rows are never guessed through ambiguous state.
    pub fn recover_journal(&self) -> VrgResult<RecoveryReport> {
        // The common no-pending path is passive and does not block readers.
        {
            let connection = self.lock()?;
            let pending: bool = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM journal WHERE state='PREPARING')",
                    [],
                    |r| r.get(0),
                )
                .map_err(storage_error)?;
            if !pending {
                return Ok(RecoveryReport::default());
            }
        }
        // A live publisher's PREPARING row is not a crash. Reconcile only while
        // publication is excluded, otherwise an observer could abort its journal.
        let _publication = named_lock(&self.data_dir, "vrg-gc.lock", true)?;
        self.recover_journal_locked()
    }

    fn recover_journal_locked(&self) -> VrgResult<RecoveryReport> {
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
        let _publication = named_lock(&self.data_dir, "vrg-gc.lock", true)?;
        self.check_schema()?;
        self.recover_journal_locked()?;
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
        self.exact_search_rows_scoped(query, root_ids, limit, None)
    }

    pub fn exact_search_rows_scoped(
        &self,
        query: &str,
        root_ids: &[String],
        limit: usize,
        sources: Option<&BTreeSet<SourceRef>>,
    ) -> VrgResult<Vec<SearchCandidate>> {
        let _reader = self.acquire_reader_lease()?;
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
            .prepare(EXACT_SEARCH_SQL)
            .map_err(storage_error)?;
        let rows = statement
            .query_map(
                params![
                    generation,
                    normalized,
                    path_with_extension,
                    MAX_QUERY_RESULTS as i64,
                    serde_json::to_string(root_ids)
                        .map_err(|e| VrgError::Storage(e.to_string()))?,
                    source_scope_json(sources)?
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
        self.fts_search_rows_scoped(query, root_ids, limit, None)
    }

    pub fn fts_search_rows_scoped(
        &self,
        query: &str,
        root_ids: &[String],
        limit: usize,
        sources: Option<&BTreeSet<SourceRef>>,
    ) -> VrgResult<Vec<SearchCandidate>> {
        let _reader = self.acquire_reader_lease()?;
        let connection = self.lock()?;
        let Some(generation) = active_generation(&connection)? else {
            return Ok(Vec::new());
        };
        let fts_query = fts_query(query);
        if fts_query.is_empty() {
            return Ok(Vec::new());
        }
        let mut statement = connection.prepare(FTS_SEARCH_SQL).map_err(storage_error)?;
        let rows = statement
            .query_map(
                params![
                    fts_query,
                    generation,
                    bounded_limit(limit),
                    serde_json::to_string(root_ids)
                        .map_err(|e| VrgError::Storage(e.to_string()))?,
                    source_scope_json(sources)?
                ],
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
        self.read_vectors(model_id, root_ids, true, None)
    }

    /// Vector metadata for cosine ranking; does not decompress COLD text.
    pub fn vector_catalog_for_model(
        &self,
        model_id: &str,
        root_ids: &[String],
    ) -> VrgResult<Vec<StoredVector>> {
        self.vector_catalog_for_model_scoped(model_id, root_ids, None)
    }

    pub fn vector_catalog_for_model_scoped(
        &self,
        model_id: &str,
        root_ids: &[String],
        sources: Option<&BTreeSet<SourceRef>>,
    ) -> VrgResult<Vec<StoredVector>> {
        self.read_vectors(model_id, root_ids, false, sources)
    }

    fn read_vectors(
        &self,
        model_id: &str,
        root_ids: &[String],
        include_bodies: bool,
        sources: Option<&BTreeSet<SourceRef>>,
    ) -> VrgResult<Vec<StoredVector>> {
        let _reader = self.acquire_reader_lease()?;
        let connection = self.lock()?;
        let Some(generation) = active_generation(&connection)? else {
            return Ok(Vec::new());
        };
        let body = if include_bodies { "c.body" } else { "NULL" };
        let mut statement = connection
            .prepare(&format!(
                "SELECT c.chunk_id, c.root_id, c.relative_path, c.revision, c.ordinal, \
                        c.start_byte, c.end_byte, {body}, d.title, v.dimensions, v.vector_data \
                 FROM vectors v JOIN chunks c \
                   ON v.generation_id = c.generation_id AND v.chunk_id = c.chunk_id \
                 JOIN documents d \
                   ON d.generation_id = c.generation_id \
                  AND d.root_id = c.root_id AND d.relative_path = c.relative_path \
                 WHERE v.generation_id = ?1 AND v.model_id = ?2 \
                   AND (json_array_length(?3)=0 OR c.root_id IN (SELECT value FROM json_each(?3))) \
                   AND (?4 IS NULL OR (c.root_id,c.relative_path) IN \
                     (SELECT json_extract(value,'$.root_id'),json_extract(value,'$.relative_path') FROM json_each(?4))) \
                 ORDER BY c.root_id, c.relative_path, c.ordinal",
            ))
            .map_err(storage_error)?;
        let rows = statement
            .query_map(
                params![
                    generation,
                    model_id,
                    serde_json::to_string(root_ids)
                        .map_err(|e| VrgError::Storage(e.to_string()))?,
                    source_scope_json(sources)?
                ],
                |row| {
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
                },
            )
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
        let _reader = self.acquire_reader_lease()?;
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
        let _reader = self.acquire_reader_lease()?;
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
        let _reader = self.acquire_reader_lease()?;
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
        let connection = self
            .connection
            .lock()
            .map_err(|_| VrgError::Storage("database lock is poisoned".into()))?;
        check_schema_connection(&connection)?;
        Ok(connection)
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
    let existing: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='schema_info')",
            [],
            |r| r.get(0),
        )
        .map_err(storage_error)?;
    if existing {
        check_schema_connection(connection)?;
    }
    connection
        .execute_batch(
            "BEGIN IMMEDIATE;
             CREATE TABLE IF NOT EXISTS schema_info (
                 singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                 version INTEGER NOT NULL
             );
             INSERT OR IGNORE INTO schema_info(singleton, version) VALUES (1, 2);
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
             CREATE TABLE IF NOT EXISTS text_objects (
                 object_id TEXT PRIMARY KEY,
                 domain TEXT NOT NULL,
                 raw_len INTEGER NOT NULL,
                 payload BLOB NOT NULL
             );
             CREATE TABLE IF NOT EXISTS chunk_manifests (
                 generation_id TEXT NOT NULL,
                 chunk_id TEXT NOT NULL,
                 root_id TEXT NOT NULL,
                 relative_path TEXT NOT NULL,
                 revision TEXT NOT NULL,
                 ordinal INTEGER NOT NULL,
                 start_byte INTEGER NOT NULL,
                 end_byte INTEGER NOT NULL,
                 object_id TEXT NOT NULL REFERENCES text_objects(object_id),
                 PRIMARY KEY (generation_id, chunk_id),
                 FOREIGN KEY (generation_id, root_id, relative_path)
                     REFERENCES documents(generation_id, root_id, relative_path)
             );
             CREATE INDEX IF NOT EXISTS chunks_source_idx
                 ON chunk_manifests(generation_id, root_id, relative_path, ordinal);
             CREATE INDEX IF NOT EXISTS chunks_object_idx
                 ON chunk_manifests(generation_id, object_id);
             CREATE VIEW IF NOT EXISTS chunks AS SELECT m.*, o.payload AS body, o.rowid AS object_rowid
                 FROM chunk_manifests m JOIN text_objects o USING(object_id);
             CREATE VIRTUAL TABLE IF NOT EXISTS chunk_fts USING fts5(
                 body, content='', contentless_delete=1, tokenize='unicode61'
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
             CREATE TABLE IF NOT EXISTS vector_objects (
                 object_id TEXT PRIMARY KEY, domain TEXT NOT NULL,
                 input_object_id TEXT NOT NULL REFERENCES text_objects(object_id),
                 model_id TEXT NOT NULL, dimensions INTEGER NOT NULL, vector_data BLOB NOT NULL
             );
             CREATE TABLE IF NOT EXISTS vector_manifests (
                 generation_id TEXT NOT NULL, chunk_id TEXT NOT NULL,
                 model_id TEXT NOT NULL,
                 object_id TEXT NOT NULL REFERENCES vector_objects(object_id),
                 PRIMARY KEY (generation_id,chunk_id,model_id),
                 FOREIGN KEY (generation_id,chunk_id) REFERENCES chunk_manifests(generation_id,chunk_id)
             );
             CREATE VIEW IF NOT EXISTS vectors AS SELECT m.generation_id,m.chunk_id,m.model_id,
                 o.dimensions,o.vector_data FROM vector_manifests m JOIN vector_objects o USING(object_id);
             CREATE TABLE IF NOT EXISTS generation_pins (
                 generation_id TEXT NOT NULL REFERENCES generations(generation_id),
                 pin_id TEXT NOT NULL, PRIMARY KEY(generation_id,pin_id)
             );
             CREATE TABLE IF NOT EXISTS adaptive_state (
                 singleton INTEGER PRIMARY KEY CHECK(singleton=1),generation_id TEXT NOT NULL,
                 proposal_id TEXT NOT NULL,fingerprint TEXT NOT NULL,state_json TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS adaptive_receipts (
                 proposal_id TEXT PRIMARY KEY,fingerprint TEXT NOT NULL
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

    for chunk in &snapshot.chunks {
        let object_id = text_object_id(&chunk.source.root_id, chunk.text.as_bytes());
        let exists: bool = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM text_objects WHERE object_id=?1)",
                [&object_id],
                |r| r.get(0),
            )
            .map_err(storage_error)?;
        let inserted = if exists {
            0
        } else {
            transaction.execute(
            "INSERT INTO text_objects(object_id,domain,raw_len,payload) VALUES(?1,?2,?3,?4)",
            params![object_id, chunk.source.root_id, chunk.text.len() as i64,
                crate::adaptive_codec::encode_frame(chunk.text.as_bytes())?],
        ).map_err(storage_error)?
        };
        if inserted != 0 {
            let rowid = transaction.last_insert_rowid();
            transaction
                .execute(
                    "INSERT INTO chunk_fts(rowid,body) VALUES(?1,?2)",
                    params![rowid, chunk.text],
                )
                .map_err(storage_error)?;
        }
        transaction.execute(
            "INSERT INTO chunk_manifests(generation_id,chunk_id,root_id,relative_path,revision,ordinal,start_byte,end_byte,object_id) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![snapshot.generation_id,chunk.id,chunk.source.root_id,chunk.source.relative_path,chunk.revision,
                i64::from(chunk.ordinal),i64::from(chunk.start_byte),i64::from(chunk.end_byte),object_id],
        ).map_err(storage_error)?;
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

    // Chunk IDs were validated unique before this transaction. Resolve once,
    // rather than scanning C chunks for each of V vectors during publication.
    let chunks_by_id: HashMap<&str, &TextChunk> = snapshot
        .chunks
        .iter()
        .map(|chunk| (chunk.id.as_str(), chunk))
        .collect();
    for vector in &snapshot.vectors {
        let chunk = chunks_by_id
            .get(vector.chunk_id.as_str())
            .ok_or_else(|| VrgError::Storage("vector has no chunk".into()))?;
        let input = text_object_id(&chunk.source.root_id, chunk.text.as_bytes());
        let bytes = encode_vector(&vector.values);
        let object_id = vector_object_id(
            &chunk.source.root_id,
            &input,
            &vector.model_id,
            vector.dimensions,
            &bytes,
        );
        transaction.execute("INSERT OR IGNORE INTO vector_objects(object_id,domain,input_object_id,model_id,dimensions,vector_data) VALUES(?1,?2,?3,?4,?5,?6)",
            params![object_id,chunk.source.root_id,input,vector.model_id,i64::from(vector.dimensions),bytes]).map_err(storage_error)?;
        transaction.execute("INSERT INTO vector_manifests(generation_id,chunk_id,model_id,object_id) VALUES(?1,?2,?3,?4)",
            params![snapshot.generation_id,vector.chunk_id,vector.model_id,object_id]).map_err(storage_error)?;
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
        text: match row.get_ref(7)? {
            rusqlite::types::ValueRef::Text(bytes) => {
                String::from_utf8(bytes.to_vec()).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        7,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?
            }
            rusqlite::types::ValueRef::Blob(bytes) => {
                let raw = crate::adaptive_codec::decode_frame(bytes).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        7,
                        rusqlite::types::Type::Blob,
                        Box::new(e),
                    )
                })?;
                String::from_utf8(raw).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        7,
                        rusqlite::types::Type::Blob,
                        Box::new(e),
                    )
                })?
            }
            rusqlite::types::ValueRef::Null => String::new(),
            _ => {
                return Err(rusqlite::Error::InvalidColumnType(
                    7,
                    "body".into(),
                    rusqlite::types::Type::Null,
                ));
            }
        },
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

fn text_object_id(domain: &str, bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(b"vrg-text-v1\0");
    digest.update((domain.len() as u64).to_le_bytes());
    digest.update(domain.as_bytes());
    digest.update(bytes);
    hex::encode(digest.finalize())
}

fn vector_object_id(
    domain: &str,
    input: &str,
    model: &str,
    dimensions: u32,
    bytes: &[u8],
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"vrg-vector-v1\0");
    for item in [domain, input, model] {
        digest.update((item.len() as u64).to_le_bytes());
        digest.update(item.as_bytes());
    }
    digest.update(dimensions.to_le_bytes());
    digest.update(bytes);
    hex::encode(digest.finalize())
}

fn check_schema_connection(connection: &Connection) -> VrgResult<()> {
    let version: i64 = connection
        .query_row(
            "SELECT version FROM schema_info WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .map_err(storage_error)?;
    if version != SCHEMA_VERSION {
        return Err(VrgError::Storage(format!(
            "unsupported derived-store schema version {version}; explicit isolated migration required"
        )));
    }
    Ok(())
}

fn named_lock(directory: &Path, name: &str, exclusive: bool) -> VrgResult<File> {
    let path = directory.join(name);
    if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(VrgError::SymlinkRejected(
            "derived lock cannot be a symlink".into(),
        ));
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
    }
    let file = options.open(&path).map_err(storage_io)?;
    let result = if exclusive {
        file.try_lock()
    } else {
        file.try_lock_shared()
    };
    match result {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => Err(VrgError::Conflict(format!(
            "derived-store {} lease is busy",
            if exclusive { "exclusive" } else { "reader" }
        ))),
        Err(std::fs::TryLockError::Error(error)) => Err(storage_io(error)),
    }
}

fn schema_lock(directory: &Path, exclusive: bool) -> VrgResult<File> {
    named_lock(directory, "vrg-schema.lock", exclusive)
}

fn maintenance_plan_connection(
    connection: &Connection,
    retain: usize,
) -> VrgResult<MaintenancePlan> {
    if !(2..=10_000).contains(&retain) {
        return Err(VrgError::InvalidConfig(
            "retention requires 2..=10000 recent generations".into(),
        ));
    }
    let current = active_generation(connection)?;
    let mut generations=connection.prepare("SELECT generation_id,parent_id FROM generations ORDER BY created_unix_ms DESC,rowid DESC").map_err(storage_error)?;
    let generations = generations
        .query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
        })
        .map_err(storage_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_error)?;
    let mut protected = BTreeSet::new();
    protected.extend(generations.iter().take(retain).map(|(id, _)| id.clone()));
    if let Some(current) = &current {
        protected.insert(current.clone());
        if let Some((_, Some(parent))) = generations.iter().find(|(id, _)| id == current) {
            protected.insert(parent.clone());
        }
    }
    let mut protection_rows = Vec::new();
    for sql in [
        "SELECT generation_id,pin_id FROM generation_pins ORDER BY generation_id,pin_id",
        "SELECT before_generation,after_generation FROM journal WHERE state='PREPARING' ORDER BY journal_id",
        "SELECT before_generation,after_generation FROM journal WHERE state='COMMITTED' ORDER BY journal_id DESC LIMIT 1",
        "SELECT generation_id,proposal_id FROM adaptive_state ORDER BY singleton",
    ] {
        let mut statement = connection.prepare(sql).map_err(storage_error)?;
        let rows = statement
            .query_map([], |r| {
                Ok((r.get::<_, Option<String>>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)?;
        for (first, second) in rows {
            if let Some(first) = &first {
                protected.insert(first.clone());
            }
            if sql.contains("journal") {
                protected.insert(second.clone());
            }
            protection_rows.push((first, second));
        }
    }
    let state: Option<String> = connection
        .query_row(
            "SELECT state_json FROM adaptive_state WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(storage_error)?;
    if let Some(state) = state {
        let memories: Vec<crate::adaptive::StoredMemory> =
            serde_json::from_str(&state).map_err(|e| {
                VrgError::Storage(format!(
                    "invalid adaptive references prevent maintenance: {e}"
                ))
            })?;
        for span in memories.iter().flat_map(|memory| &memory.sources) {
            let source = crate::adaptive::parse_source_id(&span.source_id).ok_or_else(|| {
                VrgError::Storage("invalid adaptive source identity prevents maintenance".into())
            })?;
            let generation:Option<String>=connection.query_row("SELECT d.generation_id FROM documents d JOIN generations g USING(generation_id) WHERE d.root_id=?1 AND d.relative_path=?2 AND d.revision=?3 ORDER BY g.created_unix_ms DESC,g.rowid DESC LIMIT 1",params![source.root_id,source.relative_path,span.revision_sha256],|r|r.get(0)).optional().map_err(storage_error)?;
            if let Some(generation) = generation {
                protected.insert(generation);
            }
        }
    }
    let fingerprint = hex::encode(Sha256::digest(
        serde_json::to_vec(&(generations.clone(), protection_rows))
            .map_err(|e| VrgError::Storage(e.to_string()))?,
    ));
    let remove_generations = generations
        .into_iter()
        .map(|(id, _)| id)
        .filter(|id| !protected.contains(id))
        .take(32)
        .collect();
    Ok(MaintenancePlan {
        expected_generation: current,
        retain,
        remove_generations,
        protection_fingerprint: fingerprint,
    })
}

fn retained_source_revisions_connection(
    connection: &Connection,
    excluded_generations: &[String],
) -> VrgResult<Vec<(SourceRef, String)>> {
    let excluded = serde_json::to_string(excluded_generations)
        .map_err(|e| VrgError::Storage(e.to_string()))?;
    let mut statement=connection.prepare("SELECT DISTINCT root_id,relative_path,revision FROM documents WHERE generation_id NOT IN (SELECT value FROM json_each(?1)) ORDER BY root_id,relative_path,revision").map_err(storage_error)?;
    let mut revisions = statement
        .query_map([excluded], |r| {
            Ok((
                SourceRef {
                    root_id: r.get(0)?,
                    relative_path: r.get(1)?,
                },
                r.get::<_, String>(2)?,
            ))
        })
        .map_err(storage_error)?
        .collect::<Result<BTreeSet<_>, _>>()
        .map_err(storage_error)?;
    let state: Option<String> = connection
        .query_row(
            "SELECT state_json FROM adaptive_state WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(storage_error)?;
    if let Some(state) = state {
        let memories: Vec<crate::adaptive::StoredMemory> =
            serde_json::from_str(&state).map_err(|e| {
                VrgError::Storage(format!(
                    "invalid adaptive references prevent archive maintenance: {e}"
                ))
            })?;
        for span in memories.into_iter().flat_map(|memory| memory.sources) {
            let source = crate::adaptive::parse_source_id(&span.source_id).ok_or_else(|| {
                VrgError::Storage(
                    "invalid adaptive source identity prevents archive maintenance".into(),
                )
            })?;
            revisions.insert((source, span.revision_sha256));
        }
    }
    Ok(revisions.into_iter().collect())
}

fn storage_stats_connection(connection: &Connection, directory: &Path) -> VrgResult<StorageStats> {
    let nonnegative = |value: i64| -> VrgResult<u64> {
        u64::try_from(value)
            .map_err(|_| VrgError::Storage("negative SQLite storage statistic".into()))
    };
    let count = |table: &str| -> VrgResult<u64> {
        let value: i64 = connection
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .map_err(storage_error)?;
        nonnegative(value)
    };
    let sum = |sql: &str| -> VrgResult<u64> {
        let value: i64 = connection
            .query_row(sql, [], |r| r.get(0))
            .map_err(storage_error)?;
        nonnegative(value)
    };
    let size = |name: &str| {
        fs::metadata(directory.join(name))
            .map(|m| m.len())
            .unwrap_or(0)
    };
    let page_size = nonnegative(
        connection
            .pragma_query_value(None, "page_size", |r| r.get::<_, i64>(0))
            .map_err(storage_error)?,
    )?;
    let free = nonnegative(
        connection
            .pragma_query_value(None, "freelist_count", |r| r.get::<_, i64>(0))
            .map_err(storage_error)?,
    )?;
    Ok(StorageStats {
        schema_version: SCHEMA_VERSION,
        generations: count("generations")?,
        documents: count("documents")?,
        chunks: count("chunk_manifests")?,
        text_objects: count("text_objects")?,
        vector_objects: count("vector_objects")?,
        logical_text_bytes: sum("SELECT coalesce(sum(raw_len),0) FROM text_objects")?,
        payload_bytes: sum("SELECT coalesce(sum(length(payload)),0) FROM text_objects")?
            + sum("SELECT coalesce(sum(length(vector_data)),0) FROM vector_objects")?,
        database_bytes: size("vrg.sqlite3"),
        wal_bytes: size("vrg.sqlite3-wal"),
        shared_memory_bytes: size("vrg.sqlite3-shm"),
        freelist_bytes: free
            .checked_mul(page_size)
            .ok_or_else(|| VrgError::Storage("freelist byte statistic overflow".into()))?,
    })
}

fn migrate_schema(directory: &Path) -> VrgResult<bool> {
    if !directory.is_absolute() || fs::canonicalize(directory).map_err(storage_io)? != directory {
        return Err(VrgError::InvalidConfig(
            "migration requires a canonical absolute isolated directory".into(),
        ));
    }
    let _schema = schema_lock(directory, true)?;
    let _sync = named_lock(directory, SYNC_LOCK_FILE, true)?;
    let _gc = named_lock(directory, "vrg-gc.lock", true)?;
    let database = directory.join("vrg.sqlite3");
    if fs::symlink_metadata(&database)
        .map_err(storage_io)?
        .file_type()
        .is_symlink()
    {
        return Err(VrgError::SymlinkRejected(
            "migration database is a symlink".into(),
        ));
    }
    let source = Connection::open(&database).map_err(storage_error)?;
    let version: i64 = source
        .query_row(
            "SELECT version FROM schema_info WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .map_err(storage_error)?;
    if version == SCHEMA_VERSION {
        return Ok(false);
    }
    if version != 1 {
        return Err(VrgError::Storage(format!(
            "unsupported migration source schema {version}"
        )));
    }
    // A crash leaves its consistent backup/staging inspectable; a retry uses a
    // fresh private attempt directory rather than trusting or overwriting them.
    let mut attempt = None;
    for slot in 0..32 {
        let path = directory.join(format!("schema-migration-{slot:02}"));
        match fs::create_dir(&path) {
            Ok(()) => {
                restrict_permissions(&path, 0o700)?;
                attempt = Some(path);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(storage_io(error)),
        }
    }
    let attempt = attempt.ok_or_else(|| {
        VrgError::Conflict(
            "migration attempts exhausted; inspect retained backups before retry".into(),
        )
    })?;
    let backup = attempt.join("schema1.rollback.sqlite3");
    let stage = attempt.join("schema2.staging.sqlite3");
    // SQLite performs checkpoint/removal of WAL itself. Never unlink live sidecars.
    let mode: String = source
        .query_row("PRAGMA journal_mode=DELETE", [], |r| r.get(0))
        .map_err(storage_error)?;
    if !mode.eq_ignore_ascii_case("delete") {
        return Err(VrgError::Conflict(
            "cannot migrate while WAL readers/writers are active".into(),
        ));
    }
    // Exclusive locking mode retains SQLite's lock after COMMIT; take it before
    // the backup so an uncooperative v1 writer cannot publish in the gap.
    source
        .execute_batch("PRAGMA locking_mode=EXCLUSIVE; BEGIN EXCLUSIVE; COMMIT;")
        .map_err(storage_error)?;
    source
        .execute("VACUUM INTO ?1", [backup.to_string_lossy().as_ref()])
        .map_err(storage_error)?;
    restrict_permissions(&backup, 0o600)?;
    adaptive_test_fault("migration-before-stage")?;
    let legacy = Connection::open(&backup).map_err(storage_error)?;
    let mut candidate = Connection::open(&stage).map_err(storage_error)?;
    candidate
        .pragma_update(None, "foreign_keys", "ON")
        .map_err(storage_error)?;
    initialize_schema(&candidate)?;
    let result = (|| -> VrgResult<()> {
        let mut statement = legacy
            .prepare(
                "SELECT generation_id,parent_id,created_unix_ms FROM generations ORDER BY rowid",
            )
            .map_err(storage_error)?;
        let generations = statement
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })
            .map_err(storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)?;
        drop(statement);
        let transaction = candidate
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        for (generation, parent, created) in generations {
            let snapshot = load_snapshot(&legacy, &generation)?;
            persist_snapshot(&transaction, &snapshot, parent.as_deref())?;
            transaction
                .execute(
                    "UPDATE generations SET created_unix_ms=?1 WHERE generation_id=?2",
                    params![created, generation],
                )
                .map_err(storage_error)?;
        }
        adaptive_test_fault("migration-stage-writing")?;
        transaction.commit().map_err(storage_error)?;
        candidate
            .execute(
                "ATTACH DATABASE ?1 AS legacy",
                [backup.to_string_lossy().as_ref()],
            )
            .map_err(storage_error)?;
        for table in ["active_generation", "journal", "session_events"] {
            candidate
                .execute(
                    &format!("INSERT INTO {table} SELECT * FROM legacy.{table}"),
                    [],
                )
                .map_err(storage_error)?;
        }
        for table in ["index_sync_status", "source_change_revision"] {
            candidate
                .execute(&format!("DELETE FROM {table}"), [])
                .map_err(storage_error)?;
            candidate
                .execute(
                    &format!("INSERT INTO {table} SELECT * FROM legacy.{table}"),
                    [],
                )
                .map_err(storage_error)?;
        }
        candidate
            .execute(
                "INSERT INTO index_sync_events SELECT * FROM legacy.index_sync_events",
                [],
            )
            .map_err(storage_error)?;
        candidate
            .execute_batch("DETACH DATABASE legacy;")
            .map_err(storage_error)?;
        let integrity: String = candidate
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .map_err(storage_error)?;
        if integrity != "ok" {
            return Err(VrgError::Recovery(
                "migration integrity check failed".into(),
            ));
        }
        let mut check = candidate
            .prepare("PRAGMA foreign_key_check")
            .map_err(storage_error)?;
        if check
            .query([])
            .map_err(storage_error)?
            .next()
            .map_err(storage_error)?
            .is_some()
        {
            return Err(VrgError::Recovery(
                "migration foreign-key check failed".into(),
            ));
        }
        Ok(())
    })();
    drop(candidate);
    drop(legacy);
    if let Err(error) = result {
        let _ = fs::remove_file(&stage);
        return Err(error);
    }
    restrict_permissions(&stage, 0o600)?;
    File::open(&stage)
        .map_err(storage_io)?
        .sync_all()
        .map_err(storage_io)?;
    adaptive_test_fault("migration-before-publish")?;
    fs::rename(&stage, &database).map_err(storage_io)?;
    adaptive_test_fault("migration-after-publish")?;
    File::open(directory)
        .map_err(storage_io)?
        .sync_all()
        .map_err(storage_io)?;
    drop(source);
    Ok(true)
}

// Explicit thread-local fault injection exists only in unit-test builds. Normal
// binaries have no environment variable or runtime switch for these faults.
#[cfg(test)]
thread_local! {static ADAPTIVE_FAULT:std::cell::Cell<Option<(&'static str,bool)>>=const {std::cell::Cell::new(None)};}
#[cfg(test)]
fn adaptive_test_fault(stage: &str) -> VrgResult<()> {
    ADAPTIVE_FAULT.with(|fault| {
        if let Some((expected, enospc)) = fault.get() {
            if stage == expected {
                fault.set(None);
                return Err(if enospc {
                    VrgError::Io(std::io::Error::from_raw_os_error(28))
                } else {
                    VrgError::Recovery(format!("synthetic interruption at {stage}"))
                });
            }
        }
        Ok(())
    })
}
#[cfg(not(test))]
#[inline]
fn adaptive_test_fault(_stage: &str) -> VrgResult<()> {
    Ok(())
}

#[cfg(test)]
mod adaptive_storage_fault_tests {
    use super::*;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path =
                std::env::temp_dir().join(format!("vrg-am-fault-{}-{nonce}", std::process::id()));
            fs::create_dir_all(&path).unwrap();
            Self(fs::canonicalize(path).unwrap())
        }
        fn legacy(&self) -> PathBuf {
            let path = self.0.join("legacy");
            fs::create_dir(&path).unwrap();
            let c = Connection::open(path.join("vrg.sqlite3")).unwrap();
            c.execute_batch(include_str!("../tests/fixtures/adaptive/schema1.sql"))
                .unwrap();
            c.execute(
                "INSERT INTO generations VALUES('original',NULL,1,0,0,1)",
                [],
            )
            .unwrap();
            c.execute("INSERT INTO active_generation VALUES(1,'original')", [])
                .unwrap();
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            ADAPTIVE_FAULT.with(|f| f.set(None));
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn snapshot(id: &str) -> IndexSnapshot {
        IndexSnapshot {
            generation_id: id.into(),
            documents: Vec::new(),
            chunks: Vec::new(),
            edges: Vec::new(),
            vectors: Vec::new(),
        }
    }
    fn version(path: &Path) -> i64 {
        Connection::open(path.join("vrg.sqlite3"))
            .unwrap()
            .query_row("SELECT version FROM schema_info", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn am_t10_journal_interruption_before_after_publish_and_recovery_idempotence() {
        for (stage, published) in [
            ("publish-after-journal", false),
            ("publish-after-commit", true),
        ] {
            let f = Fixture::new();
            let store = DerivedStore::open(f.0.join("derived")).unwrap();
            store.commit_generation(snapshot("before"), None).unwrap();
            ADAPTIVE_FAULT.with(|fault| fault.set(Some((stage, false))));
            assert!(
                store
                    .commit_generation(snapshot("after"), Some("before"))
                    .is_err()
            );
            drop(store);
            let store = DerivedStore::open(f.0.join("derived")).unwrap();
            let report = store.recover_journal().unwrap();
            assert_eq!(report.committed, usize::from(published));
            assert_eq!(report.aborted, usize::from(!published));
            assert_eq!(
                store.current_generation_id().unwrap().as_deref(),
                Some(if published { "after" } else { "before" })
            );
            let again = store.recover_journal().unwrap();
            assert_eq!(again.committed + again.aborted, 0);
        }
    }

    #[test]
    fn am_t10_two_writers_reader_conflict_and_schema_handshake() {
        let f = Fixture::new();
        let path = f.0.join("derived");
        let first = DerivedStore::open(&path).unwrap();
        let second = DerivedStore::open(&path).unwrap();
        first.commit_generation(snapshot("before"), None).unwrap();
        let reader = first.acquire_reader_lease().unwrap();
        assert!(
            second
                .commit_generation(snapshot("busy"), Some("before"))
                .is_err()
        );
        drop(reader);
        second
            .commit_generation(snapshot("after"), Some("before"))
            .unwrap();
        assert!(
            first
                .commit_generation(snapshot("stale"), Some("before"))
                .is_err()
        );
        let conn = Connection::open(path.join("vrg.sqlite3")).unwrap();
        conn.execute("UPDATE schema_info SET version=99", [])
            .unwrap();
        assert!(
            first
                .commit_generation(snapshot("incompatible"), Some("after"))
                .is_err()
        );
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM generations WHERE generation_id='incompatible'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
        conn.execute("UPDATE schema_info SET version=2", [])
            .unwrap();
        assert_eq!(
            first.current_generation_id().unwrap().as_deref(),
            Some("after")
        );
    }

    #[test]
    fn am_t10_migration_before_after_publish_retries_and_simulated_enospc() {
        for (stage, enospc, published) in [
            ("migration-before-stage", true, false),
            ("migration-stage-writing", true, false),
            ("migration-before-publish", false, false),
            ("migration-after-publish", false, true),
        ] {
            let f = Fixture::new();
            let path = f.legacy();
            ADAPTIVE_FAULT.with(|fault| fault.set(Some((stage, enospc))));
            let error = DerivedStore::migrate_schema(&path).unwrap_err();
            if enospc {
                assert!(matches!(error,VrgError::Io(ref io) if io.raw_os_error()==Some(28)));
            }
            assert_eq!(version(&path), if published { 2 } else { 1 });
            let original =
                Connection::open(path.join("schema-migration-00/schema1.rollback.sqlite3"))
                    .unwrap();
            let id: String = original
                .query_row("SELECT generation_id FROM active_generation", [], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(id, "original");
            drop(original);
            assert_eq!(DerivedStore::migrate_schema(&path).unwrap(), !published);
            assert!(!DerivedStore::migrate_schema(&path).unwrap());
            let store = DerivedStore::open(&path).unwrap();
            assert_eq!(
                store.current_generation_id().unwrap().as_deref(),
                Some("original")
            );
        }
    }
}

/// NULL means unscoped; Some(empty) deliberately admits no source tuples.
fn source_scope_json(sources: Option<&BTreeSet<SourceRef>>) -> VrgResult<Option<String>> {
    sources
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| VrgError::Storage(error.to_string()))
}
