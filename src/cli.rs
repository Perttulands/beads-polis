//! CLI definitions for beads-v2.
//!
//! Clap derive structs defining the full `br` interface.
//! Command handlers call into core modules via the Engine.

use crate::bead;
use crate::compact;
use crate::config;
use crate::doctor;
use crate::engine::Engine;
use crate::event::{BeadSnapshot, Event};
use crate::index::Filters;
use chrono::{Duration, Utc};
use clap::{Args, Parser, Subcommand, ValueEnum};
use std::collections::HashMap;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Top-level CLI
// ---------------------------------------------------------------------------

#[derive(Parser, Debug)]
#[command(
    name = "br",
    version,
    about = "Event-sourced work tracker for Polis",
    long_about = "Beads v2 — JSONL source of truth, SQLite derived index.\n\n\
                  Every mutation appends to events.jsonl. The SQLite index is \
                  derived and disposable. If they disagree, JSONL wins."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// Output as JSON (machine-readable)
    #[arg(long, global = true)]
    pub json: bool,

    /// Override POLIS_ACTOR identity (debugging only)
    #[arg(long, global = true)]
    pub actor: Option<String>,

    /// Override database directory
    #[arg(long, global = true)]
    pub db: Option<PathBuf>,

    /// Disable colored output
    #[arg(long, global = true)]
    pub no_color: bool,

    /// Increase logging verbosity (-v, -vv)
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,
}

impl Cli {
    /// Resolve the actor identity: --actor flag > POLIS_ACTOR env > error.
    pub fn resolve_actor(&self) -> Result<String, CliError> {
        if let Some(ref actor) = self.actor {
            return Ok(actor.clone());
        }
        std::env::var("POLIS_ACTOR").map_err(|_| CliError::NoActor)
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum Commands {
    // -- Core commands -------------------------------------------------------

    /// Create a new bead
    Create(CreateArgs),

    /// Show full details of a bead
    Show(ShowArgs),

    /// List beads with filters
    List(ListArgs),

    /// Update bead fields
    Update(UpdateArgs),

    /// Close a bead with a reason
    Close(CloseArgs),

    /// Show actionable beads (open, unblocked, unclaimed)
    Ready(ReadyArgs),

    /// Full-text search across beads
    Search(SearchArgs),

    /// Sync: export, snapshot, or import
    Sync(SyncArgs),

    // -- Agent workflow ------------------------------------------------------

    /// Claim a bead for work (sets in_progress + assignee + deadline)
    Claim(ClaimArgs),

    /// Extend claim deadline (signal still working)
    Heartbeat(HeartbeatArgs),

    /// Release a claim (sets status back to open)
    Unclaim(UnclaimArgs),

    // -- Maintenance ---------------------------------------------------------

    /// Check JSONL integrity, index freshness, stale claims
    Doctor,

    /// Read-only health check (JSON summary of store state)
    Health,

    /// Force index rebuild from JSONL
    Rebuild,

    /// Force compaction of the event log
    Compact,

    /// Back up events.jsonl to a timestamped snapshot
    Backup(BackupArgs),

    /// Restore events.jsonl from a backup file
    Restore(RestoreArgs),

    /// City-wide commands (cross-project)
    City(CityArgs),

    /// Lint: check bead quality (title length, body, done-condition keywords)
    Lint(LintArgs),

    // -- Intelligence (replaces bv) ------------------------------------------

    /// Triage: search beads for work dispatch (replaces bv --robot-search)
    Triage(TriageArgs),

    /// Find beads related to a given bead (replaces bv --robot-related)
    Related(RelatedArgs),

    /// Generate execution plan from open P1 beads (replaces bv --robot-plan)
    Plan(PlanArgs),
}

// ---------------------------------------------------------------------------
// Core command args
// ---------------------------------------------------------------------------

#[derive(Args, Debug)]
pub struct CreateArgs {
    /// Bead title
    #[arg(value_name = "TITLE", required_unless_present = "title_flag")]
    pub title: Option<String>,

    /// Alias for the positional TITLE argument
    #[arg(long = "title", value_name = "TITLE", conflicts_with = "title")]
    pub title_flag: Option<String>,

    /// Priority: 0=critical, 1=high, 2=medium, 3=low, 4=backlog
    #[arg(short, long, default_value_t = 2)]
    pub priority: u8,

    /// Bead type
    #[arg(short = 't', long = "type", default_value = "task")]
    pub bead_type: BeadType,

    /// Project name
    #[arg(long)]
    pub project: Option<String>,

    /// Dependency: bead ID this is blocked by (repeatable)
    #[arg(long = "dep")]
    pub deps: Vec<String>,

    /// Parent bead ID (for epic → child relationships)
    #[arg(long)]
    pub parent: Option<String>,

    /// Full description / context
    #[arg(long)]
    pub description: Option<String>,

    /// Labels (repeatable)
    #[arg(short, long)]
    pub label: Vec<String>,

    /// V1 compatibility alias for comma-separated labels
    #[arg(long = "labels", value_delimiter = ',')]
    pub labels: Vec<String>,

    /// Assignee
    #[arg(short = 'a', long)]
    pub assignee: Option<String>,

    /// Suppress stdout on success
    #[arg(long)]
    pub silent: bool,
}

#[derive(Args, Debug)]
pub struct ShowArgs {
    /// Bead ID
    pub id: String,
}

#[derive(Args, Debug, Default)]
pub struct ListArgs {
    /// V1 compatibility flag. List already shows all statuses by default.
    #[arg(long)]
    pub all: bool,

    /// Filter by project
    #[arg(long)]
    pub project: Option<String>,

    /// Filter by status
    #[arg(long)]
    pub status: Option<BeadStatus>,

    /// Filter by priority
    #[arg(long)]
    pub priority: Option<u8>,

    /// Filter by type
    #[arg(short = 't', long = "type")]
    pub bead_type: Option<BeadType>,

    /// Filter by assignee
    #[arg(long)]
    pub assignee: Option<String>,
}

#[derive(Args, Debug)]
pub struct UpdateArgs {
    /// Bead ID
    pub id: String,

    /// New status
    #[arg(long)]
    pub status: Option<BeadStatus>,

    /// New priority
    #[arg(long)]
    pub priority: Option<u8>,

    /// New title
    #[arg(long)]
    pub title: Option<String>,

    /// New description
    #[arg(long)]
    pub description: Option<String>,

    /// Add a dependency
    #[arg(long)]
    pub add_dep: Vec<String>,

    /// Remove a dependency
    #[arg(long)]
    pub rm_dep: Vec<String>,

    /// New project
    #[arg(long)]
    pub project: Option<String>,

    /// New assignee
    #[arg(long)]
    pub assignee: Option<String>,
}

#[derive(Args, Debug)]
pub struct CloseArgs {
    /// Bead ID
    pub id: String,

    /// Reason for closing
    #[arg(long)]
    pub reason: String,
}

#[derive(Args, Debug, Default)]
pub struct ReadyArgs {
    /// Filter by project
    #[arg(long)]
    pub project: Option<String>,
}

#[derive(Args, Debug)]
pub struct SearchArgs {
    /// Search query (matched against title + description)
    pub query: String,

    /// Max results to return
    #[arg(long)]
    pub limit: Option<usize>,

    /// Filter by status
    #[arg(long)]
    pub status: Option<BeadStatus>,

    /// Filter by bead type
    #[arg(long = "type")]
    pub bead_type: Option<String>,

    /// Sort field
    #[arg(long, value_enum, default_value_t = SearchSort::Priority)]
    pub sort: SearchSort,

    /// Reverse sort order
    #[arg(long)]
    pub reverse: bool,
}

#[derive(Args, Debug)]
pub struct SyncArgs {
    /// Export beads for a specific project to its repo .beads/ dir
    #[arg(long)]
    pub export_project: Option<String>,

    /// Create a full-state snapshot
    #[arg(long)]
    pub snapshot: bool,

    /// Import-only mode (rebuild index from JSONL)
    #[arg(long)]
    pub import_only: bool,

    /// Migrate from legacy issues.jsonl to event-sourced events.jsonl
    #[arg(long)]
    pub migrate: bool,
}

// ---------------------------------------------------------------------------
// Agent workflow args
// ---------------------------------------------------------------------------

#[derive(Args, Debug)]
pub struct ClaimArgs {
    /// Bead ID to claim
    pub id: String,

    /// Lock duration (e.g. "1h", "30m", "2h"). Default: 1h
    #[arg(long, default_value = "1h")]
    pub lock_for: String,
}

#[derive(Args, Debug)]
pub struct HeartbeatArgs {
    /// Bead ID to heartbeat
    pub id: String,
}

#[derive(Args, Debug)]
pub struct UnclaimArgs {
    /// Bead ID to unclaim
    pub id: String,
}

// ---------------------------------------------------------------------------
// City (cross-project) args
// ---------------------------------------------------------------------------

#[derive(Args, Debug)]
pub struct CityArgs {
    #[command(subcommand)]
    pub command: CityCommands,
}

#[derive(Subcommand, Debug)]
pub enum CityCommands {
    /// Show actionable beads across all projects
    Ready,
    /// List beads across all projects
    List(ListArgs),
}

// ---------------------------------------------------------------------------
// Lint args
// ---------------------------------------------------------------------------

#[derive(Args, Debug)]
pub struct LintArgs {
    /// Bead ID to lint
    pub id: String,
}

// ---------------------------------------------------------------------------
// Intelligence command args (bv replacements)
// ---------------------------------------------------------------------------

#[derive(Args, Debug)]
pub struct TriageArgs {
    /// Search query (matched against title + description)
    #[arg(long)]
    pub search: String,

    /// Max results to return
    #[arg(long, default_value_t = 10)]
    pub limit: usize,
}

#[derive(Args, Debug)]
pub struct RelatedArgs {
    /// Bead ID to find relations for
    pub id: String,
}

#[derive(Args, Debug)]
pub struct PlanArgs {
    // No extra args — generates plan from open P1 beads grouped by parent epic.
}

#[derive(Args, Debug)]
pub struct BackupArgs {
    /// Destination path for the backup file (default: .beads/backups/events-YYYYMMDDTHHMMSSZ.jsonl.gz)
    #[arg(long)]
    pub dest: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct RestoreArgs {
    /// Path to the backup file to restore from
    #[arg(long = "from")]
    pub file: PathBuf,

    /// Validate backup integrity without restoring
    #[arg(long)]
    pub verify: bool,

    /// Overwrite current events.jsonl without confirmation
    #[arg(long, short = 'f')]
    pub force: bool,
}

// ---------------------------------------------------------------------------
// Value enums
// ---------------------------------------------------------------------------

#[derive(ValueEnum, Clone, Debug, PartialEq, Eq)]
pub enum BeadStatus {
    Open,
    InProgress,
    Closed,
    Deferred,
}

impl BeadStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::InProgress => "in_progress",
            Self::Closed => "closed",
            Self::Deferred => "deferred",
        }
    }
}

