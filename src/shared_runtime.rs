//! Native rmcp HTTP transport. Enrollment is local and explicit; requests never select paths.
use std::{collections::HashMap, fs, path::{Path, PathBuf}, sync::{Arc, Mutex}, time::Duration};
use axum::{body::{Body, to_bytes}, extract::{State, Query}, http::{Request, StatusCode}, response::{IntoResponse, Response}, routing::{any, get, post}, Router};
use hmac::{Hmac, KeyInit, Mac};
use rmcp::transport::streamable_http_server::{StreamableHttpService, StreamableHttpServerConfig, session::local::LocalSessionManager};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tower::ServiceExt;
use crate::{VrgEngine, VrgError, VrgResult, mcp_server::VrgTools, note_write};

type McpService = StreamableHttpService<VrgTools, LocalSessionManager>;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub port: u16,
    pub identity_key: String,
    pub bindings: Vec<BindingConfig>,
    #[serde(default)]
    pub auto_registry_path: Option<PathBuf>,
    #[serde(default)]
    pub auto_discovery_root: Option<PathBuf>,
}
#[derive(Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BindingConfig {
    pub channel: String,
    pub project: String,
    pub config: PathBuf,
    pub config_sha256: String,
    pub token: String,
    #[serde(default)]
    pub managed_note_writes: bool,
}
struct Loaded { service: McpService, engine: Arc<VrgEngine> }
struct Binding { config: BindingConfig, loaded: Mutex<Option<Loaded>> }
struct Runtime {
    authority: String,
    identity_key: String,
    transport: StreamableHttpServerConfig,
    bindings: HashMap<String, Arc<Binding>>,
    dynamic_bindings: Mutex<HashMap<String, Arc<Binding>>>,
    auto_registry_path: Option<PathBuf>,
    auto_discovery_root: Option<PathBuf>,
    permits: Arc<tokio::sync::Semaphore>,
}
fn invalid() -> VrgError { VrgError::InvalidConfig("shared runtime enrollment is invalid or changed".into()) }
fn private_read(path: &Path) -> VrgResult<Vec<u8>> {
    use std::os::unix::fs::MetadataExt;
    let m = fs::symlink_metadata(path)?;
    if !m.is_file() || m.uid() != rustix::process::getuid().as_raw() || m.mode() & 0o077 != 0 || path.canonicalize()? != path { return Err(invalid()); }
    Ok(fs::read(path)?)
}
fn profile_valid(c: &BindingConfig) -> bool {
    private_read(&c.config).is_ok_and(|bytes| {
        hex::encode(Sha256::digest(&bytes)) == c.config_sha256
            && std::str::from_utf8(&bytes).ok().and_then(|s|toml::from_str::<crate::VrgConfig>(s).ok())
                .is_some_and(|config|config.project_memory.is_empty())
    })
}
fn dynamic_binding(state: &Runtime, token: &str) -> Option<Arc<Binding>> {
    let registry_path = state.auto_registry_path.as_ref()?;
    let discovery_root = state.auto_discovery_root.as_ref()?;
    let registry: serde_json::Value = serde_json::from_slice(&private_read(registry_path).ok()?).ok()?;
    if registry.get("schema_version")?.as_u64()? != 1
        || registry.get("discovery_roots")?.as_array()?.as_slice()
            != [serde_json::Value::String(discovery_root.to_str()?.to_owned())]
        || discovery_root.canonicalize().ok()? != *discovery_root
    { return None; }
    for entry in registry.get("projects")?.as_array()? {
        if entry.get("auto_enrolled").and_then(|v| v.as_bool()) != Some(true) { continue; }
        let project = PathBuf::from(entry.get("project")?.as_str()?);
        if project.parent() != Some(discovery_root.as_path())
            || project.canonicalize().ok()? != project || !project.is_dir()
        { continue; }
        for channel in ["memory", "sources"] {
            let (config, expected_hash) = if channel == "memory" {
                (entry.get("profile")?.as_str()?, entry.get("profile_sha256")?.as_str()?)
            } else {
                let source = entry.get("source")?;
                if source.get("project_root")?.as_str()? != project.to_str()? { continue; }
                (source.get("config_path")?.as_str()?, source.get("config_sha256")?.as_str()?)
            };
            let config_path = PathBuf::from(config);
            let bytes = private_read(&config_path).ok()?;
            if hex::encode(Sha256::digest(&bytes)) != expected_hash { continue; }
            let profile: toml::Value = toml::from_str(std::str::from_utf8(&bytes).ok()?).ok()?;
            let roots = profile.get("roots").and_then(toml::Value::as_array)?;
            if roots.len() != 1 { continue; }
            let selected_root = roots[0].get("path").and_then(toml::Value::as_str);
            if channel == "memory" {
                if selected_root != entry.get("memory").and_then(serde_json::Value::as_str)
                    || roots[0].get("id").and_then(toml::Value::as_str) != Some("auto-memory")
                    || roots[0].get("allowed_files").and_then(toml::Value::as_array)
                        .is_none_or(|files| files.iter().filter_map(toml::Value::as_str).collect::<Vec<_>>()
                            != ["STATUS.md", "DECISIONS.md", "SOLUTIONS.md"])
                { continue; }
            } else {
                let source = entry.get("source")?;
                if selected_root != source.get("corpus_path").and_then(serde_json::Value::as_str)
                    || roots[0].get("id").and_then(toml::Value::as_str)
                        != source.get("root_id").and_then(serde_json::Value::as_str)
                    || profile.get("projection_invalidation")?.get("manifest_path")?.as_str()
                        != source.get("policy_path").and_then(serde_json::Value::as_str)
                    || profile.get("projection_invalidation")?.get("manifest_sha256")?.as_str()
                        != source.get("policy_sha256").and_then(serde_json::Value::as_str)
                { continue; }
                let policy_path = PathBuf::from(source.get("policy_path")?.as_str()?);
                let policy = private_read(&policy_path).ok()?;
                if hex::encode(Sha256::digest(&policy))
                    != source.get("policy_sha256")?.as_str()?
                { continue; }
                let policy: serde_json::Value = serde_json::from_slice(&policy).ok()?;
                if policy.get("original_roots")?.as_array()?.as_slice()
                    != [serde_json::Value::String(project.to_str()?.to_owned())]
                    || policy.get("corpus_path")?.as_str()?
                        != source.get("corpus_path")?.as_str()?
                { continue; }
            }
            let managed = channel == "memory" && entry.get("managed_note_writes")?.as_bool()?;
            let candidate = BindingConfig {
                channel: channel.to_owned(), project: project.to_str()?.to_owned(),
                config: config_path, config_sha256: expected_hash.to_owned(),
                token: token.to_owned(), managed_note_writes: managed,
            };
            if !profile_valid(&candidate) { continue; }
            let message = format!("vrg-auto-binding\0{channel}\0{}\0{expected_hash}", candidate.project);
            let mut mac = Hmac::<Sha256>::new_from_slice(state.identity_key.as_bytes()).ok()?;
            mac.update(message.as_bytes());
            if mac.verify_slice(&hex::decode(token).ok()?).is_err() { continue; }
            let mut cache = state.dynamic_bindings.lock().ok()?;
            if let Some(binding) = cache.get(token) {
                if binding.config == candidate { return Some(binding.clone()); }
            }
            let binding = Arc::new(Binding { config: candidate, loaded: Mutex::new(None) });
            cache.insert(token.to_owned(), binding.clone());
            return Some(binding);
        }
    }
    None
}
fn authenticated_binding(state: &Runtime, token: &str) -> Option<Arc<Binding>> {
    state.bindings.get(token).cloned().or_else(|| dynamic_binding(state, token))
}
fn valid_headers(req: &Request<Body>, state: &Runtime) -> bool {
    let h = req.headers();
    h.get_all("host").iter().count() == 1 && h.get("host").and_then(|v|v.to_str().ok()) == Some(state.authority.as_str())
        && !h.contains_key("origin") // Local Codex is not a browser. No browser origin is authorized.
        && !h.contains_key("forwarded") && !h.contains_key("x-forwarded-host")
}
impl Binding {
    fn service(&self, transport: &StreamableHttpServerConfig) -> VrgResult<McpService> {
        let mut loaded = self.loaded.lock().map_err(|_| invalid())?;
        if !profile_valid(&self.config) { return Err(invalid()); }
        if loaded.is_none() {
            let engine = Arc::new(crate::cli::open_engine(Some(self.config.config.clone()))?);
            // Existing engine takes its native monitor lease; never a watcher per HTTP client.
            engine.start_change_monitor()?;
            engine.reconcile_at_startup()?;
            let tools = VrgTools { engine: engine.clone(), project: self.config.project.clone(), shared: true };
            let service = StreamableHttpService::new(move || Ok(tools.clone()), Arc::new(LocalSessionManager::default()), transport.clone());
            *loaded = Some(Loaded { service, engine });
        }
        Ok(loaded.as_ref().unwrap().service.clone())
    }
}
async fn mcp(State(state): State<Arc<Runtime>>, req: Request<Body>) -> Response {
    if !valid_headers(&req, &state) { return StatusCode::FORBIDDEN.into_response(); }
    let Some(token) = req.headers().get("authorization").and_then(|v|v.to_str().ok()).and_then(|v|v.strip_prefix("Bearer ")) else { return StatusCode::UNAUTHORIZED.into_response(); };
    let Some(binding) = authenticated_binding(&state, token) else { return StatusCode::UNAUTHORIZED.into_response(); };
    if req.uri().path() != format!("/{}", binding.config.channel) { return StatusCode::FORBIDDEN.into_response(); }
    let Ok(_permit) = state.permits.clone().try_acquire_owned() else { return StatusCode::SERVICE_UNAVAILABLE.into_response(); };
    let transport = state.transport.clone();
    let service = tokio::task::spawn_blocking(move || binding.service(&transport)).await;
    match service {
        Ok(Ok(service)) => match service.oneshot(req).await {
            Ok(response) => response.map(Body::new),
            Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        },
        _ => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoteApplyRequest {
    relative_path: String,
    expected_revision: String,
    proposed_markdown: String,
    operation_id: String,
}
async fn managed_note_apply(State(state): State<Arc<Runtime>>, req: Request<Body>) -> Response {
    if !valid_headers(&req, &state) { return StatusCode::FORBIDDEN.into_response(); }
    let Some(token) = req.headers().get("authorization").and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")) else { return StatusCode::UNAUTHORIZED.into_response(); };
    let Some(binding) = authenticated_binding(&state, token) else { return StatusCode::UNAUTHORIZED.into_response(); };
    if binding.config.channel != "memory" || !binding.config.managed_note_writes { return StatusCode::FORBIDDEN.into_response(); }
    let Ok(_permit) = state.permits.clone().try_acquire_owned() else { return StatusCode::SERVICE_UNAVAILABLE.into_response(); };
    let Ok(bytes) = to_bytes(req.into_body(), 2_100_000).await else { return StatusCode::PAYLOAD_TOO_LARGE.into_response(); };
    let Ok(input) = serde_json::from_slice::<NoteApplyRequest>(&bytes) else { return StatusCode::BAD_REQUEST.into_response(); };
    let result = tokio::task::spawn_blocking(move || {
        if !profile_valid(&binding.config) { return Err(invalid()); }
        let config = crate::VrgConfig::load(&binding.config.config)?;
        note_write::apply_managed(&config, &input.relative_path, &input.expected_revision, &input.proposed_markdown, &input.operation_id)
    }).await;
    match result {
        Ok(Ok(receipt)) => axum::Json(serde_json::json!(receipt)).into_response(),
        Ok(Err(VrgError::Conflict(message))) if message.starts_with("REVISION_CONFLICT:") =>
            (StatusCode::CONFLICT, axum::Json(serde_json::json!({"error":"REVISION_CONFLICT","current_revision":message.trim_start_matches("REVISION_CONFLICT:")}))).into_response(),
        Ok(Err(VrgError::AccessDenied(_) | VrgError::SymlinkRejected(_) | VrgError::PathEscape(_) | VrgError::InvalidConfig(_))) => StatusCode::FORBIDDEN.into_response(),
        _ => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}
#[derive(Deserialize)]
struct Challenge { nonce: String }
async fn identity(State(state): State<Arc<Runtime>>, Query(challenge): Query<Challenge>, req: Request<Body>) -> Response {
    if !valid_headers(&req, &state) || challenge.nonce.len() != 64 || !challenge.nonce.bytes().all(|c|c.is_ascii_hexdigit()) { return StatusCode::FORBIDDEN.into_response(); }
    let mut mac = Hmac::<Sha256>::new_from_slice(state.identity_key.as_bytes()).expect("HMAC key");
    mac.update(challenge.nonce.as_bytes());
    axum::Json(serde_json::json!({"pid":std::process::id(),"proof":hex::encode(mac.finalize().into_bytes()),"transport":"rmcp-streamable-http"})).into_response()
}
pub async fn serve(path: &Path) -> VrgResult<()> {
    let manifest: Manifest = serde_json::from_slice(&private_read(path)?).map_err(|_|invalid())?;
    if manifest.port == 0 || manifest.identity_key.len() != 64 || manifest.bindings.is_empty() { return Err(invalid()); }
    let mut bindings = HashMap::new();
    for config in manifest.bindings {
        if !["memory","sources"].contains(&config.channel.as_str()) || config.token.len()!=64 || !profile_valid(&config) { return Err(invalid()); }
        if bindings.insert(config.token.clone(), Arc::new(Binding{ config, loaded:Mutex::new(None) })).is_some() { return Err(invalid()); }
    }
    let mut transport = StreamableHttpServerConfig::default();
    transport.allowed_hosts = vec![format!("127.0.0.1:{}", manifest.port)];
    transport.allowed_origins = vec![format!("http://127.0.0.1:{}", manifest.port)];
    let state = Arc::new(Runtime { authority:format!("127.0.0.1:{}",manifest.port), identity_key:manifest.identity_key,
        transport, bindings, dynamic_bindings:Mutex::new(HashMap::new()),
        auto_registry_path:manifest.auto_registry_path, auto_discovery_root:manifest.auto_discovery_root,
        permits:Arc::new(tokio::sync::Semaphore::new(16)) });
    // Binding the persisted endpoint is authoritative. Never switch port or connect to an occupant.
    let listener = if let Ok(fd) = std::env::var("VRG_LISTEN_FD") {
        use std::os::fd::FromRawFd;
        let fd: i32 = fd.parse().map_err(|_|invalid())?;
        if fd < 3 { return Err(invalid()); }
        // The short-lived enrollment helper passes this owned socket through runsv.
        // runsv retains its own descriptor, reserving the endpoint across child restarts.
        let socket = unsafe { std::net::TcpListener::from_raw_fd(fd) };
        if socket.local_addr()?.to_string() != state.authority { return Err(invalid()); }
        socket.set_nonblocking(true)?;
        tokio::net::TcpListener::from_std(socket)?
    } else {
        tokio::net::TcpListener::bind(&state.authority).await?
    };
    let idle_state = state.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        loop {
            interval.tick().await;
            for binding in idle_state.bindings.values() {
                if let Ok(loaded) = binding.loaded.try_lock() { if let Some(loaded) = &*loaded { let _ = loaded.engine.start_change_monitor(); loaded.engine.release_idle_resources(); } }
            }
            let dynamic = idle_state.dynamic_bindings.lock().ok()
                .map(|bindings| bindings.values().cloned().collect::<Vec<_>>())
                .unwrap_or_default();
            for binding in dynamic {
                if let Ok(loaded) = binding.loaded.try_lock() { if let Some(loaded) = &*loaded { let _ = loaded.engine.start_change_monitor(); loaded.engine.release_idle_resources(); } }
            }
        }
    });
    let shutdown = state.transport.cancellation_token.clone();
    let router = Router::new().route("/identity",get(identity)).route("/memory",any(mcp)).route("/sources",any(mcp)).route("/note/apply",post(managed_note_apply)).with_state(state);
    axum::serve(listener, router).with_graceful_shutdown(async move {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("SIGTERM");
        tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = term.recv() => {} }
        shutdown.cancel();
    }).await?;
    Ok(())
}
