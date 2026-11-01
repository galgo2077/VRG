use std::{
    collections::BTreeMap,
    env, fs,
    io::Write,
    path::{Path, PathBuf},
};

use clap::{Args, Parser, Subcommand};
use serde::Serialize;

use crate::{
    VrgEngine,
    config::VrgConfig,
    contracts::{RuntimeIdentity, RuntimeRootIdentity, SearchRequest, SourceRef},
    error::{VrgError, VrgResult},
    mcp_server,
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