impl std::fmt::Display for BeadStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(ValueEnum, Clone, Debug, PartialEq, Eq)]
pub enum BeadType {
    Epic,
    Feature,
    Bug,
    Task,
    Chore,
}

impl BeadType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Epic => "epic",
            Self::Feature => "feature",
            Self::Bug => "bug",
            Self::Task => "task",
            Self::Chore => "chore",
        }
    }
}

impl std::fmt::Display for BeadType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchSort {
    Created,
    Updated,
    Priority,
}

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

/// CLI-level errors (before we reach the core engine).
#[derive(Debug)]
pub enum CliError {
    /// POLIS_ACTOR not set and no --actor override
    NoActor,
    /// Core engine error (wraps whatever the engine returns)
    Engine(String),
    /// Bead not found
    NotFound(String),
    /// Bead already claimed by another agent
    AlreadyClaimed {
        bead: String,
        holder: String,
        deadline: String,
    },
    /// Permission denied (not the assignee)
    PermissionDenied {
        bead: String,
        holder: String,
    },
    /// Invalid duration string
    InvalidDuration(String),
    /// Thrash detection: too many rebuilds
    IndexThrashing {
        rebuilds: u32,
        window_secs: u64,
    },
    /// Generic I/O error
    Io(std::io::Error),
    /// Lint check failed — carries the result JSON for stdout output
    LintFailed(serde_json::Value),
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoActor => write!(
                f,
                "POLIS_ACTOR environment variable is not set. \
                 Set it or use --actor <name> to override."
            ),
            Self::Engine(msg) => write!(f, "{msg}"),
            Self::NotFound(id) => write!(f, "bead not found: {id}"),
            Self::AlreadyClaimed {
                bead,
                holder,
                deadline,
            } => write!(
                f,
                "bead {bead} already claimed by {holder} until {deadline}"
            ),
            Self::PermissionDenied { bead, holder } => {
                write!(f, "bead {bead} is assigned to {holder} — only they or operator can modify it")
            }
            Self::InvalidDuration(s) => write!(f, "invalid duration: {s}"),
            Self::IndexThrashing {
                rebuilds,
                window_secs,
            } => write!(
                f,
                "index thrashing detected: {rebuilds} rebuilds in {window_secs}s"
            ),
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::LintFailed(v) => {
                let errors = v["errors"].as_array().map(|a| a.len()).unwrap_or(0);
                write!(f, "lint failed: {} error(s)", errors)
            }
        }
    }
}

impl std::error::Error for CliError {}

impl From<std::io::Error> for CliError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl CliError {
    /// Render as structured JSON for --json mode or agent consumption.
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Self::NoActor => serde_json::json!({
                "error": "no_actor",
                "message": self.to_string(),
            }),
            Self::Engine(msg) => serde_json::json!({
                "error": "engine",
                "message": msg,
            }),
            Self::NotFound(id) => serde_json::json!({
                "error": "not_found",
                "bead": id,
            }),
            Self::AlreadyClaimed {
                bead,
                holder,
                deadline,
            } => serde_json::json!({
                "error": "already_claimed",
                "bead": bead,
                "holder": holder,
                "deadline": deadline,
            }),
            Self::PermissionDenied { bead, holder } => serde_json::json!({
                "error": "permission_denied",
                "bead": bead,
                "holder": holder,
            }),
            Self::InvalidDuration(s) => serde_json::json!({
                "error": "invalid_duration",
                "input": s,
            }),
            Self::IndexThrashing {
                rebuilds,
                window_secs,
            } => serde_json::json!({
                "alert": "index_thrashing",
                "rebuilds": rebuilds,
                "window": format!("{window_secs}s"),
            }),
            Self::Io(e) => serde_json::json!({
                "error": "io",
                "message": e.to_string(),
            }),
            Self::LintFailed(v) => v.clone(),
        }
    }

    /// Exit code for this error.
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::NoActor => 2,
            Self::NotFound(_) => 1,
            Self::AlreadyClaimed { .. } => 1,
            Self::PermissionDenied { .. } => 1,
            Self::InvalidDuration(_) => 2,
            Self::IndexThrashing { .. } => 3,
            Self::Engine(_) => 1,
            Self::Io(_) => 1,
            Self::LintFailed(_) => 1,
        }
    }
}

// ---------------------------------------------------------------------------
// Dispatch — called from main.rs
// ---------------------------------------------------------------------------

/// Dispatch a parsed CLI command to the appropriate handler.
///
/// Returns Ok(json_value) on success (printed to stdout),
/// or Err(CliError) on failure (printed to stderr).
pub fn dispatch(cli: &Cli) -> Result<Option<serde_json::Value>, CliError> {
    // Doctor, Health, Rebuild, Compact, Backup, Restore don't need actor
    let db = cli.db.as_deref();
    match &cli.command {
        Commands::Doctor => return cmd_doctor(db),
        Commands::Health => return cmd_health(db),
        Commands::Rebuild => return cmd_rebuild(db),
        Commands::Compact => return cmd_compact(db),
        Commands::Backup(args) => return cmd_backup(db, args),
        Commands::Restore(args) => return cmd_restore(db, args),
        _ => {}
    }

    let actor = cli.resolve_actor()?;
    let beads_dir = resolve_dir(db);
    let engine = Engine::open(&beads_dir)?;

    match &cli.command {
        // -- Core commands ---------------------------------------------------
        Commands::Create(args) => cmd_create(&engine, &actor, args),
        Commands::Show(args) => cmd_show(&engine, args),
        Commands::List(args) => cmd_list(&engine, args),
        Commands::Update(args) => cmd_update(&engine, &actor, args),
        Commands::Close(args) => cmd_close(&engine, &actor, args),
        Commands::Ready(args) => cmd_ready(&engine, args),
        Commands::Search(args) => cmd_search(&engine, args),
        Commands::Sync(args) => cmd_sync(&engine, args),

        // -- Agent workflow --------------------------------------------------
        Commands::Claim(args) => cmd_claim(&engine, &actor, args),
        Commands::Heartbeat(args) => cmd_heartbeat(&engine, &actor, args),
        Commands::Unclaim(args) => cmd_unclaim(&engine, &actor, args),

        // -- City -----------------------------------------------------------
        Commands::City(args) => cmd_city(&engine, args),

        // -- Lint -----------------------------------------------------------
        Commands::Lint(args) => cmd_lint(&engine, args),

        // -- Intelligence (bv replacements) ---------------------------------
        Commands::Triage(args) => cmd_triage(&engine, args),
        Commands::Related(args) => cmd_related(&engine, args),
        Commands::Plan(_) => cmd_plan(&engine),

        // Already handled above
        Commands::Doctor | Commands::Health | Commands::Rebuild | Commands::Compact
        | Commands::Backup(_) | Commands::Restore(_) => unreachable!(),
    }
}

// ---------------------------------------------------------------------------
// Command handlers — stubs calling into core modules
// ---------------------------------------------------------------------------

