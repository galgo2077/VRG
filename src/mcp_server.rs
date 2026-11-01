use std::{path::Path, sync::Arc};

use rmcp::{
    ErrorData as McpError, ServerHandler, ServiceExt, handler::server::wrapper::Parameters,
    schemars::JsonSchema, tool, tool_handler, tool_router, transport::stdio,
};
use serde::Deserialize;

use crate::{
    VrgEngine, VrgError, VrgResult,
    contracts::{SearchRequest, SourceRef},
};

const MAX_QUERY_BYTES: usize = 4_096;
const MAX_LIMIT: usize = 50;
const MAX_ROOT_FILTERS: usize = 16;
const MAX_ROOT_ID_BYTES: usize = 128;
const MAX_RELATIVE_PATH_BYTES: usize = 2_048;
const MAX_SOURCE_BYTES: usize = 32 * 1_024;
const MAX_SEARCH_SNIPPET_BYTES: usize = 512;

#[derive(Clone)]
struct VrgTools {
    engine: Arc<VrgEngine>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SearchArgs {
    query: String,
    limit: Option<usize>,
    root_ids: Option<Vec<String>>,
    semantic: Option<bool>,
    graph_hops: Option<u8>,
    allow_stale: Option<bool>,
    /// Hard aggregate serialized response bytes, including provenance and accounting.
    max_response_bytes: Option<usize>,
    /// UTF8-bytes/4 estimated units, not provider tokens.
    max_context_units: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReadSourceArgs {
    root_id: String,
    /// Relative path within the configured root; absolute paths are rejected.
    relative_path: String,
    max_bytes: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NeighborsArgs {
    root_id: String,
    /// Relative path within the configured root; absolute paths are rejected.
    relative_path: String,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MemoryEventsArgs {
    limit: Option<usize>,
}

#[tool_router]
impl VrgTools {
    #[tool(
        name = "vrg_status",
        description = "Return the local VRG module and index status."
    )]
    fn status(&self) -> Result<String, McpError> {
        let mut status =
            serde_json::to_value(self.engine.status()).map_err(|_| internal_error())?;
        status["adaptive_memory"] = self
            .engine
            .adaptive_status()
            .map_err(|_| internal_error())?;
        serde_json::to_string(&status).map_err(|_| internal_error())
    }

    #[tool(
        name = "vrg_search",
        description = "Search configured roots and return attributed results with bounded snippets."
    )]
    fn search(&self, Parameters(args): Parameters<SearchArgs>) -> Result<String, McpError> {
        if args.query.trim().is_empty() || args.query.len() > MAX_QUERY_BYTES {
            return Err(invalid_params());
        }

        let limit = args.limit.unwrap_or(10);
        if !(1..=MAX_LIMIT).contains(&limit) {
            return Err(invalid_params());
        }

        if let Some(root_ids) = &args.root_ids {
            if root_ids.len() > MAX_ROOT_FILTERS
                || root_ids.iter().any(|id| {
                    id.trim().is_empty() || id.len() > MAX_ROOT_ID_BYTES || id.contains('\0')
                })
            {
                return Err(invalid_params());
            }
        }

        let graph_hops = args.graph_hops.unwrap_or(0);
        if graph_hops > 2 {
            return Err(invalid_params());
        }

        let query = args.query;
        let request = SearchRequest {
            query: query.clone(),
            limit,
            root_ids: args.root_ids,
            use_semantic: args.semantic.unwrap_or(true),
            graph_hops,
        };
        let policy = self.engine.adaptive_policy();
        if policy.enabled || args.max_response_bytes.is_some() || args.max_context_units.is_some() {
            let budget = crate::adaptive::Budget {
                max_response_bytes: args.max_response_bytes.unwrap_or(policy.max_response_bytes),
                max_context_units: args.max_context_units.unwrap_or(policy.max_context_units),
            };
            let response = self
                .engine
                .search_budgeted_with_policy(request, budget, args.allow_stale.unwrap_or(false))
                .map_err(|error| match error {
                    VrgError::InvalidConfig(_) => invalid_params(),
                    _ => internal_error(),
                })?;
            return serde_json::to_string(&response).map_err(|_| internal_error());
        }
        let mut response = self
            .engine
            .search_with_policy(request, args.allow_stale.unwrap_or(false))
            .map_err(|_| internal_error())?;
        for hit in &mut response.hits {
            hit.snippet = bounded_search_snippet(&hit.snippet, &query);
        }
        serde_json::to_string(&response).map_err(|_| internal_error())
    }

