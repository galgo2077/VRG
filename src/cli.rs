use std::{
    collections::BTreeMap,
    env, fs,
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
};

use clap::{Args, Parser, Subcommand};
use serde::Serialize;

use crate::{
    config::VrgConfig,
    contracts::{RuntimeIdentity, RuntimeRootIdentity, SearchRequest, SourceRef},
    error::{VrgError, VrgResult},
    mcp_server, VrgEngine,
};

const MAX_QUERY_BYTES: usize = 16 * 1024;
const MAX_RESULTS: usize = 100;

#[derive(Debug, Parser)]
#[command(name = "vrg", version, about = "Local-first Vault RAG Graph")]
struct Cli {
    /// Use this configuration file instead of the standard VRG config path.
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    #[command(name = "__verify-onnx-runtime", hide = true)]
    VerifyOnnxRuntime { path: PathBuf },
    /// Plan or configure an explicitly authorized VRG profile.
    Setup(SetupArgs),
    /// Inspect configuration without opening or changing derived storage.
    Doctor(DoctorArgs),
    /// Create an empty-root configuration without overwriting an existing file.
    Init {
        /// Destination configuration file (default: XDG config path).
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Report configured modules and active derived generation.
    Status,
    /// Explicitly scan configured roots and publish a derived generation.
    Index,
    /// Refresh the incremental derived index for a Codex Stop hook event.
    CodexStop,
    /// Reconcile authorized roots when a Codex session starts or resumes.
    CodexStart,
    /// Prepare a per-turn documentation check for a trusted Codex hook.
    CodexTurn,
    /// Declare the documentation outcome for the current Codex turn.
    CodexClose {
        #[arg(long)]
        token: String,
        #[arg(long, value_enum)]
        result: CloseResult,
        /// Required for no-new; a bounded category, never free-form text.
        #[arg(long, value_enum)]
        reason: Option<CloseReason>,
    },
    /// Manage the optional event-only session memory.
    Memory {
        #[command(subcommand)]
        command: MemoryCommand,
    },
    /// Restore the active derived generation to a retained generation ID.
    Rollback { generation_id: String },
    /// Search the current derived generation.
    Search(SearchArgs),
    /// List explicit graph neighbors/backlinks for one indexed source.
    Graph {
        root_id: String,
        relative_path: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Run the local stdio MCP server.
    Mcp {
        #[command(subcommand)]
        command: McpCommand,
    },
}

#[derive(Debug, Args)]
struct SetupArgs {
    /// Recover an interrupted VRG-owned transaction from its private backup.
    #[arg(long)]
    recover: Option<PathBuf>,
    /// Existing project directory to configure.
    #[arg(long)]
    project: Option<PathBuf>,
    /// Explicitly authorized Markdown source directory (repeatable).
    #[arg(long = "root")]
    roots: Vec<PathBuf>,
    /// Create an isolated sample source within the selected project.
    #[arg(long, conflicts_with = "roots")]
    synthetic: bool,
    /// Dedicated derived data directory.
    #[arg(long)]
    data_dir: Option<PathBuf>,
    /// Existing compatible local model directory.
    #[arg(long, conflicts_with = "text_only")]
    model_dir: Option<PathBuf>,
    /// Download the pinned compatible model to this dedicated path (explicit consent).
    #[arg(long, conflicts_with_all = ["model_dir", "text_only"])]
    download_model: Option<PathBuf>,
    /// Select text-only retrieval explicitly.
    #[arg(long, conflicts_with_all = ["model_dir", "download_model"])]
    text_only: bool,
    /// Configure Codex for the selected project.
    #[arg(long, conflicts_with = "cli_only")]
    codex: bool,
    /// Authorize the configured Codex model to receive retrieved fragments from selected roots.
    #[arg(long, requires = "codex")]
    allow_remote_context: bool,
    /// Configure only the VRG command line.
    #[arg(long, conflicts_with = "codex")]
    cli_only: bool,
    /// Explicitly authorize operational memory under this project directory.
    #[arg(long)]
    memory_dir: Option<PathBuf>,
    /// Print the proposed changes without creating files or downloading.
    #[arg(long)]
    dry_run: bool,
    /// Do not ask interactive questions; require explicit choices.
    #[arg(long)]
    non_interactive: bool,
    /// Confirm the displayed bounded plan; does not authorize hook trust.
    #[arg(long)]
    yes: bool,
    /// Prohibit network access.
    #[arg(long)]
    offline: bool,
    /// Consent to build ONNX Runtime from pinned official sources on macOS Intel.
    #[arg(long)]
    build_onnx_runtime: bool,
}

#[derive(Debug, Args)]
struct DoctorArgs {
    /// Output a machine-readable report.
    #[arg(long)]
    json: bool,
    /// Run explicitly requested isolated runtime checks.
    #[arg(long)]
    check_runtime: bool,
    /// Repair recognized VRG-owned problems only.
    #[arg(long)]
    fix: bool,
    /// Show the repair plan without applying it.
    #[arg(long, requires = "fix")]
    dry_run: bool,
}

#[derive(Debug, Args)]
struct SearchArgs {
    query: String,
    #[arg(long, default_value_t = 10)]
    limit: usize,
    /// Restrict search to configured root IDs (repeatable).
    #[arg(long = "root")]
    roots: Vec<String>,
    /// Disable local semantic retrieval and use exact/full-text search only.
    #[arg(long)]
    text_only: bool,
    /// Return the last complete generation when source synchronization is pending or failed.
    #[arg(long)]
    allow_stale: bool,
    #[arg(long, default_value_t = 0)]
    graph_hops: u8,
}

#[derive(Debug, Subcommand)]
enum McpCommand {
    Serve,
}

#[derive(Clone, Debug, clap::ValueEnum)]
enum CloseResult {
    Documented,
    NoNew,
}

#[derive(Clone, Debug, clap::ValueEnum)]
enum CloseReason {
    NoChange,
    Trivial,
    AlreadyRecorded,
    NoDurableResult,
}

#[derive(Debug, Subcommand)]
enum MemoryCommand {
    Status,
    Events {
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    Prune,
    /// Delete only VRG-derived session event metadata; source files are untouched.
    Clear {
        #[arg(long)]
        confirm: bool,
    },
}

pub async fn run() -> VrgResult<()> {
    let cli = Cli::parse();
    match cli.command {
        #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
        Command::VerifyOnnxRuntime { path } => crate::runtime_probe::inspect_api(&path),
        Command::Setup(args) => run_setup(cli.config, args),
        Command::Doctor(args) => run_doctor(cli.config, args),
        Command::Init { output } => {
            let destination = match output.or(cli.config) {
                Some(path) => path,
                None => default_config_path()?,
            };
            write_default_config(&destination)?;
            println!("created empty-root VRG config at {}", destination.display());
            Ok(())
        }
        Command::Status => {
            let engine = open_engine(cli.config)?;
            print_json(&engine.status())
        }
        Command::Index => {
            let engine = open_engine(cli.config)?;
            print_json(&engine.index()?)
        }
        Command::CodexStop => {
            crate::auto_sync::run_stop_hook(cli.config)?;
            Ok(())
        }
        Command::CodexStart => {
            crate::auto_sync::run_start_hook(cli.config)?;
            Ok(())
        }
        Command::CodexTurn => crate::auto_sync::run_turn_hook(cli.config),
        Command::CodexClose {
            token,
            result,
            reason,
        } => {
            let result = match result {
                CloseResult::Documented => "documented",
                CloseResult::NoNew => "no-new",
            };
            let reason = reason.map(|reason| match reason {
                CloseReason::NoChange => "no-change",
                CloseReason::Trivial => "trivial",
                CloseReason::AlreadyRecorded => "already-recorded",
                CloseReason::NoDurableResult => "no-durable-result",
            });
            crate::auto_sync::attest_close(cli.config, &token, result, reason)
        }
        Command::Memory { command } => {
            let engine = open_engine(cli.config)?;
            match command {
                MemoryCommand::Status => print_json(&serde_json::json!({
                    "capture_enabled": engine.capture_session_events(),
                    "stored_events": engine.session_event_count()?,
                })),
                MemoryCommand::Events { limit } => {
                    let cwd = env::current_dir()?.to_string_lossy().to_string();
                    print_json(&engine.recent_session_events(&cwd, limit)?)
                }
                MemoryCommand::Prune => print_json(&serde_json::json!({
                    "deleted_events": engine.prune_session_events()?,
                })),
                MemoryCommand::Clear { confirm: true } => print_json(&serde_json::json!({
                    "deleted_events": engine.clear_session_events()?,
                })),
                MemoryCommand::Clear { confirm: false } => Err(VrgError::InvalidConfig(
                    "memory clear requires --confirm".into(),
                )),
            }
        }
        Command::Rollback { generation_id } => {
            let engine = open_engine(cli.config)?;
            print_json(&engine.rollback(&generation_id)?)
        }
        Command::Search(args) => {
            if args.query.trim().is_empty() || args.query.len() > MAX_QUERY_BYTES {
                return Err(VrgError::InvalidConfig(format!(
                    "query must be non-empty and at most {MAX_QUERY_BYTES} bytes"
                )));
            }
            if args.limit == 0 || args.limit > MAX_RESULTS {
                return Err(VrgError::InvalidConfig(format!(
                    "limit must be between 1 and {MAX_RESULTS}"
                )));
            }
            if args.graph_hops > 2 {
                return Err(VrgError::InvalidConfig(
                    "graph_hops must be between 0 and 2".into(),
                ));
            }
            let engine = open_engine(cli.config)?;
            let roots = (!args.roots.is_empty()).then_some(args.roots);
            print_json(&engine.search_with_policy(
                SearchRequest {
                    query: args.query,
                    limit: args.limit,
                    root_ids: roots,
                    use_semantic: !args.text_only,
                    graph_hops: args.graph_hops,
                },
                args.allow_stale,
            )?)
        }
        Command::Graph {
            root_id,
            relative_path,
            limit,
        } => {
            if limit == 0 || limit > MAX_RESULTS {
                return Err(VrgError::InvalidConfig(format!(
                    "limit must be between 1 and {MAX_RESULTS}"
                )));
            }
            let engine = open_engine(cli.config)?;
            print_json(&engine.neighbors(
                &SourceRef {
                    root_id,
                    relative_path,
                },
                limit,
            )?)
        }
        Command::Mcp {
            command: McpCommand::Serve,
        } => {
            let engine = open_engine(cli.config)?;
            mcp_server::serve(engine).await
        }
    }
}

fn open_engine(config_path: Option<PathBuf>) -> VrgResult<VrgEngine> {
    let path = match config_path.or_else(|| env::var_os("VRG_CONFIG").map(PathBuf::from)) {
        Some(path) => path,
        None => default_config_path()?,
    };
    let (config, config_path, config_sha256) = VrgConfig::load_with_identity(path)?;
    let roots = config
        .roots
        .iter()
        .map(|root| {
            Ok(RuntimeRootIdentity {
                id: root.id.clone(),
                canonical_path: root
                    .path
                    .canonicalize()
                    .map_err(|_| {
                        VrgError::InvalidConfig("cannot canonicalize configured root".into())
                    })?
                    .display()
                    .to_string(),
            })
        })
        .collect::<VrgResult<Vec<_>>>()?;
    let mut selection_environment = BTreeMap::new();
    for name in [
        "VRG_CONFIG",
        "VRG_DATA_DIR",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
    ] {
        selection_environment.insert(name.to_owned(), env::var(name).ok());
    }
    let runtime_identity = RuntimeIdentity {
        config_path: config_path.display().to_string(),
        config_sha256,
        server_executable: env::current_exe()
            .map_err(|_| VrgError::InvalidConfig("cannot identify server executable".into()))?
            .display()
            .to_string(),
        server_args: env::args_os()
            .skip(1)
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect(),
        server_cwd: env::current_dir()
            .map_err(|_| {
                VrgError::InvalidConfig("cannot identify server working directory".into())
            })?
            .display()
            .to_string(),
        selection_environment,
        roots,
        data_dir: config.canonical_data_dir()?.display().to_string(),
    };
    VrgEngine::open_with_runtime_identity(config, Some(runtime_identity))
}

fn default_config_path() -> VrgResult<PathBuf> {
    if let Some(path) = env::var_os("XDG_CONFIG_HOME") {
        let path = PathBuf::from(path);
        if !path.is_absolute() {
            return Err(VrgError::InvalidConfig(
                "XDG_CONFIG_HOME must be an absolute path".into(),
            ));
        }
        return Ok(path.join("vrg/config.toml"));
    }
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| VrgError::InvalidConfig("HOME is not set".into()))?;
    Ok(home.join(".config/vrg/config.toml"))
}

fn write_default_config(destination: &Path) -> VrgResult<()> {
    if !destination.is_absolute() {
        return Err(VrgError::InvalidConfig(
            "configuration destination must be absolute".into(),
        ));
    }
    let parent = destination.parent().ok_or_else(|| {
        VrgError::InvalidConfig("configuration destination has no parent directory".into())
    })?;
    fs::create_dir_all(parent)?;

    let config = VrgConfig::default();
    config.validate()?;
    let serialized = toml::to_string_pretty(&config)
        .map_err(|error| VrgError::InvalidConfig(error.to_string()))?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                VrgError::AccessDenied(format!(
                    "refusing to overwrite existing config {}",
                    destination.display()
                ))
            } else {
                VrgError::Io(error)
            }
        })?;
    file.write_all(serialized.as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}