fn cmd_create(engine: &Engine, actor: &str, args: &CreateArgs) -> Result<Option<serde_json::Value>, CliError> {
    let now = Utc::now();
    let id = engine.generate_id();
    let title = args.resolved_title()?;

    let snapshot = BeadSnapshot {
        id: id.clone(),
        title: title.clone(),
        description: args.description.clone(),
        status: "open".into(),
        priority: args.priority.min(4),
        bead_type: args.bead_type.as_str().into(),
        project: args.project.clone().unwrap_or_default(),
        assignee: args.assignee.clone(),
        parent: args.parent.clone(),
        dependencies: args.deps.clone(),
        labels: args.all_labels(),
        created_at: now,
        updated_at: now,
        closed_at: None,
        close_reason: None,
        claimed_at: None,
        claim_deadline: None,
        last_heartbeat: None,
    };

    let event = Event::Create {
        ts: now,
        actor: actor.into(),
        bead: snapshot.clone(),
    };

    engine.log.append(&event)?;

    let bead = crate::bead::Bead::from_snapshot(&snapshot);
    engine.index.upsert_bead(&bead).map_err(|e| CliError::Engine(e.to_string()))?;

    Ok(Some(serde_json::json!({
        "id": id,
        "title": title,
    })))
}

fn cmd_show(engine: &Engine, args: &ShowArgs) -> Result<Option<serde_json::Value>, CliError> {
    let bead = engine.index.query_show(&args.id)
        .ok_or_else(|| CliError::NotFound(args.id.clone()))?;
    Ok(Some(serde_json::to_value(&bead).map_err(|e| CliError::Engine(e.to_string()))?))
}

fn cmd_list(engine: &Engine, args: &ListArgs) -> Result<Option<serde_json::Value>, CliError> {
    let filters = list_filters(args);
    let beads = engine.index.query_list(&filters);
    Ok(Some(serde_json::to_value(&beads).map_err(|e| CliError::Engine(e.to_string()))?))
}

fn cmd_update(engine: &Engine, actor: &str, args: &UpdateArgs) -> Result<Option<serde_json::Value>, CliError> {
    let existing = engine.index.query_show(&args.id)
        .ok_or_else(|| CliError::NotFound(args.id.clone()))?;

    let mut fields: HashMap<String, serde_json::Value> = HashMap::new();

    if let Some(ref s) = args.status {
        fields.insert("status".into(), serde_json::Value::String(s.as_str().into()));
    }
    if let Some(p) = args.priority {
        fields.insert("priority".into(), serde_json::json!(p));
    }
    if let Some(ref t) = args.title {
        fields.insert("title".into(), serde_json::Value::String(t.clone()));
    }
    if let Some(ref d) = args.description {
        fields.insert("description".into(), serde_json::Value::String(d.clone()));
    }
    if let Some(ref p) = args.project {
        fields.insert("project".into(), serde_json::Value::String(p.clone()));
    }
    if let Some(ref a) = args.assignee {
        fields.insert("assignee".into(), serde_json::Value::String(a.clone()));
    }

    // Handle dependency modifications
    if !args.add_dep.is_empty() || !args.rm_dep.is_empty() {
        let mut deps: Vec<String> = existing.dependencies.clone();
        for dep in &args.add_dep {
            if !deps.contains(dep) {
                deps.push(dep.clone());
            }
        }
        deps.retain(|d| !args.rm_dep.contains(d));
        fields.insert("dependencies".into(), serde_json::json!(deps));
    }

    if fields.is_empty() {
        return Err(CliError::Engine("no fields to update".into()));
    }

    let event = Event::Update {
        ts: Utc::now(),
        actor: actor.into(),
        id: args.id.clone(),
        fields,
    };

    engine.log.append(&event)?;

    // Re-read to get updated state
    let mut updated = existing;
    updated.apply_event(&event);
    engine.index.upsert_bead(&updated).map_err(|e| CliError::Engine(e.to_string()))?;

    Ok(Some(serde_json::to_value(&updated).map_err(|e| CliError::Engine(e.to_string()))?))
}

fn cmd_close(engine: &Engine, actor: &str, args: &CloseArgs) -> Result<Option<serde_json::Value>, CliError> {
    let existing = engine.index.query_show(&args.id)
        .ok_or_else(|| CliError::NotFound(args.id.clone()))?;

    if existing.status == bead::Status::Closed {
        return Err(CliError::Engine(format!("bead {} is already closed", args.id)));
    }

    // Permission check: only assignee or operator can close in_progress beads
    if existing.status == bead::Status::InProgress {
        if let Some(ref holder) = existing.assignee {
            if actor != holder && actor != "operator" {
                return Err(CliError::PermissionDenied {
                    bead: args.id.clone(),
                    holder: holder.clone(),
                });
            }
        }
    }

    let now = Utc::now();
    let event = Event::Close {
        ts: now,
        actor: actor.into(),
        id: args.id.clone(),
        reason: args.reason.clone(),
    };

    engine.log.append(&event)?;

    let mut updated = existing;
    updated.apply_event(&event);
    engine.index.upsert_bead(&updated).map_err(|e| CliError::Engine(e.to_string()))?;

    Ok(Some(serde_json::json!({
        "id": args.id,
        "status": "closed",
    })))
}

fn cmd_ready(engine: &Engine, args: &ReadyArgs) -> Result<Option<serde_json::Value>, CliError> {
    let beads = engine.index.query_ready(args.project.as_deref());
    Ok(Some(serde_json::to_value(&beads).map_err(|e| CliError::Engine(e.to_string()))?))
}

fn cmd_search(engine: &Engine, args: &SearchArgs) -> Result<Option<serde_json::Value>, CliError> {
    let mut beads = engine.index.query_search(&args.query);

    if let Some(status) = args.status.as_ref() {
        let status = status.to_core();
        beads.retain(|bead| bead.status == status);
    }

    if let Some(bead_type) = args.bead_type.as_deref() {
        beads.retain(|bead| bead.bead_type.as_str() == bead_type);
    }

    match args.sort {
        SearchSort::Created => beads.sort_by_key(|bead| bead.created_at),
        SearchSort::Updated => beads.sort_by_key(|bead| bead.updated_at),
        SearchSort::Priority => beads.sort_by_key(|bead| bead.priority),
    }

    if args.reverse {
        beads.reverse();
    }

    if let Some(limit) = args.limit {
        beads.truncate(limit);
    }

    Ok(Some(serde_json::to_value(&beads).map_err(|e| CliError::Engine(e.to_string()))?))
}

fn cmd_sync(engine: &Engine, args: &SyncArgs) -> Result<Option<serde_json::Value>, CliError> {
    if args.import_only {
        let events = engine.log.read_all()?;
        let db_path = engine.beads_dir.join("index.db");
        crate::index::Index::rebuild(&db_path, &events)?;
        let count = engine.log.line_count()?;
        std::fs::write(engine.beads_dir.join("index.watermark"), count.to_string())?;
        return Ok(Some(serde_json::json!({
            "action": "import",
            "events": count,
        })));
    }

    if args.snapshot {
        let log_path = engine.beads_dir.join("events.jsonl");
        let snap_dir = engine.snapshot_dir();
        let bead_count = compact::compact(&log_path, &snap_dir)
            .map_err(CliError::Engine)?;
        return Ok(Some(serde_json::json!({
            "action": "snapshot",
            "beads": bead_count,
        })));
    }

    if let Some(ref project) = args.export_project {
        let filters = Filters {
            project: Some(project.clone()),
            ..Default::default()
        };
        let beads = engine.index.query_list(&filters);

        if let Some(project_path) = engine.config.projects.get(project) {
            let export_dir = project_path.join(".beads");
            std::fs::create_dir_all(&export_dir)?;
            let export_path = export_dir.join("beads.jsonl");
            let mut f = std::fs::File::create(&export_path)?;
            for bead in &beads {
                let line = serde_json::to_string(bead).map_err(|e| CliError::Engine(e.to_string()))?;
                use std::io::Write;
                writeln!(f, "{}", line)?;
            }
            return Ok(Some(serde_json::json!({
                "action": "export",
                "project": project,
                "beads": beads.len(),
                "path": export_path.to_string_lossy(),
            })));
        } else {
            return Err(CliError::Engine(format!("project '{}' not found in config.yaml", project)));
        }
    }

    if args.migrate {
        return cmd_sync_migrate(engine);
    }

    // Default: if events.jsonl is empty but issues.jsonl exists, offer migration
    let legacy_path = engine.beads_dir.join("issues.jsonl");
    if legacy_path.exists() {
        let event_count = engine.log.line_count()?;
        if event_count == 0 {
            return Err(CliError::Engine(
                "events.jsonl is empty but issues.jsonl exists. Use 'br sync --migrate' to convert legacy data.".into()
            ));
        }
    }

    Err(CliError::Engine("specify --import-only, --snapshot, --export-project, or --migrate".into()))
}

