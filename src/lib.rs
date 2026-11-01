pub mod auto_sync;
pub mod cli;
pub mod doctor;
pub mod setup;
pub mod setup_codex;
pub mod config;
pub mod contracts;
pub mod error;
pub mod graph;
pub mod indexer;
pub mod mcp_server;
pub mod model_setup;
pub mod retrieval;
pub mod semantic;
pub mod storage;
pub mod text_search;
pub mod vault;

#[cfg(test)]
mod intel_runtime;
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
mod runtime_probe;
mod engine;
mod note_write;

pub use config::VrgConfig;
pub use contracts::{
    EngineStatus, IndexReport, IndexSyncStatus, SearchRequest, SearchResponse, StopSyncOutcome,
};
pub use engine::VrgEngine;
pub use error::{VrgError, VrgResult};
