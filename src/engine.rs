use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex, Weak,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

use notify::{
    EventKind, RecommendedWatcher, RecursiveMode, Watcher,
    event::{AccessKind, AccessMode},
};

use crate::{
    config::VrgConfig,
    contracts::{
        EngineStatus, GraphEdge, IndexReport, IndexSyncStatus, ModuleStatus, RuntimeIdentity,
        SearchRequest, SearchResponse, SearchSyncState, SourceDocument, SourceRef, StopSyncOutcome,
        WritePlan,
    },
    error::{VrgError, VrgResult},
    indexer, note_write, retrieval,
    semantic::LocalEmbedder,
    storage::DerivedStore,
    vault,
};

/// Cloneable handle shared by the CLI command and local MCP stdio service.
#[derive(Clone)]
pub struct VrgEngine {
    state: Arc<Mutex<EngineState>>,
}

struct EngineState {
    config: VrgConfig,
    runtime_identity: Option<RuntimeIdentity>,
    store: DerivedStore,
    embedder: Option<LocalEmbedder>,
    semantic_status: ModuleStatus,
    monitor: Option<ChangeMonitor>,
    monitor_status: ModuleStatus,
    last_reconcile_unix_ms: u64,
}

struct ChangeMonitor {
    _watcher: RecommendedWatcher,
    _monitor_lock: std::fs::File,
    epoch: Arc<AtomicU64>,
    handled_epoch: Arc<AtomicU64>,
    changes: Arc<Mutex<SourceChanges>>,
}
#[derive(Clone, Debug, Default)]
struct SourceChanges {
    full: bool,
    paths: BTreeSet<PathBuf>,
}

impl VrgEngine {
    pub fn open(config: VrgConfig) -> VrgResult<Self> {
        Self::open_with_runtime_identity(config, None)
    }

    pub fn open_with_runtime_identity(
        mut config: VrgConfig,
        runtime_identity: Option<RuntimeIdentity>,
    ) -> VrgResult<Self> {
        config.validate()?;
        config.validate_layout()?;
        config.data_dir = config.canonical_data_dir()?;

        // This is the first derived write; layout checks above prove it cannot
        // overlap a source or model root.
        let store = DerivedStore::open(&config.data_dir)?;
        // Journal reconciliation must not race an active generation publisher.
        // If another process holds the lock, leave recovery to that publisher.
        if let Some(_sync_lock) = store.try_sync_lock()? {
            store.recover_journal()?;
        }

        let (embedder, semantic_status) = match &config.semantic_model {
            Some(model_config) => {
                match LocalEmbedder::load_with_data_dir(model_config, Some(&config.data_dir)) {
                    Ok(embedder) => (Some(embedder), ModuleStatus::Ready),
                    Err(error) => (None, ModuleStatus::Degraded(error.to_string())),
                }
            }
            None => (
                None,
                ModuleStatus::Degraded("no local semantic model is configured".into()),
            ),
        };

        Ok(Self {
            state: Arc::new(Mutex::new(EngineState {
                config,
                runtime_identity,
                store,
                embedder,
                semantic_status,
                monitor: None,
                monitor_status: ModuleStatus::Degraded("change monitor has not started".into()),
                last_reconcile_unix_ms: 0,
            })),
        })
    }

    /// Called by the MCP task; never queue behind an active query/index operation.
    pub fn release_idle_resources(&self) {
        if let Ok(state) = self.state.try_lock() {
            if let Some(embedder) = &state.embedder {
                embedder.release_idle(Duration::from_secs(120));
            }
        }
    }

    pub fn memory_runtime_status(&self) -> VrgResult<serde_json::Value> {
        let state = self.lock_state()?;
        Ok(serde_json::json!({
            "model_resident": state.embedder.as_ref().is_some_and(LocalEmbedder::is_resident),
            "model_idle_seconds": 120, "idle_check_seconds": 30,
            "event_batch_seconds": 30, "event_path_capacity": 4096,
            "sqlite_cache_kib": 2048, "sqlite_mmap_bytes": 16777216,
            "model_id": state.embedder.as_ref().map(LocalEmbedder::model_id),
        }))
    }

    pub fn status(&self) -> EngineStatus {
        let Ok(state) = self.state.lock() else {
            return failed_engine_status("engine lock poisoned");
        };
        let generation_id = match state.store.current_generation_id() {
            Ok(value) => value,
            Err(error) => {
                return EngineStatus {
                    runtime_identity: state.runtime_identity.clone(),
                    generation_id: None,
                    configured_roots: state.config.roots.len(),
                    text: ModuleStatus::Failed(error.to_string()),
                    semantic: state.semantic_status.clone(),
                    graph: ModuleStatus::Failed("derived store unavailable".into()),
                    storage: ModuleStatus::Failed(error.to_string()),
                    note_writes: ModuleStatus::Disabled,
                    remote_chatgpt: ModuleStatus::Disabled,
                    index_sync: IndexSyncStatus::Failed {
                        generation_id: None,
                        reason: "derived store unavailable".into(),
                        failed_unix_ms: unix_ms(),
                    },
                    change_monitor: ModuleStatus::Failed("derived store unavailable".into()),
                };
            }
        };
        EngineStatus {
            runtime_identity: state.runtime_identity.clone(),
            text: ModuleStatus::Ready,
            semantic: match (&state.semantic_status, &state.embedder) {
                (ModuleStatus::Ready, Some(embedder)) => {
                    match state.store.semantic_coverage(embedder.model_id()) {
                        Ok((chunks, vectors)) if chunks > 0 && chunks == vectors => {
                            ModuleStatus::Ready
                        }
                        Ok((chunks, vectors)) => ModuleStatus::Degraded(format!(
                            "local model is configured but active vector coverage is {vectors}/{chunks}"
                        )),
                        Err(error) => ModuleStatus::Degraded(error.to_string()),
                    }
                }
                _ => state.semantic_status.clone(),
            },
            graph: if generation_id.is_some() {
                ModuleStatus::Ready
            } else {
                ModuleStatus::Disabled
            },
            storage: ModuleStatus::Ready,
            note_writes: ModuleStatus::Disabled,
            remote_chatgpt: ModuleStatus::Disabled,
            generation_id: generation_id.clone(),
            configured_roots: state.config.roots.len(),
            index_sync: if state.config.roots.is_empty() {
                IndexSyncStatus::Disabled {
                    reason: "no authorized roots configured".into(),
                }
            } else {
                state
                    .store
                    .index_sync_status()
                    .unwrap_or_else(|error| IndexSyncStatus::Failed {
                        generation_id: generation_id.clone(),
                        reason: error.to_string(),
                        failed_unix_ms: unix_ms(),
                    })
            },
            change_monitor: if state.config.roots.is_empty() {
                ModuleStatus::Disabled
            } else if state.store.monitor_is_active().unwrap_or(false) {
                ModuleStatus::Ready
            } else {
                state.monitor_status.clone()
            },
        }
    }

