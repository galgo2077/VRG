pub mod auto_sync;
pub mod cli;
pub mod config;
pub mod contracts;
pub mod doctor;
pub mod error;
pub mod graph;
pub mod indexer;
pub mod mcp_server;
pub mod model_setup;
#[cfg(any(test, all(target_os = "macos", target_arch = "x86_64")))]
mod onnx_source_setup;
pub mod retrieval;
pub mod semantic;
pub mod setup;
pub mod setup_codex;
pub mod storage;
pub mod text_search;
pub mod vault;

mod engine;
#[cfg(test)]
mod intel_runtime;
mod note_write;
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
mod runtime_probe;

pub use config::VrgConfig;
pub use contracts::{
    EngineStatus, IndexReport, IndexSyncStatus, SearchRequest, SearchResponse, StopSyncOutcome,
};
pub use engine::VrgEngine;
pub use error::{VrgError, VrgResult};
