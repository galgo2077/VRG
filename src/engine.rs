use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Condvar, Mutex, Weak,
    },
    thread,
    time::Duration,
};

use notify::{
    event::{AccessKind, AccessMode},
    EventKind, RecommendedWatcher, RecursiveMode, Watcher,
};

use crate::{
    config::VrgConfig,
    contracts::{
        EngineStatus, GraphEdge, IndexReport, IndexSyncStatus, ModuleStatus, RuntimeIdentity,
        SearchRequest, SearchResponse, SearchSyncState, SourceDocument, SourceRef, StopSyncOutcome,
        WritePlan,
    },
    error::{VrgError, VrgResult},
    graph::{self, GraphSnapshot},
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
            Some(model_config) => match LocalEmbedder::load(model_config) {
                Ok(embedder) => (Some(embedder), ModuleStatus::Ready),
                Err(error) => (None, ModuleStatus::Degraded(error.to_string())),
            },
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
            semantic: state.semantic_status.clone(),
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
        let callback_epoch = Arc::clone(&epoch);
        let callback_signal = Arc::clone(&signal);
        let callback_roots = roots.clone();
        let callback_store = state.store.clone();
        let watcher_result =
            notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
                let relevant = match result {
                    Ok(event) => {
                        let read_only_access = matches!(
                            event.kind,
                            EventKind::Access(
                                AccessKind::Read
                                    | AccessKind::Open(AccessMode::Any | AccessMode::Read)
                                    | AccessKind::Close(AccessMode::Read)
                            )
                        );
                        !read_only_access
                            && event.paths.iter().any(|path| {
                                callback_roots.iter().any(|(root_path, root_config)| {
                                    let Ok(relative) = path.strip_prefix(root_path) else {
                                        return false;
                                    };
                                    if crate::vault::is_excluded(relative, root_config) {
                                        return false;
                                    }
                                    if crate::vault::is_markdown(relative) {
                                        return true;
                                    }
                                    path.is_dir() || !path.exists()
                                })
                            })
                    }
                    Err(_) => true,
                };
                if relevant {
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
        for (path, _) in &roots {
            if let Err(error) = watcher.watch(path, RecursiveMode::Recursive) {
                state.monitor_status = ModuleStatus::Degraded(format!(
                    "filesystem notifications unavailable: {error}"
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
            && matches!(state.store.index_sync_status()?, IndexSyncStatus::UpToDate { checked_unix_ms, .. } if unix_ms().saturating_sub(checked_unix_ms) < 60_000)
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
        match indexer::index(&state.config, &state.store, state.embedder.as_ref()) {
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
        self.start_change_monitor()?;
        let (sync_state, sync_reason) = self.refresh_if_needed(allow_stale)?;
        let state = self.lock_state()?;
        if matches!(&sync_state, SearchSyncState::Disabled) {
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
        let mut response = retrieval::retrieve(&state.store, state.embedder.as_ref(), &request)?;
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
        let monitor_ready = matches!(&state.monitor_status, ModuleStatus::Ready)
            || state.store.monitor_is_active()?;
        let index_status = state.store.index_sync_status()?;
        let status_recent = matches!(
            index_status,
            IndexSyncStatus::UpToDate { checked_unix_ms, .. }
                if unix_ms().saturating_sub(checked_unix_ms) < 60_000
        );
        let periodic_reconcile_due =
            unix_ms().saturating_sub(state.last_reconcile_unix_ms) >= 60_000;
        let needs_reconcile = !monitor_ready
            || current_epoch != handled_epoch
            || !status_recent
            || periodic_reconcile_due && state.monitor.is_some();
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
        vault::read_source(&state.config, root_id, relative_path, max_bytes)
    }

    pub fn neighbors(&self, source: &SourceRef, limit: usize) -> VrgResult<Vec<GraphEdge>> {
        let (freshness, reason) = self.refresh_if_needed(false)?;
        if !matches!(freshness, SearchSyncState::Ready) {
            return Err(VrgError::Storage(
                reason.unwrap_or_else(|| "derived graph is not current".into()),
            ));
        }
        let state = self.lock_state()?;
        let Some(snapshot) = state.store.load_current_snapshot()? else {
            return Ok(Vec::new());
        };
        let graph = GraphSnapshot {
            nodes: snapshot
                .documents
                .iter()
                .map(|document| document.source.clone())
                .collect(),
            edges: snapshot.edges,
        };
        Ok(graph::adjacent_edges(&graph, source, limit))
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
        let Ok(mut state) = state_arc.lock() else {
            continue;
        };
        let periodic = unix_ms().saturating_sub(state.last_reconcile_unix_ms) >= 60_000;
        if !(dirty && signalled || timeout.timed_out() && periodic) || state.config.roots.is_empty()
        {
            continue;
        }
        if dirty {
            thread::sleep(Duration::from_millis(250));
        }
        let observed = epoch.load(Ordering::Acquire);
        let Ok(Some(_sync_lock)) = state.store.try_sync_lock() else {
            continue;
        };
        // A recently published status cannot cover changes made while no
        // watcher was running. First reconcile once in this process before
        // treating a recent status as evidence that the monitor has caught up.
        if state.last_reconcile_unix_ms != 0
            && matches!(state.store.index_sync_status(), Ok(IndexSyncStatus::UpToDate { checked_unix_ms, .. }) if unix_ms().saturating_sub(checked_unix_ms) < 60_000)
        {
            handled_epoch.store(observed, Ordering::Release);
            continue;
        }
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
        match indexer::index(&state.config, &state.store, state.embedder.as_ref()) {
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