    pub fn index(&self) -> VrgResult<IndexReport> {
        self.start_change_monitor()?;
        self.index_with_existing_guard()
    }

    /// A short CLI index still guards authoritative files, while its owned
    /// corpus writes are not misclassified as external filesystem changes.
    pub(crate) fn index_once(&self) -> VrgResult<IndexReport> {
        self.start_change_monitor_with_derived(false)?;
        self.index_with_existing_guard()
    }

    fn index_with_existing_guard(&self) -> VrgResult<IndexReport> {
        let mut state = self.lock_state()?;
        if state.config.roots.is_empty() {
            return Err(VrgError::InvalidConfig(
                "refusing to index with an empty source-root allowlist".into(),
            ));
        }
        let _sync_lock = state.store.try_sync_lock()?.ok_or_else(|| {
            VrgError::Conflict("an index synchronization is already in progress".into())
        })?;
        let observed_epoch = state
            .monitor
            .as_ref()
            .map(|monitor| monitor.epoch.load(Ordering::Acquire));
        let result = self.index_locked(&mut state, None);
        if result.is_ok() {
            if let (Some(monitor), Some(observed)) = (&state.monitor, observed_epoch) {
                if monitor.epoch.load(Ordering::Acquire) == observed {
                    monitor.handled_epoch.store(observed, Ordering::Release);
                }
            }
        }
        result
    }

    /// Start one cross-platform filesystem watcher and a coalescing reconciliation
    /// worker. Manifests/hashes remain the correctness fallback for missed events.
    pub fn start_change_monitor(&self) -> VrgResult<()> {
        // Projected corpora are owned output. Watching their publication feeds
        // our own writes back into source invalidation. The projection manifest
        // supplies the authoritative code/note roots; ordinary vaults still
        // watch their configured roots when no projection manifest is present.
        self.start_change_monitor_with_derived(false)
    }