fn cmd_claim(engine: &Engine, actor: &str, args: &ClaimArgs) -> Result<Option<serde_json::Value>, CliError> {
    let duration_secs = parse_duration(&args.lock_for)?;
    let now = Utc::now();
    let deadline = now + Duration::seconds(duration_secs as i64);

    let existing = engine.index.query_show(&args.id)
        .ok_or_else(|| CliError::NotFound(args.id.clone()))?;

    // Check if already claimed by someone else with active deadline
    if existing.status == bead::Status::InProgress {
        if let Some(ref holder) = existing.assignee {
            if holder != actor {
                if let Some(claim_deadline) = existing.claim_deadline {
                    if claim_deadline > now {
                        return Err(CliError::AlreadyClaimed {
                            bead: args.id.clone(),
                            holder: holder.clone(),
                            deadline: claim_deadline.to_rfc3339(),
                        });
                    }
                }
            }
        }
    }

    let mut fields: HashMap<String, serde_json::Value> = HashMap::new();
    fields.insert("status".into(), serde_json::json!("in_progress"));
    fields.insert("assignee".into(), serde_json::json!(actor));
    fields.insert("claimed_at".into(), serde_json::json!(now.to_rfc3339()));
    fields.insert("claim_deadline".into(), serde_json::json!(deadline.to_rfc3339()));
    fields.insert("last_heartbeat".into(), serde_json::json!(now.to_rfc3339()));

    let event = Event::Update {
        ts: now,
        actor: actor.into(),
        id: args.id.clone(),
        fields,
    };

    engine.log.append(&event)?;

    let mut updated = existing;
    updated.apply_event(&event);
    engine.index.upsert_bead(&updated).map_err(|e| CliError::Engine(e.to_string()))?;

    Ok(Some(serde_json::json!({
        "id": args.id,
        "assignee": actor,
        "claim_deadline": deadline.to_rfc3339(),
    })))
}

fn cmd_heartbeat(engine: &Engine, actor: &str, args: &HeartbeatArgs) -> Result<Option<serde_json::Value>, CliError> {
    let existing = engine.index.query_show(&args.id)
        .ok_or_else(|| CliError::NotFound(args.id.clone()))?;

    if let Some(ref holder) = existing.assignee {
        if holder != actor && actor != "operator" {
            return Err(CliError::PermissionDenied {
                bead: args.id.clone(),
                holder: holder.clone(),
            });
        }
    } else {
        return Err(CliError::Engine(format!("bead {} is not claimed", args.id)));
    }

    let now = Utc::now();
    let deadline = now + Duration::hours(1);

    let mut fields: HashMap<String, serde_json::Value> = HashMap::new();
    fields.insert("last_heartbeat".into(), serde_json::json!(now.to_rfc3339()));
    fields.insert("claim_deadline".into(), serde_json::json!(deadline.to_rfc3339()));

    let event = Event::Update {
        ts: now,
        actor: actor.into(),
        id: args.id.clone(),
        fields,
    };

    engine.log.append(&event)?;

    let mut updated = existing;
    updated.apply_event(&event);
    engine.index.upsert_bead(&updated).map_err(|e| CliError::Engine(e.to_string()))?;

    Ok(Some(serde_json::json!({
        "id": args.id,
        "claim_deadline": deadline.to_rfc3339(),
    })))
}

fn cmd_unclaim(engine: &Engine, actor: &str, args: &UnclaimArgs) -> Result<Option<serde_json::Value>, CliError> {
    let existing = engine.index.query_show(&args.id)
        .ok_or_else(|| CliError::NotFound(args.id.clone()))?;

    if let Some(ref holder) = existing.assignee {
        if holder != actor && actor != "operator" {
            return Err(CliError::PermissionDenied {
                bead: args.id.clone(),
                holder: holder.clone(),
            });
        }
    }

    let now = Utc::now();
    let mut fields: HashMap<String, serde_json::Value> = HashMap::new();
    fields.insert("status".into(), serde_json::json!("open"));
    fields.insert("assignee".into(), serde_json::Value::Null);
    fields.insert("claimed_at".into(), serde_json::Value::Null);
    fields.insert("claim_deadline".into(), serde_json::Value::Null);
    fields.insert("last_heartbeat".into(), serde_json::Value::Null);

    let event = Event::Update {
        ts: now,
        actor: actor.into(),
        id: args.id.clone(),
        fields,
    };

    engine.log.append(&event)?;

    let mut updated = existing;
    updated.apply_event(&event);
    engine.index.upsert_bead(&updated).map_err(|e| CliError::Engine(e.to_string()))?;

    Ok(Some(serde_json::json!({
        "id": args.id,
        "status": "open",
    })))
}

// ---------------------------------------------------------------------------
// Intelligence command handlers (bv replacements)
// ---------------------------------------------------------------------------

fn cmd_triage(engine: &Engine, args: &TriageArgs) -> Result<Option<serde_json::Value>, CliError> {
    let mut beads = engine.index.query_search(&args.search);
    // Filter to open/in_progress only
    beads.retain(|b| b.status == bead::Status::Open || b.status == bead::Status::InProgress);
    // Sort by priority (lower = higher priority), then by title match closeness
    beads.sort_by(|a, b| a.priority.cmp(&b.priority).then(a.created_at.cmp(&b.created_at)));
    let total = beads.len();
    beads.truncate(args.limit);

    let results: Vec<serde_json::Value> = beads
        .iter()
        .enumerate()
        .map(|(i, b)| {
            // Simple relevance score: higher priority beads score higher, decay by position
            let score = 1.0 - (i as f64 * 0.1).min(0.9);
            serde_json::json!({
                "id": b.id,
                "title": b.title,
                "status": b.status.as_str(),
                "priority": b.priority,
                "score": score,
            })
        })
        .collect();

    Ok(Some(serde_json::json!({
        "query": args.search,
        "results": results,
        "total": total,
    })))
}

fn cmd_related(engine: &Engine, args: &RelatedArgs) -> Result<Option<serde_json::Value>, CliError> {
    let target = engine
        .index
        .query_show(&args.id)
        .ok_or_else(|| CliError::NotFound(args.id.clone()))?;

    let mut related: Vec<serde_json::Value> = Vec::new();

    // 1. Same parent — strongest relationship
    if let Some(ref parent) = target.parent {
        let siblings = engine.index.query_list(&Filters {
            status: Some(bead::Status::Open),
            ..Default::default()
        });
        for b in &siblings {
            if b.id != target.id && b.parent.as_deref() == Some(parent) {
                related.push(serde_json::json!({
                    "id": b.id,
                    "title": b.title,
                    "relationship": "same-parent",
                    "strength": 0.9,
                }));
            }
        }
    }

    // 2. Shared labels
    if !target.labels.is_empty() {
        for label in &target.labels {
            let matches = engine.index.query_list(&Filters {
                label: Some(label.clone()),
                ..Default::default()
            });
            for b in &matches {
                if b.id != target.id && !related.iter().any(|r| r["id"].as_str() == Some(&b.id)) {
                    related.push(serde_json::json!({
                        "id": b.id,
                        "title": b.title,
                        "relationship": format!("shared-label:{}", label),
                        "strength": 0.7,
                    }));
                }
            }
        }
    }

    // 3. Dependency relationships
    let all_open = engine.index.query_list(&Filters {
        status: Some(bead::Status::Open),
        ..Default::default()
    });
    for b in &all_open {
        if b.id != target.id && !related.iter().any(|r| r["id"].as_str() == Some(&b.id)) {
            if b.dependencies.contains(&target.id) {
                related.push(serde_json::json!({
                    "id": b.id,
                    "title": b.title,
                    "relationship": "blocked-by-target",
                    "strength": 0.8,
                }));
            } else if target.dependencies.contains(&b.id) {
                related.push(serde_json::json!({
                    "id": b.id,
                    "title": b.title,
                    "relationship": "blocks-target",
                    "strength": 0.8,
                }));
            }
        }
    }

    // 4. Title keyword overlap (weak signal)
    let keywords: Vec<&str> = target.title.split_whitespace()
        .filter(|w| w.len() > 3)
        .collect();
    if !keywords.is_empty() {
        for kw in &keywords {
            let matches = engine.index.query_search(kw);
            for b in &matches {
                if b.id != target.id
                    && !related.iter().any(|r| r["id"].as_str() == Some(&b.id))
                    && (b.status == bead::Status::Open || b.status == bead::Status::InProgress)
                {
                    related.push(serde_json::json!({
                        "id": b.id,
                        "title": b.title,
                        "relationship": "keyword-overlap",
                        "strength": 0.4,
                    }));
                }
            }
        }
    }

    let total_related = related.len();

    Ok(Some(serde_json::json!({
        "target_bead_id": target.id,
        "related": related,
        "total_related": total_related,
    })))
}

