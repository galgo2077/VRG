use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub struct SourceRef {
    pub root_id: String,
    /// UTF-8 path relative to the configured root; never an absolute path.
    pub relative_path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceDocument {
    pub source: SourceRef,
    /// SHA-256 of the original source bytes.
    pub revision: String,
    pub title: String,
    pub raw_markdown: String,
    pub body_text: String,
    pub frontmatter: BTreeMap<String, serde_json::Value>,
    pub links: Vec<LinkRef>,
    pub modified_unix_ms: Option<u64>,
}

/// Source metadata persisted in the derived index; full raw note text is not stored here.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredDocument {
    pub source: SourceRef,
    pub revision: String,
    pub title: String,
    pub frontmatter: BTreeMap<String, serde_json::Value>,
    pub modified_unix_ms: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LinkRef {
    pub target: String,
    pub alias: Option<String>,
    pub heading: Option<String>,
    pub kind: LinkKind,
    pub start_byte: u32,
    pub end_byte: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkKind {
    Wiki,
    Embed,
    Markdown,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TextChunk {
    pub id: String,
    pub source: SourceRef,
    pub revision: String,
    pub ordinal: u32,
    /// UTF-8 byte offsets into `SourceDocument::body_text`.
    pub start_byte: u32,
    pub end_byte: u32,
    pub text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GraphEdge {
    pub from: SourceRef,
    pub to: Option<SourceRef>,
    pub unresolved_target: Option<String>,
    pub kind: LinkKind,
    pub start_byte: u32,
    pub end_byte: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VectorEmbedding {
    pub chunk_id: String,
    pub model_id: String,
    pub dimensions: u32,
    pub values: Vec<f32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IndexSnapshot {
    pub generation_id: String,
    pub documents: Vec<StoredDocument>,
    pub chunks: Vec<TextChunk>,
    pub edges: Vec<GraphEdge>,
    pub vectors: Vec<VectorEmbedding>,
}

#[derive(Clone, Debug)]
pub struct SearchCandidate {
    pub chunk: TextChunk,
    pub title: String,
    pub score: f32,
    pub kinds: Vec<MatchKind>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchRequest {
    pub query: String,
    pub limit: usize,
    pub root_ids: Option<Vec<String>>,
    pub use_semantic: bool,
    pub graph_hops: u8,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RetrievalHit {
    pub source: SourceRef,
    pub revision: String,
    pub chunk_id: String,
    pub snippet: String,
    pub start_byte: u32,
    pub end_byte: u32,
    pub score: f32,
    pub matches: Vec<MatchKind>,
    pub relations: Vec<GraphEdge>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchKind {
    ExactPath,
    ExactTitle,
    FullText,
    Semantic,
    GraphExpansion,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetrievalMode {
    Hybrid,
    TextOnly,
    DegradedTextOnly,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchResponse {
    pub generation_id: Option<String>,
    pub sync_state: SearchSyncState,
    pub sync_reason: Option<String>,
    pub mode: RetrievalMode,
    pub degradation_reason: Option<String>,
    pub hits: Vec<RetrievalHit>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchSyncState {
    Ready,
    Stale,
    SyncPending,
    Degraded,
    Error,
    Disabled,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IndexReport {
    pub generation_id: Option<String>,
    pub roots_scanned: usize,
    pub files_seen: usize,
    pub indexed: usize,
    pub unchanged: usize,
    pub excluded: usize,
    pub failed: usize,
    pub semantic_status: ModuleStatus,
    #[serde(default)]
    pub work: IndexWork,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct IndexWork {
    pub event_paths: usize,
    pub full_scans: usize,
    pub source_files_read: usize,
    pub source_read_operations: usize,
    pub chunks_embedded: usize,
    pub vectors_reused_by_content: usize,
    #[serde(default)]
    pub policy_manifest_reads: usize,
    #[serde(default)]
    pub policy_cache_hits: usize,
    #[serde(default)]
    pub head_reads: usize,
    #[serde(default)]
    pub head_cache_hits: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", content = "reason", rename_all = "snake_case")]
pub enum ModuleStatus {
    Ready,
    Disabled,
    Degraded(String),
    Failed(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EngineStatus {
    /// Effective process and profile identity for distinguishing isolated MCP servers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_identity: Option<RuntimeIdentity>,
    pub generation_id: Option<String>,
    pub configured_roots: usize,
    pub text: ModuleStatus,
    pub semantic: ModuleStatus,
    pub graph: ModuleStatus,
    pub storage: ModuleStatus,
    pub note_writes: ModuleStatus,
    pub remote_chatgpt: ModuleStatus,
    /// Result of the latest end-of-turn synchronization check.
    pub index_sync: IndexSyncStatus,
    pub change_monitor: ModuleStatus,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RuntimeIdentity {
    pub config_path: String,
    pub config_sha256: String,
    pub server_executable: String,
    pub server_args: Vec<String>,
    pub server_cwd: String,
    pub selection_environment: BTreeMap<String, Option<String>>,
    pub roots: Vec<RuntimeRootIdentity>,
    pub data_dir: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RuntimeRootIdentity {
    pub id: String,
    pub canonical_path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum IndexSyncStatus {
    Disabled {
        reason: String,
    },
    Stale {
        generation_id: Option<String>,
    },
    Indexing {
        generation_id: Option<String>,
    },
    UpToDate {
        generation_id: Option<String>,
        checked_unix_ms: u64,
    },
    Failed {
        generation_id: Option<String>,
        reason: String,
        failed_unix_ms: u64,
    },
}

#[derive(Clone, Debug)]
pub enum StopSyncOutcome {
    Disabled,
    InProgress,
    SkippedCurrent(IndexSyncStatus),
    Duplicate(IndexSyncStatus),
    Completed {
        report: IndexReport,
        status: IndexSyncStatus,
    },
    Failed(IndexSyncStatus),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionEventRecord {
    pub session_id: String,
    pub turn_id: String,
    pub project_key: String,
    pub event_name: String,
    pub observed_unix_ms: i64,
    pub evidence_state: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WritePlan {
    pub source: SourceRef,
    pub expected_revision: String,
    pub proposed_markdown: String,
    pub reason: String,
}