    fn start_change_monitor_with_derived(&self, include_derived: bool) -> VrgResult<()> {
        let mut state = self.lock_state()?;
        if state.config.roots.is_empty() || state.monitor.is_some() {
            return Ok(());
        }
        let Some(monitor_lock) = state.store.try_monitor_lock()? else {
            state.monitor_status = ModuleStatus::Ready;
            return Ok(());
        };
        let epoch = Arc::new(AtomicU64::new(1));
        let handled_epoch = Arc::new(AtomicU64::new(0));
        let signal = Arc::new((Mutex::new(true), Condvar::new()));
        let changes = Arc::new(Mutex::new(SourceChanges {
            full: true,
            paths: BTreeSet::new(),
        }));
        let callback_changes = Arc::clone(&changes);
        let roots = state
            .config
            .roots
            .iter()
            .filter_map(|root| {
                root.path
                    .canonicalize()
                    .ok()
                    .map(|path| (path, root.clone()))
            })
            .collect::<Vec<_>>();
        let projection_sources = crate::projection::updates::watch_sources(&state.config)?;
        let callback_sources = projection_sources.clone();
        let callback_epoch = Arc::clone(&epoch);
        let callback_signal = Arc::clone(&signal);
        let callback_roots = roots.clone();
        let callback_store = state.store.clone();
        let watcher_result =
            notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
                let (full, paths) = match result {
                    Ok(event) => {
                        if matches!(
                            event.kind,
                            EventKind::Access(
                                AccessKind::Read
                                    | AccessKind::Open(AccessMode::Any | AccessMode::Read)
                                    | AccessKind::Close(AccessMode::Read)
                            )
                        ) {
                            return;
                        }
                        let full = event.need_rescan()
                            || event.paths.iter().any(|path| {
                                path.file_name().is_some_and(|name| name == ".codexignore")
                            });
                        let paths = event
                            .paths
                            .into_iter()
                            .filter(|path| {
                                crate::projection::updates::source_event_relevant(
                                    &callback_sources,
                                    path,
                                ) || callback_roots.iter().any(|(root, config)| {
                                    path.strip_prefix(root).ok().is_some_and(|relative| {
                                        !crate::vault::is_excluded(relative, config)
                                            && (crate::vault::is_markdown(relative)
                                                || path.is_dir()
                                                || !path.exists())
                                    })
                                })
                            })
                            .collect::<BTreeSet<_>>();
                        (full, paths)
                    }
                    Err(_) => (true, BTreeSet::new()),
                };
                let relevant = full || !paths.is_empty();
                if relevant {
                    if let Ok(mut changes) = callback_changes.lock() {
                        changes.full |= full;
                        changes.paths.extend(paths);
                        // Overflow falls back to existing durable stale/recovery
                        // reconciliation; do not retain an unbounded event queue.
                        if changes.full || changes.paths.len() > 4096 {
                            changes.full = true;
                            changes.paths.clear();
                        }
                    }
                    let _ = callback_store.mark_index_stale();
                    callback_epoch.fetch_add(1, Ordering::AcqRel);
                    if let Ok(mut pending) = callback_signal.0.lock() {
                        *pending = true;
                    }
                    callback_signal.1.notify_one();
                }
            });
        let mut watcher = match watcher_result {
            Ok(watcher) => watcher,
            Err(error) => {
                state.monitor_status = ModuleStatus::Degraded(format!(
                    "filesystem notifications unavailable: {error}"
                ));
                return Ok(());
            }
        };
        let watch_configured_roots = include_derived || projection_sources.is_empty();
        for (path, _) in &roots {
            if !watch_configured_roots {
                continue;
            }
            if let Err(error) = watcher.watch(path, RecursiveMode::Recursive) {
                state.monitor_status = ModuleStatus::Degraded(format!(
                    "filesystem notifications unavailable: {error}"
                ));
                return Ok(());
            }
        }
        for (path, _) in &projection_sources {
            if roots.iter().any(|(root, _)| root == path) {
                continue;
            }
            if let Err(error) = watcher.watch(path, RecursiveMode::Recursive) {
                state.monitor_status = ModuleStatus::Degraded(format!(
                    "source filesystem notifications unavailable: {error}"
                ));
                return Ok(());
            }
        }
        // A newly acquired monitor cannot cover edits made since another
        // process last released the watcher lock, even if this engine indexed
        // while that other monitor was still active.
        state.last_reconcile_unix_ms = 0;
        state.monitor_status = ModuleStatus::Ready;
        state.monitor = Some(ChangeMonitor {
            _watcher: watcher,
            _monitor_lock: monitor_lock,
            epoch: Arc::clone(&epoch),
            handled_epoch: Arc::clone(&handled_epoch),
            changes,
        });
        let weak_state = Arc::downgrade(&self.state);
        thread::Builder::new()
            .name("vrg-change-reconcile".into())
            .spawn(move || {
                change_worker(weak_state, epoch, handled_epoch, signal);
            })
            .map_err(VrgError::Io)?;
        Ok(())
    }

    /// Reconcile source changes before the MCP service begins accepting queries.
    pub fn reconcile_at_startup(&self) -> VrgResult<()> {
        self.start_change_monitor()?;
        let _ = self.refresh_if_needed(false)?;
        Ok(())
    }

    /// Refresh the derived index once for a Codex Stop event. The event key is
    /// persisted so duplicated Stop handlers cannot republish the same turn.
    /// Use the existing native monitor's publication. A full reconciliation is
    /// requested only when this process must recover an absent monitor owner.
    pub fn sync_existing_or_recover(&self) -> VrgResult<serde_json::Value> {
        let started = std::time::Instant::now();
        let deadline = started + Duration::from_secs(60);
        let mut recovery_requested = false;
        loop {
            let state = self.lock_state()?;
            if state.config.roots.is_empty() {
                return Ok(
                    serde_json::json!({"state":"disabled","reason":"no authorized source roots","recovery_requested":false}),
                );
            }
            let active = state.monitor.is_some() || state.store.monitor_is_active()?;
            let caught_up = state.monitor.as_ref().is_none_or(|monitor| {
                monitor.epoch.load(Ordering::Acquire)
                    == monitor.handled_epoch.load(Ordering::Acquire)
            });
            let status = state.store.index_sync_status()?;
            if active && caught_up && matches!(&status, IndexSyncStatus::UpToDate { .. }) {
                return Ok(
                    serde_json::json!({"state":"up_to_date","mode":"native_monitor","observation_basis":"native_index_sync_status","generation_id":state.store.current_generation_id()?,"recovery_requested":recovery_requested,"waited_ms":started.elapsed().as_millis(),"index_sync":status}),
                );
            }
            if let IndexSyncStatus::Failed { reason, .. } = &status {
                if active && (!recovery_requested || started.elapsed() >= Duration::from_secs(30)) {
                    return Err(VrgError::Conflict(format!(
                        "native monitor synchronization failed: {reason}"
                    )));
                }
            }
            if std::time::Instant::now() >= deadline {
                return Err(VrgError::Conflict("native monitor synchronization is still pending after 60 seconds; no competing full scan was started".into()));
            }
            drop(state);
            if !active {
                self.start_change_monitor()?;
                recovery_requested = true;
            }
            thread::sleep(Duration::from_millis(100));
        }
    }

    pub fn sync_on_stop(&self, event_key: &str) -> VrgResult<StopSyncOutcome> {
        self.start_change_monitor()?;
        let mut state = self.lock_state()?;
        if state.config.roots.is_empty() {
            return Ok(StopSyncOutcome::Disabled);
        }
        let Some(_sync_lock) = state.store.try_sync_lock()? else {
            return Ok(StopSyncOutcome::InProgress);
        };
        if let Some(previous_state) = state.store.sync_event_state(event_key)? {
            if previous_state != "indexing" {
                return Ok(StopSyncOutcome::Duplicate(state.store.index_sync_status()?));
            }
        }
        let monitor_caught_up = if let Some(monitor) = &state.monitor {
            monitor.epoch.load(Ordering::Acquire) == monitor.handled_epoch.load(Ordering::Acquire)
        } else {
            state.store.monitor_is_active()?
        };
        if monitor_caught_up
            && matches!(
                state.store.index_sync_status()?,
                IndexSyncStatus::UpToDate { .. }
            )
        {
            let current = state.store.current_generation_id()?;
            state.store.set_index_sync_status(
                "up_to_date",
                current.as_deref(),
                Some(unix_ms() as i64),
                None,
                Some(event_key),
            )?;
            return Ok(StopSyncOutcome::SkippedCurrent(
                state.store.index_sync_status()?,
            ));
        }
        let observed_epoch = state
            .monitor
            .as_ref()
            .map(|monitor| monitor.epoch.load(Ordering::Acquire));
        match self.index_locked(&mut state, Some(event_key)) {
            Ok(report) => {
                let status = state.store.index_sync_status()?;
                if !matches!(
                    &status,
                    IndexSyncStatus::Stale { .. } | IndexSyncStatus::Indexing { .. }
                ) {
                    if let (Some(monitor), Some(observed)) = (&state.monitor, observed_epoch) {
                        if monitor.epoch.load(Ordering::Acquire) == observed {
                            monitor.handled_epoch.store(observed, Ordering::Release);
                        }
                    }
                }
                if matches!(
                    &status,
                    IndexSyncStatus::Stale { .. } | IndexSyncStatus::Indexing { .. }
                ) {
                    Ok(StopSyncOutcome::InProgress)
                } else {
                    Ok(StopSyncOutcome::Completed { report, status })
                }
            }
            Err(error) => Ok(StopSyncOutcome::Failed(
                state
                    .store
                    .index_sync_status()
                    .unwrap_or(IndexSyncStatus::Failed {
                        generation_id: state.store.current_generation_id().ok().flatten(),
                        reason: error.to_string(),
                        failed_unix_ms: unix_ms(),
                    }),
            )),
        }
    }

    pub fn record_session_event(
        &self,
        session_id: &str,
        turn_id: &str,
        cwd: &str,
        event_name: &str,
    ) -> VrgResult<()> {
        let state = self.lock_state()?;
        if !state.config.capture_session_events {
            return Ok(());
        }
        let project_key = crate::auto_sync::hash_project(cwd);
        state.store.record_session_event(
            session_id,
            turn_id,
            &project_key,
            event_name,
            unix_ms() as i64,
            state.config.session_event_retention_days,
        )
    }

    pub fn session_event_count(&self) -> VrgResult<u64> {
        self.lock_state()?.store.session_event_count()
    }

    pub fn recent_session_events(
        &self,
        cwd: &str,
        limit: usize,
    ) -> VrgResult<Vec<crate::contracts::SessionEventRecord>> {
        let state = self.lock_state()?;
        if !state.config.capture_session_events {
            return Ok(Vec::new());
        }
        let project_key = crate::auto_sync::hash_project(cwd);
        state.store.recent_session_events(&project_key, limit)
    }

    pub fn capture_session_events(&self) -> bool {
        self.state
            .lock()
            .map(|state| state.config.capture_session_events)
            .unwrap_or(false)
    }

    pub fn clear_session_events(&self) -> VrgResult<u64> {
        self.lock_state()?.store.clear_session_events()
    }

    pub fn prune_session_events(&self) -> VrgResult<u64> {
        let state = self.lock_state()?;
        let cutoff = unix_ms()
            .saturating_sub(u64::from(state.config.session_event_retention_days) * 86_400_000)
            as i64;
        state.store.prune_session_events(cutoff)
    }

    fn index_locked(
        &self,
        state: &mut EngineState,
        event_key: Option<&str>,
    ) -> VrgResult<IndexReport> {
        let parent = state.store.current_generation_id()?;
        let source_change_revision = state.store.source_change_revision()?;
        state.store.set_index_sync_status(
            "indexing",
            parent.as_deref(),
            Some(unix_ms() as i64),
            None,
            event_key,
        )?;
        match index_from_events(state, event_key.is_none()) {
            Ok(report) => {
                state.semantic_status = report.semantic_status.clone();
                state.store.finish_index_sync(
                    source_change_revision,
                    report.generation_id.as_deref(),
                    unix_ms() as i64,
                    event_key,
                )?;
                state.last_reconcile_unix_ms = unix_ms();
                Ok(report)
            }
            Err(error) => {
                let generation = state
                    .store
                    .current_generation_id()
                    .ok()
                    .flatten()
                    .or(parent);
                let detail = error.to_string();
                let _ = state.store.set_index_sync_status(
                    "failed",
                    generation.as_deref(),
                    Some(unix_ms() as i64),
                    Some(&detail),
                    event_key,
                );
                Err(error)
            }
        }
    }

    /// Move only the active derived-state pointer to a retained generation.
    pub fn rollback(&self, target_generation: &str) -> VrgResult<String> {
        if target_generation.trim().is_empty() || target_generation.len() > 128 {
            return Err(VrgError::InvalidConfig(
                "target generation ID is invalid".into(),
            ));
        }
        let state = self.lock_state()?;
        let _sync_lock = state.store.try_sync_lock()?.ok_or_else(|| {
            VrgError::Conflict("cannot roll back while index synchronization is active".into())
        })?;
        let current = state
            .store
            .current_generation_id()?
            .ok_or_else(|| VrgError::Recovery("no active generation to roll back from".into()))?;
        state
            .store
            .rollback_generation(target_generation, &current)?;
        Ok(target_generation.to_owned())
    }

    pub fn search(&self, request: SearchRequest) -> VrgResult<SearchResponse> {
        self.search_with_policy(request, false)
    }

    pub fn search_with_policy(
        &self,
        request: SearchRequest,
        allow_stale: bool,
    ) -> VrgResult<SearchResponse> {
        self.search_scoped_with_policy(request, allow_stale, None)
    }

    pub fn search_scoped_with_policy(
        &self,
        mut request: SearchRequest,
        allow_stale: bool,
        context_source: Option<&SourceRef>,
    ) -> VrgResult<SearchResponse> {
        self.start_change_monitor()?;
        let (sync_state, sync_reason) = self.refresh_if_needed(allow_stale)?;
        let state = self.lock_state()?;
        if context_source.is_some() && state.config.adaptive_memory.enabled {
            return Err(VrgError::InvalidConfig(
                "context-scoped search does not support adaptive response budgets".into(),
            ));
        }
        if matches!(&sync_state, SearchSyncState::Disabled) {
            if context_source.is_some() {
                return Err(VrgError::AccessDenied(
                    "context source is unavailable or not current".into(),
                ));
            }
            return Ok(SearchResponse {
                generation_id: None,
                sync_state,
                sync_reason: sync_reason.clone(),
                mode: if request.use_semantic {
                    crate::contracts::RetrievalMode::DegradedTextOnly
                } else {
                    crate::contracts::RetrievalMode::TextOnly
                },
                degradation_reason: sync_reason,
                hits: Vec::new(),
            });
        }
        if !allow_stale
            && !matches!(
                &sync_state,
                SearchSyncState::Ready | SearchSyncState::Disabled
            )
        {
            let reason = sync_reason.clone();
            return Ok(SearchResponse {
                generation_id: state.store.current_generation_id()?,
                sync_state,
                sync_reason: sync_reason.clone(),
                mode: crate::contracts::RetrievalMode::DegradedTextOnly,
                degradation_reason: reason,
                hits: Vec::new(),
            });
        }
        // A client filter never grants access to a root absent from this profile.
        let authorized = state
            .config
            .roots
            .iter()
            .map(|r| r.id.clone())
            .collect::<Vec<_>>();
        request.root_ids = Some(match request.root_ids.take() {
            Some(ids) => ids
                .into_iter()
                .filter(|id| authorized.contains(id))
                .collect(),
            None => authorized,
        });
        let context_revision = if let Some(context) = context_source {
            if !request
                .root_ids
                .as_ref()
                .is_some_and(|roots| roots.contains(&context.root_id))
            {
                return Err(VrgError::AccessDenied(
                    "context source is unavailable or not current".into(),
                ));
            }
            let current = vault::read_source(
                &state.config,
                &context.root_id,
                &context.relative_path,
                4 * 1024 * 1024,
            )
            .map_err(|_| {
                VrgError::AccessDenied("context source is unavailable or not current".into())
            })?;
            let indexed = state.store.load_document(context)?;
            if !indexed.is_some_and(|document| document.revision == current.revision) {
                return Err(VrgError::AccessDenied(
                    "context source is unavailable or not current".into(),
                ));
            }
            Some(current.revision)
        } else {
            None
        };
        let access_cache =
            std::cell::RefCell::new(std::collections::BTreeMap::<SourceRef, Option<String>>::new());
        let authorize = |source: &SourceRef, revision: &str| {
            if let Some(current) = access_cache.borrow().get(source) {
                return current.as_deref() == Some(revision);
            }
            let current = vault::current_revision(&state.config, source).ok();
            let allowed = current.as_deref() == Some(revision);
            access_cache.borrow_mut().insert(source.clone(), current);
            allowed
        };
        let code_first = context_source.is_none()
            && crate::projection::updates::direct_sources_enabled(&state.config)?;
        let mut response = retrieval::retrieve_authorized_scoped_with_policy(
            &state.store,
            state.embedder.as_ref(),
            &request,
            &authorize,
            context_source,
            code_first,
        )?;
        // Recheck source policy and exact revision even when stale retrieval was requested.
        // Revocation cannot be bypassed through an old index, graph or summary.
        let mut checked = std::collections::BTreeMap::new();
        response.hits.retain_mut(|hit| {
            let current = checked
                .entry(hit.source.clone())
                .or_insert_with(|| vault::current_revision(&state.config, &hit.source).ok());
            if current.as_deref() != Some(hit.revision.as_str()) {
                return false;
            }
            hit.relations
                .retain(|edge| edge_authorized(&state.config, &state.store, edge));
            true
        });
        // This is deliberately a new capability read, not access_cache:
        // revoking or editing the hub while ranking invalidates the response.
        if let (Some(context), Some(expected)) = (context_source, context_revision.as_ref()) {
            let current = vault::read_source(
                &state.config,
                &context.root_id,
                &context.relative_path,
                4 * 1024 * 1024,
            )
            .map_err(|_| VrgError::AccessDenied("context source changed during search".into()))?;
            if &current.revision != expected {
                return Err(VrgError::AccessDenied(
                    "context source changed during search".into(),
                ));
            }
        }
        response.sync_state = sync_state;
        response.sync_reason = sync_reason;
        Ok(response)
    }

    fn refresh_if_needed(&self, allow_stale: bool) -> VrgResult<(SearchSyncState, Option<String>)> {
        let mut state = self.lock_state()?;
        if state.config.roots.is_empty() {
            return Ok((
                SearchSyncState::Disabled,
                Some("no authorized source roots configured".into()),
            ));
        }
        let current_epoch = state
            .monitor
            .as_ref()
            .map(|monitor| monitor.epoch.load(Ordering::Acquire));
        let handled_epoch = state
            .monitor
            .as_ref()
            .map(|monitor| monitor.handled_epoch.load(Ordering::Acquire));
        let monitor_ready = state.monitor.is_some()
            && matches!(&state.monitor_status, ModuleStatus::Ready)
            || state.store.monitor_is_active()?;
        let index_status = state.store.index_sync_status()?;
        let status_current = matches!(index_status, IndexSyncStatus::UpToDate { .. });
        let needs_reconcile = !monitor_ready || current_epoch != handled_epoch || !status_current;
        if !needs_reconcile {
            return Ok((SearchSyncState::Ready, None));
        }
        if state.monitor.is_some() {
            thread::sleep(Duration::from_millis(100));
        }
        let observed_epoch = state
            .monitor
            .as_ref()
            .map(|monitor| monitor.epoch.load(Ordering::Acquire));
        let Some(_sync_lock) = state.store.try_sync_lock()? else {
            return if allow_stale {
                Ok((
                    SearchSyncState::Stale,
                    Some("another process is synchronizing the index".into()),
                ))
            } else {
                Ok((
                    SearchSyncState::SyncPending,
                    Some("index synchronization is already in progress".into()),
                ))
            };
        };
        let event_key = format!(
            "query:{}:{}:{}",
            std::process::id(),
            unix_ms(),
            observed_epoch.unwrap_or_default()
        );
        let result = self.index_locked(&mut state, Some(&event_key));
        match result {
            Ok(_) => {
                state.last_reconcile_unix_ms = unix_ms();
                if matches!(
                    state.store.index_sync_status()?,
                    IndexSyncStatus::Stale { .. } | IndexSyncStatus::Indexing { .. }
                ) {
                    return if allow_stale {
                        Ok((
                            SearchSyncState::Stale,
                            Some("sources changed during reconciliation".into()),
                        ))
                    } else {
                        Ok((
                            SearchSyncState::SyncPending,
                            Some("sources changed during reconciliation".into()),
                        ))
                    };
                }
                if let (Some(monitor), Some(observed)) = (&state.monitor, observed_epoch) {
                    if monitor.epoch.load(Ordering::Acquire) == observed {
                        monitor.handled_epoch.store(observed, Ordering::Release);
                    } else {
                        return if allow_stale {
                            Ok((
                                SearchSyncState::Stale,
                                Some("sources changed during synchronization".into()),
                            ))
                        } else {
                            Ok((
                                SearchSyncState::SyncPending,
                                Some("sources changed during synchronization".into()),
                            ))
                        };
                    }
                }
                if monitor_ready {
                    Ok((SearchSyncState::Ready, None))
                } else {
                    Ok((SearchSyncState::Ready, Some("filesystem notifications are unavailable; this query used a direct source reconciliation".into())))
                }
            }
            Err(error) => {
                if allow_stale {
                    Ok((
                        SearchSyncState::Degraded,
                        Some(format!(
                            "source synchronization failed; serving the last valid generation: {error}"
                        )),
                    ))
                } else {
                    Ok((
                        SearchSyncState::Error,
                        Some(format!(
                            "source synchronization failed; no stale results returned: {error}"
                        )),
                    ))
                }
            }
        }
    }

    pub fn read_source(
        &self,
        root_id: &str,
        relative_path: &str,
        max_bytes: usize,
    ) -> VrgResult<SourceDocument> {
        let state = self.lock_state()?;
        if state.config.projection_invalidation.is_none() {
            return vault::read_source(&state.config, root_id, relative_path, max_bytes);
        }
        let document = vault::read_source(&state.config, root_id, relative_path, max_bytes)?;
        let verified = vault::read_source(&state.config, root_id, relative_path, max_bytes)?;
        if verified.revision != document.revision {
            return Err(VrgError::Conflict("source changed during read".into()));
        }
        Ok(verified)
    }

    pub fn neighbors(&self, source: &SourceRef, limit: usize) -> VrgResult<Vec<GraphEdge>> {
        let (freshness, reason) = self.refresh_if_needed(false)?;
        if !matches!(freshness, SearchSyncState::Ready) {
            return Err(VrgError::Storage(
                reason.unwrap_or_else(|| "derived graph is not current".into()),
            ));
        }
        let state = self.lock_state()?;
        if !state.config.roots.iter().any(|root| root.id == source.root_id) {
            return Err(VrgError::AccessDenied("root ID is not configured".into()));
        }
        // An absent node has no relations. Do not probe an excluded source on disk.
        let Some(indexed) = state.store.load_document(source)? else {
            return Ok(Vec::new());
        };
        // Authorize an indexed node before touching any historical relation.
        let current = vault::read_source(
            &state.config,
            &source.root_id,
            &source.relative_path,
            4 * 1024 * 1024,
        )?;
        if indexed.revision != current.revision {
            return Err(VrgError::Conflict(
                "graph source revision changed; reconcile before reading".into(),
            ));
        }
        let Some(snapshot) = state.store.load_current_catalog()? else {
            return Ok(Vec::new());
        };
        let graph = crate::graph::GraphSnapshot {
            nodes: snapshot
                .documents
                .iter()
                .map(|d| d.source.clone())
                .collect(),
            edges: snapshot.edges,
        };
        Ok(crate::graph::adjacent_edges(&graph, source, limit.min(100))
            .into_iter()
            .filter(|edge| edge_authorized(&state.config, &state.store, edge))
            .collect())
    }

    pub fn adaptive_policy(&self) -> crate::config::AdaptiveMemoryConfig {
        self.state
            .lock()
            .map(|s| s.config.adaptive_memory.clone())
            .unwrap_or_default()
    }

    pub fn search_budgeted_with_policy(
        &self,
        request: SearchRequest,
        budget: crate::adaptive::Budget,
        allow_stale: bool,
    ) -> VrgResult<crate::adaptive::BudgetedResponse> {
        {
            let state = self.lock_state()?;
            if state.config.projection_invalidation.is_some()
                && !crate::projection::updates::automatic_memory_enabled(&state.config)?
            {
                return Err(VrgError::InvalidConfig("projection invalidation does not support response budgets without pinned automatic-memory authorization".into()));
            }
        }
        let query = request.query.clone();
        let response = self.search_with_policy(request, allow_stale)?;
        let state = self.lock_state()?;
        let raw = state.store.load_adaptive_state()?;
        let memories = crate::adaptive::load_memories(raw.as_deref())?;
        let (project_id, privacy_scope_id) = adaptive_scope(&state.config)?;
        // The native budgeter can only select memories whose complete source
        // set is already visible. Do not authorize unrelated project memories.
        let visible = response
            .hits
            .iter()
            .map(|hit| {
                (
                    crate::adaptive::source_id(&hit.source),
                    hit.revision.clone(),
                )
            })
            .collect::<std::collections::BTreeSet<_>>();
        let memories = memories
            .into_iter()
            .filter(|memory| {
                memory.project_id == project_id
                    && memory.privacy_scope_id == privacy_scope_id
                    && !memory.sources.is_empty()
                    && memory.sources.iter().all(|span| {
                        visible.contains(&(span.source_id.clone(), span.revision_sha256.clone()))
                    })
                    && memory.sources.iter().all(|span| {
                        source_from_memory_id(&span.source_id).is_some_and(|source| {
                            vault::read_source(
                                &state.config,
                                &source.root_id,
                                &source.relative_path,
                                4 * 1024 * 1024,
                            )
                            .is_ok_and(|doc| doc.revision == span.revision_sha256)
                        })
                    })
            })
            .collect::<Vec<_>>();
        crate::adaptive::budget_response_with_query(response, &memories, budget, &query)
    }

    /// Passive facts only; no classification is fabricated when Codex has not proposed it.
    pub fn index_work_status(&self) -> VrgResult<Option<serde_json::Value>> {
        #[derive(serde::Deserialize, serde::Serialize)]
        #[serde(deny_unknown_fields)]
        struct Receipt {
            generation_id: Option<String>,
            unix_ns: String,
            indexed: usize,
            unchanged: usize,
            work: crate::contracts::IndexWork,
        }
        let state = self.lock_state()?;
        let path = state.config.data_dir.join("last-index-work.json");
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 32768 {
            return Err(VrgError::AccessDenied("invalid index work receipt".into()));
        }
        let receipt: Receipt = serde_json::from_slice(&std::fs::read(path)?)
            .map_err(|_| VrgError::Storage("invalid index work receipt".into()))?;
        serde_json::to_value(receipt)
            .map(Some)
            .map_err(|error| VrgError::Storage(error.to_string()))
    }

    pub fn projection_update_status(&self) -> VrgResult<Option<serde_json::Value>> {
        let state = self.lock_state()?;
        crate::projection::updates::pending_review(&state.config)
    }

    pub fn adaptive_status(&self) -> VrgResult<serde_json::Value> {
        let state = self.lock_state()?;
        let raw = state.store.load_adaptive_state()?;
        let memories = crate::adaptive::load_memories(raw.as_deref())?;
        let bytes = derived_disk_bytes(&state.config.data_dir)?;
        let budget = state
            .config
            .adaptive_memory
            .soft_store_budget_mib
            .saturating_mul(1024 * 1024);
        Ok(
            serde_json::json!({"physical_store_bytes":bytes,"soft_budget_bytes":budget,"capacity_pressure":bytes>budget,
            "enabled":state.config.adaptive_memory.enabled,
            "generation_id":state.store.current_generation_id()?,
            "classification":if memories.is_empty(){"pending"}else{"proposed_derived"},
            "memories":memories.len(), "stats":state.store.storage_stats()?,
            "source_writes":false,"provider_tokens":"unavailable"}),
        )
    }

    /// Export attributed source metadata, never an automatic AI classification.
    pub fn adaptive_plan(&self) -> VrgResult<serde_json::Value> {
        let state = self.lock_state()?;
        let scan = vault::scan_roots(&state.config)?;
        if !scan.issues.is_empty() {
            return Err(VrgError::AccessDenied("source scan is incomplete".into()));
        }
        let (project_id, privacy_scope_id) = adaptive_scope(&state.config)?;
        let sources = scan.documents.iter().map(|doc| serde_json::json!({
            "source_id":crate::adaptive::source_id(&doc.source), "revision_sha256":doc.revision,
            "raw_bytes":doc.raw_markdown.len(),"title":doc.title
        })).collect::<Vec<_>>();
        Ok(
            serde_json::json!({"schema_version":1,"project_id":project_id,
            "privacy_scope_id":privacy_scope_id,"expected_generation":state.store.current_generation_id()?,
            "classification":"pending_codex_proposal","sources":sources,
            "allowed_actions":["keep","set_tier","upsert_summary"]}),
        )
    }

    pub fn apply_memory_proposal(&self, bytes: &[u8]) -> VrgResult<serde_json::Value> {
        let proposal = crate::adaptive::parse_proposal(bytes)?;
        let state = self.lock_state()?;
        require_adaptive(&state.config)?;
        let _sync = state
            .store
            .try_sync_lock()?
            .ok_or_else(|| VrgError::Conflict("index writer is active".into()))?;
        let generation_id = state
            .store
            .current_generation_id()?
            .ok_or_else(|| VrgError::Conflict("no indexed generation".into()))?;
        let (project_id, privacy_scope_id) = adaptive_scope(&state.config)?;
        let mut sources = std::collections::BTreeMap::new();
        for op in &proposal.operations {
            for span in &op.sources {
                let source = source_from_memory_id(&span.source_id)
                    .ok_or_else(|| VrgError::AccessDenied("invalid source identity".into()))?;
                let doc = vault::read_source(
                    &state.config,
                    &source.root_id,
                    &source.relative_path,
                    4 * 1024 * 1024,
                )?;
                sources.insert(source, doc);
            }
        }
        let raw = state.store.load_adaptive_state()?;
        let context = crate::adaptive::ValidationContext {
            project_id,
            privacy_scope_id,
            generation_id: generation_id.clone(),
            sources: sources.values().cloned().collect(),
            existing: crate::adaptive::load_memories(raw.as_deref())?,
        };
        let validated = crate::adaptive::validate_proposal(&proposal, &context)?;
        // Optimistic source preconditions are checked again after semantic validation.
        for doc in sources.values() {
            let current = vault::read_source(
                &state.config,
                &doc.source.root_id,
                &doc.source.relative_path,
                4 * 1024 * 1024,
            )?;
            if current.revision != doc.revision {
                return Err(VrgError::Conflict(
                    "source changed while applying proposal".into(),
                ));
            }
        }
        let state_json = serde_json::to_string(&validated.memories)
            .map_err(|e| VrgError::Storage(e.to_string()))?;
        let applied = state.store.apply_adaptive_state(
            &generation_id,
            &proposal.proposal_id,
            &validated.fingerprint,
            &state_json,
        )?;
        Ok(
            serde_json::json!({"applied":applied,"replayed":!applied,"proposal_id":proposal.proposal_id,
            "memories":validated.memories.len(),"source_writes":false}),
        )
    }

    pub fn adaptive_maintain(&self, apply: bool) -> VrgResult<serde_json::Value> {
        let state = self.lock_state()?;
        require_adaptive(&state.config)?;
        let plan = state
            .store
            .maintenance_plan(state.config.adaptive_memory.retain_generations)?;
        let archive = crate::adaptive_codec::ArchiveStore::open(&state.config.data_dir)?;
        if !apply {
            let retained = state.store.projected_source_revisions(&plan)?;
            let protected = archive_protections(&state.config, retained)?;
            let cold = archive.plan_gc(&protected)?;
            return Ok(serde_json::json!({"database":plan,"cold":cold,"applied":false}));
        }
        let database = state.store.apply_maintenance(&plan)?;
        // Keep compatible publishers/proposal writers out while deciding COLD reachability.
        let _sync = state.store.try_sync_lock()?.ok_or_else(|| {
            VrgError::Conflict("publication active; defer cold maintenance".into())
        })?;
        let cold = state.store.with_retained_source_revisions(|retained| {
            let protected = archive_protections(&state.config, retained)?;
            let cold_plan = archive.plan_gc(&protected)?;
            archive.apply_gc(&cold_plan, &protected)
        })?;
        let bytes = derived_disk_bytes(&state.config.data_dir)?;
        let budget = state
            .config
            .adaptive_memory
            .soft_store_budget_mib
            .saturating_mul(1024 * 1024);
        Ok(
            serde_json::json!({"database":database,"cold":cold,"applied":true,
            "physical_store_bytes":bytes,"capacity_pressure":bytes>budget,
            "soft_budget_bytes":budget,"source_writes":false}),
        )
    }

    pub fn archive_source(
        &self,
        root_id: &str,
        relative_path: &str,
    ) -> VrgResult<serde_json::Value> {
        let state = self.lock_state()?;
        require_adaptive(&state.config)?;
        let doc = vault::read_source(&state.config, root_id, relative_path, 4 * 1024 * 1024)?;
        let (_, scope) = adaptive_scope(&state.config)?;
        let archive = crate::adaptive_codec::ArchiveStore::open(&state.config.data_dir)?;
        let source_id = crate::adaptive::source_id(&doc.source);
        let object = archive.archive_checked(
            &scope,
            &source_id,
            &doc.revision,
            doc.raw_markdown.as_bytes(),
            || {
                let current =
                    vault::read_source(&state.config, root_id, relative_path, 4 * 1024 * 1024)?;
                if current.revision != doc.revision {
                    return Err(VrgError::Conflict(
                        "source changed during archive staging".into(),
                    ));
                }
                Ok(())
            },
        )?;
        Ok(
            serde_json::json!({"status":if object.codec == "raw" {"not_beneficial"} else {"archived"},"object":object}),
        )
    }

    /// Returns exact historical bytes only while the source remains authorized.
    /// This method never overwrites a Markdown source.
    pub fn restore_archive(
        &self,
        root_id: &str,
        relative_path: &str,
        revision: &str,
    ) -> VrgResult<serde_json::Value> {
        let state = self.lock_state()?;
        require_adaptive(&state.config)?;
        let doc = vault::read_source(&state.config, root_id, relative_path, 4 * 1024 * 1024)?;
        let (_, scope) = adaptive_scope(&state.config)?;
        let archive = crate::adaptive_codec::ArchiveStore::open(&state.config.data_dir)?;
        let bytes = archive.restore(&scope, &crate::adaptive::source_id(&doc.source), revision)?;
        let text = String::from_utf8(bytes)
            .map_err(|_| VrgError::Storage("archive is not UTF8 source".into()))?;
        Ok(
            serde_json::json!({"source":doc.source,"revision":revision,"representation":"exact_archive",
            "derived":false,"raw_markdown":text,"source_writes":false}),
        )
    }

    /// Produce a reviewable plan from a current, bounded read. No method in
    /// this build applies the plan to a source note.
    pub fn plan_note_write(
        &self,
        root_id: &str,
        relative_path: &str,
        proposed_markdown: String,
        reason: String,
    ) -> VrgResult<WritePlan> {
        let state = self.lock_state()?;
        note_write::prepare_plan(
            &state.config,
            root_id,
            relative_path,
            proposed_markdown,
            reason,
        )
    }

    fn lock_state(&self) -> VrgResult<std::sync::MutexGuard<'_, EngineState>> {
        self.state
            .lock()
            .map_err(|_| VrgError::Recovery("engine lock was poisoned".into()))
    }
}