fn print_json(value: &impl Serialize) -> VrgResult<()> {
    let json = serde_json::to_string_pretty(value)
        .map_err(|error| VrgError::InvalidConfig(error.to_string()))?;
    println!("{json}");
    Ok(())
}

fn ask(prompt: &str) -> VrgResult<String> {
    eprint!("{prompt}");
    io::stderr().flush()?;
    let mut answer = String::new();
    if io::stdin().read_line(&mut answer)? == 0 {
        return Err(VrgError::InvalidConfig(
            "input ended; supply explicit setup options".into(),
        ));
    }
    Ok(answer.trim().to_owned())
}

fn absolute(path: PathBuf) -> VrgResult<PathBuf> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(env::current_dir()?.join(path))
    }
}

/// Resolve existing ancestor aliases (such as macOS /var) before planning.
/// A selected leaf symlink or parent traversal is never accepted.
fn normalized(path: PathBuf) -> VrgResult<PathBuf> {
    let path = absolute(path)?;
    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(VrgError::InvalidConfig(
            "path contains parent traversal".into(),
        ));
    }
    if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(VrgError::SymlinkRejected(path.display().to_string()));
    }
    let mut cursor = path.as_path();
    let mut suffix = Vec::new();
    while !cursor.exists() {
        let name = cursor
            .file_name()
            .ok_or_else(|| VrgError::InvalidConfig("path has no existing ancestor".into()))?;
        suffix.push(name.to_os_string());
        cursor = cursor
            .parent()
            .ok_or_else(|| VrgError::InvalidConfig("path has no parent".into()))?;
    }
    let mut resolved = cursor.canonicalize()?;
    for name in suffix.iter().rev() {
        resolved.push(name);
    }
    Ok(resolved)
}

