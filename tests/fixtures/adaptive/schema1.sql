BEGIN IMMEDIATE;
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
             COMMIT;