fn failed_engine_status(reason: &str) -> EngineStatus {
    EngineStatus {
        runtime_identity: None,
        generation_id: None,
        configured_roots: 0,
        text: ModuleStatus::Failed(reason.into()),
        semantic: ModuleStatus::Failed(reason.into()),
        graph: ModuleStatus::Failed(reason.into()),
        storage: ModuleStatus::Failed(reason.into()),
        note_writes: ModuleStatus::Disabled,
        remote_chatgpt: ModuleStatus::Disabled,
        index_sync: IndexSyncStatus::Failed {
            generation_id: None,
            reason: reason.into(),
            failed_unix_ms: unix_ms(),
        },
        change_monitor: ModuleStatus::Failed(reason.into()),
    }
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or_default()
}

fn change_worker(
    state: Weak<Mutex<EngineState>>,
    epoch: Arc<AtomicU64>,
    handled_epoch: Arc<AtomicU64>,
    signal: Arc<(Mutex<bool>, Condvar)>,
) {
    loop {
        let pending = match signal.0.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };
        // The initial signal is already set before the worker starts. Waiting
        // unconditionally can lose that wakeup and defer recovery for a minute.
        let waited = signal
            .1
            .wait_timeout_while(pending, Duration::from_secs(60), |pending| !*pending);
        let (mut pending_guard, timeout) = match waited {
            Ok(value) => value,
            Err(_) => return,
        };
        let signalled = *pending_guard;
        *pending_guard = false;
        drop(pending_guard);
        let Some(state_arc) = state.upgrade() else {
            return;
        };
        let current_epoch = epoch.load(Ordering::Acquire);
        let dirty = current_epoch != handled_epoch.load(Ordering::Acquire);
        if dirty && signalled && handled_epoch.load(Ordering::Acquire) != 0 {
            let Ok(pending) = signal.0.lock() else {
                return;
            };
            // The predicate remains true until timeout, even across notifications.
            let Ok((mut pending, _)) =
                signal
                    .1
                    .wait_timeout_while(pending, Duration::from_secs(30), |_| true)
            else {
                return;
            };
            *pending = false;
        }
        let Ok(mut state) = state_arc.lock() else {
            continue;
        };
        if !(dirty && (signalled || timeout.timed_out())) || state.config.roots.is_empty() {
            continue;
        }
        let observed = epoch.load(Ordering::Acquire);
        let Ok(Some(_sync_lock)) = state.store.try_sync_lock() else {
            continue;
        };
        let event_key = format!("watch:{}:{}", std::process::id(), observed);
        let parent = state.store.current_generation_id().ok().flatten();
        let change_revision = state.store.source_change_revision().unwrap_or_default();
        let _ = state.store.set_index_sync_status(
            "indexing",
            parent.as_deref(),
            Some(unix_ms() as i64),
            None,
            Some(&event_key),
        );
        match index_from_events(&mut state, false) {
            Ok(report) => {
                state.semantic_status = report.semantic_status;
                let _ = state.store.finish_index_sync(
                    change_revision,
                    report.generation_id.as_deref(),
                    unix_ms() as i64,
                    Some(&event_key),
                );
                state.last_reconcile_unix_ms = unix_ms();
                if epoch.load(Ordering::Acquire) == observed
                    && matches!(
                        state.store.index_sync_status(),
                        Ok(IndexSyncStatus::UpToDate { .. })
                    )
                {
                    handled_epoch.store(observed, Ordering::Release);
                }
            }
            Err(error) => {
                let generation = state.store.current_generation_id().ok().flatten();
                let reason = error.to_string();
                let _ = state.store.set_index_sync_status(
                    "failed",
                    generation.as_deref(),
                    Some(unix_ms() as i64),
                    Some(&reason),
                    Some(&event_key),
                );
            }
        }
    }
}