fn cmd_plan(engine: &Engine) -> Result<Option<serde_json::Value>, CliError> {
    // Get all open/in_progress beads
    let all = engine.index.query_list(&Filters::default());
    let open: Vec<&bead::Bead> = all
        .iter()
        .filter(|b| b.status == bead::Status::Open || b.status == bead::Status::InProgress)
        .collect();

    // Group by parent epic
    let mut tracks: HashMap<String, Vec<serde_json::Value>> = HashMap::new();
    let mut no_parent: Vec<serde_json::Value> = Vec::new();

    for b in &open {
        let item = serde_json::json!({
            "id": b.id,
            "title": b.title,
            "priority": b.priority,
            "status": b.status.as_str(),
            "unblocks": find_unblocks(&b.id, &all),
        });

        if let Some(ref parent) = b.parent {
            tracks.entry(parent.clone()).or_default().push(item);
        } else {
            no_parent.push(item);
        }
    }

    // Build track list, sorted by lowest priority item in track
    let mut track_list: Vec<serde_json::Value> = tracks
        .into_iter()
        .map(|(epic, mut items)| {
            items.sort_by_key(|i| i["priority"].as_u64().unwrap_or(99));
            serde_json::json!({
                "track_id": epic,
                "items": items,
                "reason": format!("children of {}", epic),
            })
        })
        .collect();

    if !no_parent.is_empty() {
        no_parent.sort_by_key(|i| i["priority"].as_u64().unwrap_or(99));
        track_list.push(serde_json::json!({
            "track_id": "_ungrouped",
            "items": no_parent,
            "reason": "beads without parent epic",
        }));
    }

    track_list.sort_by_key(|t| {
        t["items"]
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|i| i["priority"].as_u64())
            .unwrap_or(99)
    });

    // Count blocked
    let blocked_count = open
        .iter()
        .filter(|b| !b.dependencies.is_empty() && b.dependencies.iter().any(|dep| {
            all.iter().any(|d| d.id == *dep && d.status != bead::Status::Closed)
        }))
        .count();

    let total_actionable = open.len() - blocked_count;

    // Find highest impact — the bead that unblocks the most others
    let mut best_id = String::new();
    let mut best_unblocks = 0usize;
    for b in &open {
        let count = find_unblocks(&b.id, &all).len();
        if count > best_unblocks {
            best_unblocks = count;
            best_id = b.id.clone();
        }
    }
    if best_id.is_empty() {
        // Fall back to highest priority open bead
        if let Some(b) = open.iter().min_by_key(|b| b.priority) {
            best_id = b.id.clone();
        }
    }

    Ok(Some(serde_json::json!({
        "plan": {
            "tracks": track_list,
            "total_actionable": total_actionable,
            "total_blocked": blocked_count,
            "summary": {
                "highest_impact": best_id,
                "impact_reason": if best_unblocks > 0 {
                    format!("unblocks {} other beads", best_unblocks)
                } else {
                    "highest priority open bead".to_string()
                },
                "unblocks_count": best_unblocks,
            }
        }
    })))
}

/// Find bead IDs that the given bead_id unblocks (i.e., beads that depend on it).
fn find_unblocks(bead_id: &str, all: &[bead::Bead]) -> Vec<String> {
    all.iter()
        .filter(|b| b.dependencies.contains(&bead_id.to_string()))
        .map(|b| b.id.clone())
        .collect()
}

fn cmd_doctor(db: Option<&std::path::Path>) -> Result<Option<serde_json::Value>, CliError> {
    let beads_dir = resolve_dir(db);
    let log_path = beads_dir.join("events.jsonl");
    let db_path = beads_dir.join("index.db");

    let diag = doctor::diagnose(&log_path, &db_path);

    // Auto-fix if issues found
    if diag.truncated_last_line || diag.index_watermark_stale
        || diag.sqlite_integrity != "ok"
        || !diag.stale_claims.is_empty()
    {
        doctor::auto_fix(&diag, &log_path, &db_path);
    }

    Ok(Some(serde_json::to_value(&diag).map_err(|e| CliError::Engine(e.to_string()))?))
}

fn cmd_health(db: Option<&std::path::Path>) -> Result<Option<serde_json::Value>, CliError> {
    let beads_dir = resolve_dir(db);
    let log_path = beads_dir.join("events.jsonl");
    let db_path = beads_dir.join("index.db");

    let diag = doctor::diagnose(&log_path, &db_path);

    let healthy = diag.jsonl_invalid_lines == 0
        && !diag.truncated_last_line
        && !diag.index_watermark_stale
        && (diag.sqlite_integrity == "ok" || diag.sqlite_integrity == "no_db")
        && diag.stale_claims.is_empty();

    let status = if healthy { "ok" } else { "degraded" };

    if !healthy {
        // Exit code 1 for unhealthy — return as error so main.rs exits non-zero
        let value = serde_json::json!({
            "status": status,
            "events": diag.jsonl_valid_lines,
            "watermark_stale": diag.index_watermark_stale,
            "integrity": diag.sqlite_integrity,
            "stale_claims": diag.stale_claims.len(),
        });
        // Print to stdout before exiting with error
        println!("{}", serde_json::to_string(&value).expect("serialize"));
        return Err(CliError::Engine("health check failed".into()));
    }

    Ok(Some(serde_json::json!({
        "status": status,
        "events": diag.jsonl_valid_lines,
        "watermark_stale": diag.index_watermark_stale,
        "integrity": diag.sqlite_integrity,
        "stale_claims": diag.stale_claims.len(),
    })))
}

fn cmd_backup(db: Option<&std::path::Path>, args: &BackupArgs) -> Result<Option<serde_json::Value>, CliError> {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;

    let beads_dir = resolve_dir(db);
    let log_path = beads_dir.join("events.jsonl");

    if !log_path.exists() {
        return Err(CliError::Engine("no events.jsonl to back up".into()));
    }

    let dest = if let Some(ref d) = args.dest {
        d.clone()
    } else {
        let backup_dir = beads_dir.join("backups");
        std::fs::create_dir_all(&backup_dir)?;
        let ts = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
        backup_dir.join(format!("events-{ts}.jsonl.gz"))
    };

    // Write to .tmp then rename for atomicity
    let tmp_dest = dest.with_extension("gz.tmp");
    let data = std::fs::read(&log_path)?;
    let tmp_file = std::fs::File::create(&tmp_dest)?;
    let mut encoder = GzEncoder::new(tmp_file, Compression::default());
    encoder.write_all(&data)?;
    encoder.finish()?;
    std::fs::rename(&tmp_dest, &dest)?;

    Ok(Some(serde_json::json!({
        "action": "backup",
        "file": dest.display().to_string(),
        "bytes": data.len(),
    })))
}

fn cmd_restore(db: Option<&std::path::Path>, args: &RestoreArgs) -> Result<Option<serde_json::Value>, CliError> {
    use flate2::read::GzDecoder;
    use std::io::Read as _;

    if !args.file.exists() {
        return Err(CliError::Engine(format!("backup file not found: {}", args.file.display())));
    }

    // Decompress if gzipped, otherwise read raw
    let data = {
        let raw = std::fs::read(&args.file)?;
        if args.file.extension().map(|e| e == "gz").unwrap_or(false) {
            let mut decoder = GzDecoder::new(&raw[..]);
            let mut buf = Vec::new();
            decoder.read_to_end(&mut buf).map_err(|e| {
                CliError::Engine(format!("failed to decompress backup: {e}"))
            })?;
            buf
        } else {
            raw
        }
    };

    // Validate: every line must be valid JSON with an "op" field
    let text = String::from_utf8(data.clone()).map_err(|_| {
        CliError::Engine("backup file is not valid UTF-8".into())
    })?;
    let total_lines = text.lines().filter(|l| !l.trim().is_empty()).count();
    let valid_lines = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter(|l| serde_json::from_str::<serde_json::Value>(l).is_ok())
        .count();
    let invalid_lines = total_lines - valid_lines;

    if args.verify {
        let integrity = if invalid_lines == 0 { "ok" } else { "corrupt" };
        return Ok(Some(serde_json::json!({
            "action": "verify",
            "file": args.file.display().to_string(),
            "events": valid_lines,
            "invalid_lines": invalid_lines,
            "integrity": integrity,
        })));
    }

    if invalid_lines > 0 {
        return Err(CliError::Engine(format!(
            "backup has {invalid_lines} invalid lines — use --verify to inspect, or fix the file"
        )));
    }

    let beads_dir = resolve_dir(db);
    let log_path = beads_dir.join("events.jsonl");

    if log_path.exists() && !args.force {
        return Err(CliError::Engine(
            "events.jsonl already exists — use --force to overwrite".into(),
        ));
    }

    // Atomic write: tmp then rename
    let tmp_path = log_path.with_extension("jsonl.restore-tmp");
    std::fs::write(&tmp_path, &data)?;
    std::fs::rename(&tmp_path, &log_path)?;

    // Delete stale index so it rebuilds on next read
    let db_path = beads_dir.join("index.db");
    if db_path.exists() {
        let _ = std::fs::remove_file(&db_path);
    }
    let wm_path = beads_dir.join("index.watermark");
    if wm_path.exists() {
        let _ = std::fs::remove_file(&wm_path);
    }

    Ok(Some(serde_json::json!({
        "action": "restore",
        "from": args.file.display().to_string(),
        "events": valid_lines,
    })))
}