fn find_program(name: &str) -> Option<PathBuf> {
    env::var_os("PATH")
        .and_then(|paths| {
            env::split_paths(&paths)
                .map(|p| p.join(name))
                .find(|p| p.is_file())
        })
        .and_then(|p| p.canonicalize().ok())
}

fn run_setup(explicit_config: Option<PathBuf>, mut args: SetupArgs) -> VrgResult<()> {
    crate::setup::ensure_supported_platform()?;
    #[cfg(not(all(target_os = "macos", target_arch = "x86_64")))]
    if args.build_onnx_runtime {
        return Err(VrgError::InvalidConfig(
            "--build-onnx-runtime is only available on macOS Intel".into(),
        ));
    }
    let interactive = !args.non_interactive && io::stdin().is_terminal();
    if let Some(backup) = args.recover.take() {
        let backup = absolute(backup)?;
        println!(
            "Recovery plan: inspect {} and revert only unchanged VRG-owned revisions",
            backup.display()
        );
        if args.dry_run {
            return Ok(());
        }
        if !args.yes
            && (!interactive || !ask("Recover this transaction? [y/N] ")?.eq_ignore_ascii_case("y"))
        {
            return Err(VrgError::InvalidConfig("recovery was not confirmed".into()));
        }
        let count = crate::setup::recover_transaction(&backup)?;
        println!("Recovered {count} file(s); later user changes are never overwritten.");
        return Ok(());
    }
    if args.project.is_none() {
        if !interactive {
            return Err(VrgError::InvalidConfig(
                "supply --project in non-interactive mode".into(),
            ));
        }
        let cwd = env::current_dir()?;
        let prompt = if cwd.join(".git").exists() {
            format!("Project directory [{}]: ", cwd.display())
        } else {
            "Project directory: ".into()
        };
        let answer = ask(&prompt)?;
        if answer.is_empty() && cwd.join(".git").exists() {
            args.project = Some(cwd);
        } else if !answer.is_empty() {
            args.project = Some(answer.into());
        } else {
            return Err(VrgError::InvalidConfig(
                "project directory is required".into(),
            ));
        }
    }
    if args.roots.is_empty() && !args.synthetic {
        if !interactive {
            return Err(VrgError::InvalidConfig(
                "select --synthetic or supply at least one authorized --root".into(),
            ));
        }
        if ask("Start with an isolated synthetic sample instead of your notes? [y/N] ")?
            .eq_ignore_ascii_case("y")
        {
            args.synthetic = true;
        } else {
            let answer = ask("Markdown source directory to authorize (no home-wide discovery): ")?;
            if answer.is_empty() {
                return Err(VrgError::InvalidConfig(
                    "an authorized source directory is required".into(),
                ));
            }
            args.roots.push(answer.into());
        }
    }
    if args.synthetic {
        let project = absolute(args.project.clone().expect("project checked"))?.canonicalize()?;
        args.roots.push(project.join(".vrg-synthetic-notes"));
    }
    if args.model_dir.is_none() && args.download_model.is_none() && !args.text_only {
        if !interactive {
            return Err(VrgError::InvalidConfig(
                "select --text-only, --model-dir or --download-model".into(),
            ));
        }
        let answer = ask("Retrieval: [t]ext, [d]ownload pinned model, or local model directory: ")?;
        if answer.eq_ignore_ascii_case("t") {
            args.text_only = true;
        } else if answer.eq_ignore_ascii_case("d") {
            args.download_model = Some(
                VrgConfig::default()
                    .data_dir
                    .parent()
                    .ok_or_else(|| VrgError::InvalidConfig("data directory has no parent".into()))?
                    .join("vrg-models/multilingual-e5-small"),
            );
        } else if !answer.is_empty() {
            args.model_dir = Some(answer.into());
        } else {
            return Err(VrgError::InvalidConfig(
                "retrieval choice is required".into(),
            ));
        }
    }
    if !args.codex && !args.cli_only {
        if !interactive {
            return Err(VrgError::InvalidConfig(
                "select --codex or --cli-only".into(),
            ));
        }
        let answer = ask("Configure Codex for this project? [y/N] ")?;
        if answer.eq_ignore_ascii_case("y") {
            args.codex = true;
        } else {
            args.cli_only = true;
        }
    }
    if args.codex && !args.allow_remote_context {
        if !interactive || !ask("Allow the configured Codex model to receive retrieved fragments from these authorized roots? [y/N] ")?.eq_ignore_ascii_case("y") {
            return Err(VrgError::InvalidConfig("Codex integration requires explicit --allow-remote-context; use --cli-only otherwise".into()));
        }
        args.allow_remote_context = true;
    }
    if args.codex && args.memory_dir.is_none() && interactive {
        let answer = ask(
            "Authorize compact operational memory under this project? Enter directory or leave empty for none: ",
        )?;
        if !answer.is_empty() {
            args.memory_dir = Some(answer.into());
        }
    }
    if args.non_interactive && !args.yes && !args.dry_run {
        return Err(VrgError::InvalidConfig(
            "non-interactive application requires --yes".into(),
        ));
    }
    let project = normalized(args.project.expect("project checked"))?.canonicalize()?;
    let roots = args
        .roots
        .into_iter()
        .map(normalized)
        .collect::<VrgResult<Vec<_>>>()?;
    let config_path = normalized(
        match explicit_config.or_else(|| env::var_os("VRG_CONFIG").map(PathBuf::from)) {
            Some(path) => path,
            None => default_config_path()?,
        },
    )?;
    let data_dir = match args.data_dir {
        Some(path) => normalized(path)?,
        None => normalized(VrgConfig::default().data_dir)?,
    };
    let memory_dir = args.memory_dir.map(normalized).transpose()?;
    let download = args.download_model.map(normalized).transpose()?;
    let model_dir = match args.model_dir.map(normalized).transpose()? {
        Some(path) => {
            if !path.is_dir() {
                return Err(VrgError::InvalidConfig(
                    "local model directory does not exist".into(),
                ));
            }
            Some(path)
        }
        None => download.clone(),
    };
    if args.offline && download.as_ref().is_some_and(|path| !path.exists()) {
        #[cfg(not(all(target_os = "macos", target_arch = "x86_64")))]
        return Err(VrgError::InvalidConfig(
            "--offline prohibits model download".into(),
        ));
        #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
        eprintln!("Offline mode cannot download the model; setup will retain text-only mode if it is unavailable.");
    }
    println!(
        "Environment: {} / {}; executable: {}",
        env::consts::OS,
        env::consts::ARCH,
        env::current_exe()?.display()
    );
    if args.codex {
        match find_program("codex").and_then(|program| {
            std::process::Command::new(program)
                .arg("--version")
                .output()
                .ok()
        }) {
            Some(result) if result.status.success() => {
                println!("Codex: {}", String::from_utf8_lossy(&result.stdout).trim())
            }
            _ => eprintln!(
                "Codex executable not detected; project configuration will require Codex installation before use."
            ),
        }
    }
    let extra = if args.codex {
        let python = match find_program("python3") {
            Some(path) => path,
            None if memory_dir.is_none() => PathBuf::from("/usr/bin/python3"),
            None => {
                return Err(VrgError::InvalidConfig(
                    "Codex hooks require python3 on PATH".into(),
                ));
            }
        };
        let exe = env::current_exe()?.canonicalize()?;
        crate::setup_codex::plan_codex(
            &project,
            &exe,
            &config_path,
            &data_dir,
            &python,
            memory_dir.as_deref(),
        )
        .map_err(VrgError::InvalidConfig)?
    } else {
        Vec::new()
    };
    // Present the exact proposed VRG handlers when a first review is needed.
    let hook_path = project.join(".codex/hooks.json");
    let hook_bytes = extra
        .iter()
        .find(|file| file.path == hook_path)
        .map(|file| file.content.clone())
        .or_else(|| fs::read(&hook_path).ok());
    let mut hook_review = Vec::new();
    if args.codex && memory_dir.is_some() {
        if let Some(value) = hook_bytes
            .as_deref()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(bytes).ok())
        {
            for event in ["SessionStart", "UserPromptSubmit", "Stop"] {
                if let Some(groups) = value["hooks"][event].as_array() {
                    for group in groups {
                        if let Some(handlers) = group["hooks"].as_array() {
                            for handler in handlers {
                                if let Some(command) = handler["command"]
                                    .as_str()
                                    .filter(|command| command.contains("vrg-hook-observer-v1.py"))
                                {
                                    hook_review.push((event, command.to_owned()));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    let mut options = crate::setup::SetupOptions {
        project,
        roots,
        config_path,
        data_dir: data_dir.clone(),
        model_dir,
        memory_dir,
        synthetic_root: args.synthetic,
    };
    let mut plan = crate::setup::plan_setup(&options, extra.clone())?;
    // Capture both plans before a potentially long native build, so a later
    // fallback cannot silently accept external changes made during that build.
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    let text_plan = if options.model_dir.is_some() {
        let mut text_options = options.clone();
        text_options.model_dir = None;
        Some(crate::setup::plan_setup(&text_options, extra.clone()))
    } else {
        None
    };
    println!(
        "VRG setup plan: {} file(s); no indexing; offline={}",
        plan.changes().len(),
        args.offline
    );
    if let Some(path) = &download {
        if path.exists() {
            println!(
                "Verify and reuse pinned model at {} ({})",
                path.display(),
                crate::model_setup::MODEL_REVISION
            );
        } else {
            println!(
                "Pinned model download: {} ({}; approximately 488 MB); requires curl and free disk space",
                path.display(),
                crate::model_setup::MODEL_REVISION
            );
        }
    } else {
        println!("No downloads planned.");
    }
    for root in &options.roots {
        println!("  Authorized source: {}", root.display());
    }
    println!("  Derived data: {}", options.data_dir.display());
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    if options.model_dir.is_some() {
        println!("  Intel runtime: reuse the packaged or verified profile runtime; if unavailable, building official ONNX Runtime 1.24.1 requires separate consent, network, Git, CMake 3.28+, Python 3.10+ and the Xcode command-line tools.");
        if args.build_onnx_runtime {
            println!(
                "  Runtime source build is authorized; cache: {}",
                data_dir.join("resources/onnxruntime").display()
            );
        }
    }
    if let Some(memory) = &options.memory_dir {
        println!("  Authorized operational memory: {}", memory.display());
    }
    for path in plan.changes() {
        println!("  File: {}", path.display());
    }
    println!(
        "Authorized source roots are local. MCP results may be shared with the configured Codex model when used; setup does not authorize transfer of unrelated material."
    );
    if args.codex {
        println!("If Codex requests review, open /hooks; setup never grants trust.");
        if !hook_review.is_empty() {
            println!("Project hook source: {}", hook_path.display());
            for (event, command) in &hook_review {
                println!("  {event}: {command}");
            }
        }
    }
    if args.dry_run {
        return Ok(());
    }
    if !args.yes {
        if !interactive || !ask("Apply this plan? [y/N] ")?.eq_ignore_ascii_case("y") {
            return Err(VrgError::InvalidConfig("plan was not confirmed".into()));
        }
    }
    let prepare_semantic = (|| -> VrgResult<()> {
        #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
        if options.model_dir.is_some()
            && crate::semantic::preflight_macos_intel_runtime(Some(&data_dir)).is_err()
        {
            let consent = args.build_onnx_runtime || (!args.offline && interactive
                && ask("Build ONNX Runtime 1.24.1 from pinned official sources? This downloads source and dependencies and requires developer tools. [y/N] ")?.eq_ignore_ascii_case("y"));
            if !consent {
                return Err(VrgError::SemanticUnavailable("ONNX Runtime unavailable; use --build-onnx-runtime to consent to an official source build".into()));
            }
            if args.offline {
                return Err(VrgError::SemanticUnavailable(
                    "offline mode prohibits downloading ONNX Runtime sources".into(),
                ));
            }
            let cache = data_dir.join("resources/onnxruntime");
            crate::setup::private_dirs(&cache)?;
            crate::onnx_source_setup::prepare(
                &cache,
                consent,
                args.offline,
                &crate::runtime_probe::probe_in_child,
            )?;
            crate::semantic::preflight_macos_intel_runtime(Some(&data_dir))?;
        }
        if let Some(path) = &download {
            let parent = path
                .parent()
                .ok_or_else(|| VrgError::InvalidConfig("model has no parent".into()))?;
            crate::setup::private_dirs(parent)?;
            crate::model_setup::download_model(path, true, args.offline)?;
        }
        if let Some(path) = &options.model_dir {
            let model = crate::config::SemanticModelConfig {
                model_id: "intfloat/multilingual-e5-small".into(),
                model_dir: path.clone(),
                threads: 4,
                max_length: 512,
            };
            let embedder =
                crate::semantic::LocalEmbedder::load_with_data_dir(&model, Some(&data_dir))?;
            // Actual local inference, not merely library discovery or ABI inspection.
            embedder.embed_query("synthetic runtime verification")?;
        }
        Ok(())
    })();
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    if let Err(error) = prepare_semantic {
        eprintln!("Intel semantic search remains pending: {error}. Attempting the preplanned text-only fallback.");
        options.model_dir = None;
        plan = text_plan.ok_or_else(|| {
            VrgError::InvalidConfig("text-only fallback was not planned".into())
        })??;
        println!(
            "Revised plan: text-only; {} file(s), no additional source access.",
            plan.changes().len()
        );
    }
    #[cfg(not(all(target_os = "macos", target_arch = "x86_64")))]
    prepare_semantic?;
    let result = plan.apply(
        &data_dir.join("setup-backups"),
        &options.project,
        &options.config_path,
    )?;
    println!(
        "Applied {} changes; private backup: {}",
        result.changed,
        result
            .backup
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "not needed".into())
    );
    Ok(())
}

fn run_doctor(config: Option<PathBuf>, args: DoctorArgs) -> VrgResult<()> {
    if args.dry_run && args.check_runtime {
        return Err(VrgError::InvalidConfig(
            "--fix --dry-run cannot run the active runtime probe; omit --check-runtime".into(),
        ));
    }
    if args.fix {
        crate::setup::ensure_supported_platform()?;
        let project = env::current_dir()?.canonicalize()?;
        let plan = crate::doctor::repair_plan(&project, config.as_deref())?;
        eprintln!("Recognized repair plan: {} file(s)", plan.changes().len());
        for path in plan.changes() {
            eprintln!("  {}", path.display());
        }
        if !args.dry_run && !plan.changes().is_empty() {
            let path = crate::doctor::resolve_config_path(config.as_deref())
                .ok_or_else(|| VrgError::InvalidConfig("configuration path unavailable".into()))?;
            let data_dir = VrgConfig::load(&path)?.data_dir;
            let applied = plan.apply(&data_dir.join("setup-backups"), &project, &path)?;
            eprintln!(
                "Repaired {} file(s); backup: {}",
                applied.changed,
                applied
                    .backup
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "not needed".into())
            );
        }
    }
    let report = crate::doctor::inspect(config.as_deref(), args.check_runtime);
    if args.json {
        print_json(&report)?;
    } else {
        report.print_human();
    }
    let code = report.exit_code();
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}