fn source_from_memory_id(id: &str) -> Option<SourceRef> {
    crate::adaptive::parse_source_id(id)
}

fn edge_authorized(config: &VrgConfig, store: &DerivedStore, edge: &GraphEdge) -> bool {
    // Incoming edges carry facts asserted by `from`, even when a result is for `to`.
    let Ok(author) = vault::current_revision(config, &edge.from) else {
        return false;
    };
    let indexed = store.load_document(&edge.from).ok().flatten();
    if !indexed.is_some_and(|document| document.revision == author) {
        return false;
    }
    edge.to
        .as_ref()
        .is_some_and(|target| vault::current_revision(config, target).is_ok())
}

fn require_adaptive(config: &VrgConfig) -> VrgResult<()> {
    if !config.adaptive_memory.enabled {
        return Err(VrgError::AccessDenied(
            "adaptive memory requires explicit project opt-in".into(),
        ));
    }
    Ok(())
}

pub(crate) fn adaptive_scope(config: &VrgConfig) -> VrgResult<(String, String)> {
    use sha2::{Digest, Sha256};
    // Local scope identity, not a permission grant. Canonical roots and filenames
    // separate revoked/reconfigured profiles; the source reader remains authoritative.
    let mut roots = config.roots.clone();
    roots.sort_by(|a, b| a.id.cmp(&b.id));
    for root in &mut roots {
        root.path = root.path.canonicalize()?;
    }
    let encoded = serde_json::to_vec(&roots).map_err(|e| VrgError::Storage(e.to_string()))?;
    let scope = hex::encode(Sha256::digest(&encoded));
    let project = hex::encode(Sha256::digest(config.data_dir.to_string_lossy().as_bytes()));
    Ok((project, scope))
}