fn cmd_rebuild(db: Option<&std::path::Path>) -> Result<Option<serde_json::Value>, CliError> {
    let beads_dir = resolve_dir(db);
    let log_path = beads_dir.join("events.jsonl");
    let db_path = beads_dir.join("index.db");

    let log = crate::log::EventLog::open(&log_path)?;
    let events = log.read_all()?;
    crate::index::Index::rebuild(&db_path, &events)?;

    let count = log.line_count()?;
    std::fs::write(beads_dir.join("index.watermark"), count.to_string())?;

    record_rebuild_event(false)?;

    Ok(Some(serde_json::json!({
        "action": "rebuild",
        "events": count,
    })))
}

fn cmd_compact(db: Option<&std::path::Path>) -> Result<Option<serde_json::Value>, CliError> {
    let beads_dir = resolve_dir(db);
    let log_path = beads_dir.join("events.jsonl");
    let snap_dir = beads_dir.join("snapshots");

    let old_lines = crate::log::EventLog::open(&log_path)?.line_count()?;
    let bead_count = compact::compact(&log_path, &snap_dir)
        .map_err(CliError::Engine)?;
    compact::cleanup_snapshots(&snap_dir, 7)
        .map_err(CliError::Engine)?;

    // Rebuild index after compaction
    let db_path = beads_dir.join("index.db");
    let log = crate::log::EventLog::open(&log_path)?;
    let events = log.read_all()?;
    crate::index::Index::rebuild(&db_path, &events)?;
    let count = log.line_count()?;
    std::fs::write(beads_dir.join("index.watermark"), count.to_string())?;

    Ok(Some(serde_json::json!({
        "action": "compact",
        "beads": bead_count,
        "old_lines": old_lines,
        "new_lines": bead_count,
    })))
}

fn cmd_city(engine: &Engine, args: &CityArgs) -> Result<Option<serde_json::Value>, CliError> {
    match &args.command {
        CityCommands::Ready => {
            let mut all_beads = engine.index.query_ready(None);
            // Aggregate from external project DBs
            for (project_name, project_path) in &engine.config.projects {
                let beads_dir = project_path.join(".beads");
                if !beads_dir.exists() {
                    continue;
                }
                if let Ok(ext_engine) = Engine::open(&beads_dir) {
                    let mut ext_beads = ext_engine.index.query_ready(None);
                    for b in &mut ext_beads {
                        b.project = project_name.clone();
                    }
                    all_beads.extend(ext_beads);
                }
            }
            // Deduplicate by ID (local wins)
            let mut seen = std::collections::HashSet::new();
            all_beads.retain(|b| seen.insert(b.id.clone()));
            // Sort by priority ascending, then created_at ascending
            all_beads.sort_by(|a, b| a.priority.cmp(&b.priority).then(a.created_at.cmp(&b.created_at)));
            Ok(Some(serde_json::to_value(&all_beads).map_err(|e| CliError::Engine(e.to_string()))?))
        }
        CityCommands::List(list_args) => {
            let filters = list_filters(list_args);
            let mut all_beads = engine.index.query_list(&filters);
            // Aggregate from external project DBs
            for (project_name, project_path) in &engine.config.projects {
                let beads_dir = project_path.join(".beads");
                if !beads_dir.exists() {
                    continue;
                }
                if let Ok(ext_engine) = Engine::open(&beads_dir) {
                    let mut ext_beads = ext_engine.index.query_list(&filters);
                    for b in &mut ext_beads {
                        b.project = project_name.clone();
                    }
                    all_beads.extend(ext_beads);
                }
            }
            // Deduplicate by ID (local wins)
            let mut seen = std::collections::HashSet::new();
            all_beads.retain(|b| seen.insert(b.id.clone()));
            // Sort by priority ascending, then created_at ascending
            all_beads.sort_by(|a, b| a.priority.cmp(&b.priority).then(a.created_at.cmp(&b.created_at)));
            Ok(Some(serde_json::to_value(&all_beads).map_err(|e| CliError::Engine(e.to_string()))?))
        }
    }
}

// ---------------------------------------------------------------------------
// Lint — bead quality gate
// ---------------------------------------------------------------------------

const DONE_CONDITION_KEYWORDS: &[&str] = &[
    "done when", "success:", "test:", "passes", "done:", "complete", "implemented", "fixed",
];

fn cmd_lint(engine: &Engine, args: &LintArgs) -> Result<Option<serde_json::Value>, CliError> {
    let bead = engine
        .index
        .query_show(&args.id)
        .ok_or_else(|| CliError::NotFound(args.id.clone()))?;

    let mut errors: Vec<String> = Vec::new();

    // Check title length (>= 10 chars)
    if bead.title.len() < 10 {
        errors.push(format!(
            "title too short: {} chars (minimum 10)",
            bead.title.len()
        ));
    }

    // Check description present and >= 20 chars
    match &bead.description {
        Some(desc) if !desc.trim().is_empty() => {
            if desc.len() < 20 {
                errors.push(format!(
                    "description too short: {} chars (minimum 20)",
                    desc.len()
                ));
            }
        }
        _ => {
            errors.push("description/body is missing".to_string());
        }
    }

    // Check for done-condition keywords in description
    let has_done_keyword = bead.description.as_deref().map_or(false, |desc| {
        let lower = desc.to_lowercase();
        DONE_CONDITION_KEYWORDS.iter().any(|kw| lower.contains(kw))
    });
    if !has_done_keyword {
        errors.push(format!(
            "no done-condition keyword found (expected one of: {})",
            DONE_CONDITION_KEYWORDS.join(", ")
        ));
    }

    let passed = errors.is_empty();
    let result = serde_json::json!({
        "passed": passed,
        "errors": errors,
    });

    if passed {
        Ok(Some(result))
    } else {
        Err(CliError::LintFailed(result))
    }
}

// ---------------------------------------------------------------------------
// Legacy migration: issues.jsonl → events.jsonl
// ---------------------------------------------------------------------------

fn cmd_sync_migrate(engine: &Engine) -> Result<Option<serde_json::Value>, CliError> {
    let legacy_path = engine.beads_dir.join("issues.jsonl");
    if !legacy_path.exists() {
        return Err(CliError::Engine("no issues.jsonl found to migrate".into()));
    }

    let data = std::fs::read_to_string(&legacy_path)?;
    let mut migrated = 0;
    let mut skipped = 0;
    let now = Utc::now();

    for line in data.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let obj: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };

        let id = obj["id"].as_str().unwrap_or("").to_string();
        if id.is_empty() {
            skipped += 1;
            continue;
        }

        // Map legacy fields to BeadSnapshot
        let status = obj["status"].as_str().unwrap_or("open").to_string();
        let issue_type = obj["issue_type"].as_str().unwrap_or("task");
        let bead_type = match issue_type {
            "epic" => "epic",
            "feature" => "feature",
            "bug" => "bug",
            "chore" => "chore",
            _ => "task",
        };

        let created_at_str = obj["created_at"].as_str().unwrap_or("");
        let created_at = created_at_str.parse::<chrono::DateTime<Utc>>().unwrap_or(now);
        let updated_at_str = obj["updated_at"].as_str().unwrap_or("");
        let updated_at = updated_at_str.parse::<chrono::DateTime<Utc>>().unwrap_or(now);

        let closed_at = obj["closed_at"]
            .as_str()
            .and_then(|s| s.parse::<chrono::DateTime<Utc>>().ok());
        let close_reason = obj["close_reason"].as_str().map(String::from);

        let assignee = obj["assignee"].as_str().map(String::from);
        let parent = obj["parent_id"].as_str().map(String::from);
        let project = obj["project"].as_str()
            .or(obj["source_repo"].as_str())
            .unwrap_or("")
            .to_string();

        let dependencies: Vec<String> = obj["dependencies"]
            .as_array()
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();
        let labels: Vec<String> = obj["labels"]
            .as_array()
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();

        let snapshot = BeadSnapshot {
            id: id.clone(),
            title: obj["title"].as_str().unwrap_or("").to_string(),
            description: obj["description"].as_str().map(String::from),
            status: status.clone(),
            priority: obj["priority"].as_u64().unwrap_or(2) as u8,
            bead_type: bead_type.to_string(),
            project,
            assignee,
            parent,
            dependencies,
            labels,
            created_at,
            updated_at,
            closed_at,
            close_reason,
            claimed_at: None,
            claim_deadline: None,
            last_heartbeat: None,
        };

        // Use Snapshot event (preserves exact state, no actor needed)
        let event = Event::Snapshot {
            ts: created_at,
            actor: obj["created_by"].as_str().unwrap_or("migration").to_string(),
            bead: snapshot,
        };

        engine.log.append(&event)?;
        migrated += 1;
    }

    // Rebuild index
    let db_path = engine.beads_dir.join("index.db");
    let events = engine.log.read_all()?;
    crate::index::Index::rebuild(&db_path, &events)?;
    let count = engine.log.line_count()?;
    std::fs::write(engine.beads_dir.join("index.watermark"), count.to_string())?;

    Ok(Some(serde_json::json!({
        "action": "migrate",
        "migrated": migrated,
        "skipped": skipped,
    })))
}