    #[tool(
        name = "vrg_read_source",
        description = "Read a bounded Markdown source by configured root ID and relative path."
    )]
    fn read_source(
        &self,
        Parameters(args): Parameters<ReadSourceArgs>,
    ) -> Result<String, McpError> {
        validate_root_id(&args.root_id)?;
        validate_relative_path(&args.relative_path)?;

        let max_bytes = args.max_bytes.unwrap_or(16 * 1_024);
        if !(1..=MAX_SOURCE_BYTES).contains(&max_bytes) {
            return Err(invalid_params());
        }

        let document = self
            .engine
            .read_source(&args.root_id, &args.relative_path, max_bytes)
            .map_err(|error| match error {
                VrgError::AccessDenied(_)
                | VrgError::PathEscape(_)
                | VrgError::SymlinkRejected(_)
                | VrgError::InvalidConfig(_) => invalid_params(),
                _ => internal_error(),
            })?;
        serde_json::to_string(&document).map_err(|_| internal_error())
    }

    #[tool(
        name = "vrg_graph_neighbors",
        description = "Return explicit Obsidian graph relations for one configured source."
    )]
    fn graph_neighbors(
        &self,
        Parameters(args): Parameters<NeighborsArgs>,
    ) -> Result<String, McpError> {
        validate_root_id(&args.root_id)?;
        validate_relative_path(&args.relative_path)?;

        let limit = args.limit.unwrap_or(32);
        if !(1..=MAX_LIMIT).contains(&limit) {
            return Err(invalid_params());
        }
        let source = SourceRef {
            root_id: args.root_id,
            relative_path: args.relative_path,
        };
        let edges = self
            .engine
            .neighbors(&source, limit)
            .map_err(|_| internal_error())?;
        serde_json::to_string(&edges).map_err(|_| internal_error())
    }

    #[tool(
        name = "vrg_session_events",
        description = "Return opt-in observed session and turn metadata for this project. It contains no conversation summaries or decisions."
    )]
    fn session_events(
        &self,
        Parameters(args): Parameters<MemoryEventsArgs>,
    ) -> Result<String, McpError> {
        let limit = args.limit.unwrap_or(20);
        if !(1..=100).contains(&limit) {
            return Err(invalid_params());
        }
        let cwd = std::env::current_dir()
            .map_err(|_| internal_error())?
            .to_string_lossy()
            .to_string();
        let events = self
            .engine
            .recent_session_events(&cwd, limit)
            .map_err(|_| internal_error())?;
        serde_json::to_string(&events).map_err(|_| internal_error())
    }
}

#[tool_handler(
    name = "vrg",
    version = "0.1.0",
    instructions = "VRG is read-only. When prior project knowledge is needed, search the approved root and check source revision and freshness. Work normally. At completion, decide whether verified durable knowledge is new; if so, update only an authorized operational document using normal file tools, then check its revision and synchronization through VRG. Do not save conversation text or create routine per-turn notes."
)]
impl ServerHandler for VrgTools {}

/// Serve the read-only local MCP surface over standard input/output.
/// The configured monitor and startup reconciliation run before serving; MCP
/// itself offers no source-note write operation.
pub async fn serve(engine: VrgEngine) -> VrgResult<()> {
    engine.start_change_monitor()?;
    engine.reconcile_at_startup()?;
    let tools = VrgTools {
        engine: Arc::new(engine),
    };
    let service = tools
        .serve(stdio())
        .await
        .map_err(|_| VrgError::Mcp("failed to start stdio MCP service".into()))?;
    service
        .waiting()
        .await
        .map(|_| ())
        .map_err(|_| VrgError::Mcp("stdio MCP service stopped with an error".into()))
}

fn validate_root_id(root_id: &str) -> Result<(), McpError> {
    if root_id.trim().is_empty() || root_id.len() > MAX_ROOT_ID_BYTES || root_id.contains('\0') {
        return Err(invalid_params());
    }
    Ok(())
}

fn validate_relative_path(relative_path: &str) -> Result<(), McpError> {
    if relative_path.is_empty()
        || relative_path.len() > MAX_RELATIVE_PATH_BYTES
        || relative_path.contains('\0')
        || relative_path.contains('\\')
    {
        return Err(invalid_params());
    }

    let path = Path::new(relative_path);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(invalid_params());
    }
    Ok(())
}

fn bounded_search_snippet(text: &str, query: &str) -> String {
    crate::adaptive::bounded_snippet(text, query)
}

#[cfg(test)]
mod snippet_tests {
    use super::{MAX_SEARCH_SNIPPET_BYTES, bounded_search_snippet};

    #[test]
    fn search_snippets_are_bounded_near_a_query_term() {
        let text = format!(
            "{}matching project context {}",
            "a".repeat(1_000),
            "z".repeat(1_000)
        );
        let snippet = bounded_search_snippet(&text, "matching context");

        assert!(snippet.len() <= MAX_SEARCH_SNIPPET_BYTES);
        assert!(snippet.contains("matching"));
        assert!(!snippet.contains(&"z".repeat(600)));
    }

    #[test]
    fn search_snippet_truncation_preserves_utf8_boundaries() {
        let text = format!("{} contexto útil {}", "á🙂".repeat(200), "ñ".repeat(200));
        let snippet = bounded_search_snippet(&text, "útil");

        assert!(snippet.len() <= MAX_SEARCH_SNIPPET_BYTES);
        assert!(snippet.contains("útil"));
    }

    #[test]
    fn search_snippet_prefers_window_with_more_query_terms() {
        let text = format!(
            "NACAR582 old {} NACAR582 acumulado 165 segundos",
            "x".repeat(900)
        );
        let snippet = bounded_search_snippet(&text, "NACAR582 acumulado");
        assert!(snippet.contains("acumulado 165"));
        assert!(snippet.len() <= MAX_SEARCH_SNIPPET_BYTES);
    }
}

fn invalid_params() -> McpError {
    McpError::invalid_params("invalid or out-of-policy VRG request", None)
}

fn internal_error() -> McpError {
    McpError::internal_error("VRG operation failed", None)
}