fn archive_protections(
    config: &VrgConfig,
    retained: Vec<(SourceRef, String)>,
) -> VrgResult<std::collections::BTreeSet<(String, String)>> {
    let scan = vault::scan_roots(config)?;
    if !scan.issues.is_empty() {
        return Err(VrgError::AccessDenied(
            "incomplete source scan; cold GC deferred".into(),
        ));
    }
    let mut protected = retained
        .into_iter()
        .map(|(source, rev)| (crate::adaptive::source_id(&source), rev))
        .collect::<std::collections::BTreeSet<_>>();
    protected.extend(scan.documents.iter().map(|doc| {
        (
            crate::adaptive::source_id(&doc.source),
            doc.revision.clone(),
        )
    }));
    Ok(protected)
}

fn derived_disk_bytes(path: &std::path::Path) -> VrgResult<u64> {
    let mut total = 0u64;
    for name in ["vrg.sqlite3", "vrg.sqlite3-wal", "vrg.sqlite3-shm"] {
        let file = path.join(name);
        if let Ok(metadata) = std::fs::symlink_metadata(&file) {
            if metadata.file_type().is_symlink() {
                return Err(VrgError::SymlinkRejected("unsafe derived file".into()));
            }
            total = total.saturating_add(metadata.len());
        }
    }
    let cold = path.join("cold");
    match std::fs::symlink_metadata(&cold) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(VrgError::SymlinkRejected("unsafe cold directory".into()));
            }
            for (index, item) in std::fs::read_dir(cold)?.take(4097).enumerate() {
                if index == 4096 {
                    return Err(VrgError::Storage(
                        "cold catalog exceeds bounded stats capacity".into(),
                    ));
                }
                let item = item?;
                let metadata = std::fs::symlink_metadata(item.path())?;
                if metadata.file_type().is_symlink() {
                    return Err(VrgError::SymlinkRejected("unsafe cold object".into()));
                }
                if metadata.is_file() {
                    total = total.saturating_add(metadata.len());
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    Ok(total)
}

fn index_from_events(state: &mut EngineState, force_full: bool) -> VrgResult<IndexReport> {
    let changes = match &state.monitor {
        Some(monitor) => {
            let mut pending = monitor
                .changes
                .lock()
                .map_err(|_| VrgError::Storage("source event queue poisoned".into()))?;
            std::mem::take(&mut *pending)
        }
        None => SourceChanges {
            full: true,
            paths: BTreeSet::new(),
        },
    };
    let result = if force_full || changes.full {
        indexer::index(&state.config, &state.store, state.embedder.as_ref())
    } else {
        indexer::index_paths(
            &state.config,
            &state.store,
            state.embedder.as_ref(),
            &changes.paths,
        )
    };
    if result.is_err() {
        if let Some(monitor) = &state.monitor {
            if let Ok(mut pending) = monitor.changes.lock() {
                pending.full |= changes.full;
                pending.paths.extend(changes.paths);
            }
        }
    }
    result
}