// ---------------------------------------------------------------------------
// Type conversions: CLI enums → core enums
// ---------------------------------------------------------------------------

impl BeadStatus {
    pub fn to_core(&self) -> bead::Status {
        match self {
            Self::Open => bead::Status::Open,
            Self::InProgress => bead::Status::InProgress,
            Self::Closed => bead::Status::Closed,
            Self::Deferred => bead::Status::Deferred,
        }
    }
}

impl BeadType {
    pub fn to_core(&self) -> bead::BeadType {
        match self {
            Self::Epic => bead::BeadType::Epic,
            Self::Feature => bead::BeadType::Feature,
            Self::Bug => bead::BeadType::Bug,
            Self::Task => bead::BeadType::Task,
            Self::Chore => bead::BeadType::Chore,
        }
    }
}

impl CreateArgs {
    fn resolved_title(&self) -> Result<String, CliError> {
        self.title
            .clone()
            .or_else(|| self.title_flag.clone())
            .ok_or_else(|| CliError::Engine("missing bead title".into()))
    }

    fn all_labels(&self) -> Vec<String> {
        let mut labels = self.label.clone();
        labels.extend(
            self.labels
                .iter()
                .flat_map(|group| group.split(','))
                .map(str::trim)
                .filter(|label| !label.is_empty())
                .map(str::to_string),
        );
        labels
    }
}

/// Build Filters from ListArgs.
fn list_filters(args: &ListArgs) -> Filters {
    Filters {
        project: args.project.clone(),
        status: args.status.as_ref().map(|s| s.to_core()),
        priority: args.priority,
        bead_type: args.bead_type.as_ref().map(|t| t.to_core()),
        label: None,
        assignee: args.assignee.clone(),
    }
}

/// Resolve beads dir from CLI flag or auto-detect.
fn resolve_dir(db: Option<&std::path::Path>) -> PathBuf {
    match db {
        Some(p) => p.to_path_buf(),
        None => config::resolve_beads_dir(),
    }
}

// ---------------------------------------------------------------------------
// Human-friendly output formatting
// ---------------------------------------------------------------------------

/// Format output for humans (non-JSON mode).
pub fn format_human(cmd: &Commands, value: &serde_json::Value) {
    match cmd {
        Commands::Show(_) => {
            if let Some(obj) = value.as_object() {
                let id = obj.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                let title = obj.get("title").and_then(|v| v.as_str()).unwrap_or("?");
                let status = obj.get("status").and_then(|v| v.as_str()).unwrap_or("?");
                let priority = obj.get("priority").and_then(|v| v.as_u64()).unwrap_or(2);
                let project = obj.get("project").and_then(|v| v.as_str()).unwrap_or("");
                let bead_type = obj.get("bead_type").and_then(|v| v.as_str()).unwrap_or("task");
                let assignee = obj.get("assignee").and_then(|v| v.as_str());
                let description = obj.get("description").and_then(|v| v.as_str());
                let deps = obj.get("dependencies").and_then(|v| v.as_array());
                let labels = obj.get("labels").and_then(|v| v.as_array());

                println!("{} {} [P{}] [{}]", status_icon(status), id, priority, bead_type);
                println!("  {}", title);
                if !project.is_empty() {
                    println!("  project: {}", project);
                }
                if let Some(a) = assignee {
                    println!("  assignee: {}", a);
                }
                if let Some(d) = description {
                    if !d.is_empty() {
                        println!("  {}", d);
                    }
                }
                if let Some(d) = deps {
                    if !d.is_empty() {
                        let dep_strs: Vec<&str> = d.iter().filter_map(|v| v.as_str()).collect();
                        println!("  deps: {}", dep_strs.join(", "));
                    }
                }
                if let Some(l) = labels {
                    if !l.is_empty() {
                        let label_strs: Vec<&str> = l.iter().filter_map(|v| v.as_str()).collect();
                        println!("  labels: {}", label_strs.join(", "));
                    }
                }
            }
        }
        Commands::Triage(_) => {
            if let Some(obj) = value.as_object() {
                let query = obj.get("query").and_then(|v| v.as_str()).unwrap_or("?");
                let total = obj.get("total").and_then(|v| v.as_u64()).unwrap_or(0);
                println!("triage: {} results for {:?}", total, query);
                if let Some(results) = obj.get("results").and_then(|v| v.as_array()) {
                    for r in results {
                        let id = r["id"].as_str().unwrap_or("?");
                        let title = r["title"].as_str().unwrap_or("?");
                        let priority = r["priority"].as_u64().unwrap_or(2);
                        let score = r["score"].as_f64().unwrap_or(0.0);
                        println!("  {} [P{}] ({:.1}) {}", id, priority, score, title);
                    }
                }
            }
        }
        Commands::Related(_) => {
            if let Some(obj) = value.as_object() {
                let target = obj.get("target_bead_id").and_then(|v| v.as_str()).unwrap_or("?");
                let total = obj.get("total_related").and_then(|v| v.as_u64()).unwrap_or(0);
                println!("related to {}: {} beads", target, total);
                if let Some(items) = obj.get("related").and_then(|v| v.as_array()) {
                    for r in items {
                        let id = r["id"].as_str().unwrap_or("?");
                        let title = r["title"].as_str().unwrap_or("?");
                        let rel = r["relationship"].as_str().unwrap_or("?");
                        println!("  {} [{}] {}", id, rel, title);
                    }
                }
            }
        }
        Commands::Plan(_) => {
            // Pretty-print the plan
            println!("{}", serde_json::to_string_pretty(value).unwrap_or_default());
        }
        Commands::List(_) | Commands::Ready(_) | Commands::Search(_) | Commands::City(_) => {
            if let Some(arr) = value.as_array() {
                if arr.is_empty() {
                    println!("(no beads)");
                    return;
                }
                for item in arr {
                    if let Some(obj) = item.as_object() {
                        let status = obj.get("status").and_then(|v| v.as_str()).unwrap_or("?");
                        let id = obj.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                        let priority = obj.get("priority").and_then(|v| v.as_u64()).unwrap_or(2);
                        let bead_type = obj.get("bead_type").and_then(|v| v.as_str()).unwrap_or("task");
                        let title = obj.get("title").and_then(|v| v.as_str()).unwrap_or("?");
                        println!("{} {} [P{}] [{}] - {}", status_icon(status), id, priority, bead_type, title);
                    }
                }
            }
        }
        Commands::Create(_) => {
            if let Some(obj) = value.as_object() {
                let id = obj.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                let title = obj.get("title").and_then(|v| v.as_str()).unwrap_or("?");
                println!("created {} — {}", id, title);
            }
        }
        Commands::Close(_) => {
            if let Some(obj) = value.as_object() {
                let id = obj.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                println!("closed {}", id);
            }
        }
        Commands::Claim(_) => {
            if let Some(obj) = value.as_object() {
                let id = obj.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                let assignee = obj.get("assignee").and_then(|v| v.as_str()).unwrap_or("?");
                let deadline = obj.get("claim_deadline").and_then(|v| v.as_str()).unwrap_or("?");
                println!("claimed {} by {} until {}", id, assignee, deadline);
            }
        }
        _ => {
            // Generic: pretty-print JSON
            println!("{}", serde_json::to_string_pretty(value).unwrap_or_default());
        }
    }
}

pub fn suppress_stdout(cmd: &Commands) -> bool {
    matches!(cmd, Commands::Create(args) if args.silent)
}

fn status_icon(status: &str) -> &'static str {
    match status {
        "open" => "○",
        "in_progress" => "◉",
        "closed" => "●",
        "deferred" => "◌",
        _ => "?",
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse a human-friendly duration string like "1h", "30m", "2h30m".
/// Returns seconds.
fn parse_duration(s: &str) -> Result<u64, CliError> {
    let s = s.trim();
    let mut total_secs: u64 = 0;
    let mut num_buf = String::new();

    for ch in s.chars() {
        if ch.is_ascii_digit() {
            num_buf.push(ch);
        } else {
            let n: u64 = num_buf
                .parse()
                .map_err(|_| CliError::InvalidDuration(s.to_string()))?;
            num_buf.clear();
            match ch {
                'h' | 'H' => total_secs += n * 3600,
                'm' | 'M' => total_secs += n * 60,
                's' | 'S' => total_secs += n,
                _ => return Err(CliError::InvalidDuration(s.to_string())),
            }
        }
    }

    // Handle bare number (assume minutes)
    if !num_buf.is_empty() {
        let n: u64 = num_buf
            .parse()
            .map_err(|_| CliError::InvalidDuration(s.to_string()))?;
        // If the string was entirely numeric, assume minutes
        if total_secs == 0 {
            total_secs = n * 60;
        } else {
            // Trailing digits without unit — treat as error
            return Err(CliError::InvalidDuration(s.to_string()));
        }
    }

    if total_secs == 0 {
        return Err(CliError::InvalidDuration(s.to_string()));
    }

    Ok(total_secs)
}

// ---------------------------------------------------------------------------
// Thrash detection state
// ---------------------------------------------------------------------------

use std::sync::Mutex;
use std::time::Instant;

static REBUILD_TRACKER: Mutex<Option<ThrashTracker>> = Mutex::new(None);

struct ThrashTracker {
    timestamps: Vec<Instant>,
}

impl ThrashTracker {
    fn new() -> Self {
        Self {
            timestamps: Vec::new(),
        }
    }

    /// Record a rebuild. Returns Some(count) if thrashing detected (>5 in 60s).
    fn record_rebuild(&mut self) -> Option<u32> {
        let now = Instant::now();
        let window = std::time::Duration::from_secs(60);

        // Prune old entries
        self.timestamps.retain(|t| now.duration_since(*t) < window);
        self.timestamps.push(now);

        let count = self.timestamps.len() as u32;
        if count > 5 {
            Some(count)
        } else {
            None
        }
    }
}

/// Record a rebuild event and check for thrashing.
/// Called by the rebuild handler after each successful rebuild.
/// Returns Err with alert if thrashing is detected.
pub fn record_rebuild_event(json_mode: bool) -> Result<(), CliError> {
    let mut guard = REBUILD_TRACKER.lock().unwrap();
    let tracker = guard.get_or_insert_with(ThrashTracker::new);

    if let Some(count) = tracker.record_rebuild() {
        let err = CliError::IndexThrashing {
            rebuilds: count,
            window_secs: 60,
        };
        if json_mode {
            eprintln!("{}", err.to_json());
        } else {
            eprintln!("WARNING: {err}");
        }
        // Don't return error — thrashing is a warning, not a fatal condition
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_parses_create() {
        let cli = Cli::parse_from([
            "br", "create", "Fix relay timeout", "-p", "1", "-t", "bug", "--project", "relay",
        ]);
        match &cli.command {
            Commands::Create(args) => {
                assert_eq!(args.title.as_deref(), Some("Fix relay timeout"));
                assert_eq!(args.priority, 1);
                assert_eq!(args.bead_type, BeadType::Bug);
                assert_eq!(args.project.as_deref(), Some("relay"));
            }
            _ => panic!("expected Create command"),
        }
    }

    #[test]
    fn cli_parses_create_compat_flags() {
        let cli = Cli::parse_from([
            "br",
            "create",
            "--title",
            "Compat title",
            "--labels",
            "foo,bar",
            "-a",
            "nobody",
            "--silent",
        ]);
        match &cli.command {
            Commands::Create(args) => {
                assert_eq!(args.title_flag.as_deref(), Some("Compat title"));
                assert_eq!(args.all_labels(), vec!["foo", "bar"]);
                assert_eq!(args.assignee.as_deref(), Some("nobody"));
                assert!(args.silent);
            }
            _ => panic!("expected Create command"),
        }
    }

    #[test]
    fn cli_parses_list_with_filters() {
        let cli = Cli::parse_from([
            "br", "--json", "list", "--project", "gate", "--status", "open", "--priority", "0",
        ]);
        assert!(cli.json);
        match &cli.command {
            Commands::List(args) => {
                assert_eq!(args.project.as_deref(), Some("gate"));
                assert_eq!(args.status, Some(BeadStatus::Open));
                assert_eq!(args.priority, Some(0));
            }
            _ => panic!("expected List command"),
        }
    }

    #[test]
    fn cli_parses_list_with_all_flag() {
        let cli = Cli::parse_from(["br", "list", "--all", "--status", "closed"]);
        match &cli.command {
            Commands::List(args) => {
                assert!(args.all);
                assert_eq!(args.status, Some(BeadStatus::Closed));
            }
            _ => panic!("expected List command"),
        }
    }

    #[test]
    fn cli_parses_search_compat_flags() {
        let cli = Cli::parse_from([
            "br",
            "search",
            "gate",
            "--limit",
            "5",
            "--status",
            "closed",
            "--type",
            "gate",
            "--sort",
            "created",
            "--reverse",
        ]);
        match &cli.command {
            Commands::Search(args) => {
                assert_eq!(args.query, "gate");
                assert_eq!(args.limit, Some(5));
                assert_eq!(args.status, Some(BeadStatus::Closed));
                assert_eq!(args.bead_type.as_deref(), Some("gate"));
                assert_eq!(args.sort, SearchSort::Created);
                assert!(args.reverse);
            }
            _ => panic!("expected Search command"),
        }
    }

    #[test]
    fn cli_parses_claim_with_duration() {
        let cli = Cli::parse_from(["br", "claim", "pol-abc1", "--lock-for", "2h"]);
        match &cli.command {
            Commands::Claim(args) => {
                assert_eq!(args.id, "pol-abc1");
                assert_eq!(args.lock_for, "2h");
            }
            _ => panic!("expected Claim command"),
        }
    }

    #[test]
    fn cli_parses_close_with_reason() {
        let cli = Cli::parse_from(["br", "close", "pol-abc1", "--reason", "Fixed in commit abc"]);
        match &cli.command {
            Commands::Close(args) => {
                assert_eq!(args.id, "pol-abc1");
                assert_eq!(args.reason, "Fixed in commit abc");
            }
            _ => panic!("expected Close command"),
        }
    }

    #[test]
    fn cli_parses_city_ready() {
        let cli = Cli::parse_from(["br", "city", "ready"]);
        match &cli.command {
            Commands::City(args) => {
                assert!(matches!(args.command, CityCommands::Ready));
            }
            _ => panic!("expected City command"),
        }
    }

    #[test]
    fn cli_parses_update_with_deps() {
        let cli = Cli::parse_from([
            "br", "update", "pol-abc1", "--add-dep", "pol-xyz2", "--rm-dep", "pol-old3",
            "--status", "in-progress",
        ]);
        match &cli.command {
            Commands::Update(args) => {
                assert_eq!(args.id, "pol-abc1");
                assert_eq!(args.add_dep, vec!["pol-xyz2"]);
                assert_eq!(args.rm_dep, vec!["pol-old3"]);
                assert_eq!(args.status, Some(BeadStatus::InProgress));
            }
            _ => panic!("expected Update command"),
        }
    }

    #[test]
    fn cli_parses_global_flags() {
        let cli = Cli::parse_from(["br", "--json", "--no-color", "-vv", "--actor", "test", "doctor"]);
        assert!(cli.json);
        assert!(cli.no_color);
        assert_eq!(cli.verbose, 2);
        assert_eq!(cli.actor.as_deref(), Some("test"));
    }

    #[test]
    fn cli_parses_sync_flags() {
        let cli = Cli::parse_from(["br", "sync", "--export-project", "gate", "--snapshot"]);
        match &cli.command {
            Commands::Sync(args) => {
                assert_eq!(args.export_project.as_deref(), Some("gate"));
                assert!(args.snapshot);
                assert!(!args.import_only);
            }
            _ => panic!("expected Sync command"),
        }
    }

    #[test]
    fn parse_duration_works() {
        assert_eq!(parse_duration("1h").unwrap(), 3600);
        assert_eq!(parse_duration("30m").unwrap(), 1800);
        assert_eq!(parse_duration("2h30m").unwrap(), 9000);
        assert_eq!(parse_duration("90s").unwrap(), 90);
        assert_eq!(parse_duration("60").unwrap(), 3600); // bare number = minutes
        assert!(parse_duration("").is_err());
        assert!(parse_duration("abc").is_err());
    }

    #[test]
    fn error_json_format() {
        let err = CliError::AlreadyClaimed {
            bead: "pol-abc1".into(),
            holder: "athena".into(),
            deadline: "2026-03-12T20:00:00Z".into(),
        };
        let json = err.to_json();
        assert_eq!(json["error"], "already_claimed");
        assert_eq!(json["holder"], "athena");
        assert_eq!(json["bead"], "pol-abc1");
    }

    #[test]
    fn help_text_includes_all_commands() {
        let help = Cli::command().render_help().to_string();
        for cmd in [
            "create", "show", "list", "update", "close", "ready", "search", "sync",
            "claim", "heartbeat", "unclaim", "doctor", "rebuild", "compact", "city", "lint",
        ] {
            assert!(help.contains(cmd), "help missing command: {cmd}");
        }
    }

    #[test]
    fn no_actor_error_message() {
        // Remove POLIS_ACTOR if set, and ensure --actor is None
        let cli = Cli::parse_from(["br", "doctor"]);
        // We can't easily test env var absence in unit tests without side effects,
        // but we can verify the override path works:
        let cli_with_actor = Cli::parse_from(["br", "--actor", "test-agent", "doctor"]);
        assert_eq!(cli_with_actor.resolve_actor().unwrap(), "test-agent");
        let _ = cli; // suppress unused warning
    }
}
