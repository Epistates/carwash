//! Command-line interface definition.

use carwash_core::ArtifactKind;
use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Parser)]
#[command(
    name = "carwash",
    version,
    about = "Reclaim disk space from build outputs, dependency installs and caches across all your projects",
    long_about = "Reclaim disk space from build outputs, dependency installs and caches across all your projects.\n\n\
                  Run without a subcommand to open the interactive UI on PATH (default: current directory).",
    args_conflicts_with_subcommands = true,
    arg_required_else_help = false
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Directory to open in the interactive UI.
    pub path: Option<PathBuf>,

    #[command(flatten)]
    pub global: GlobalArgs,
}

#[derive(Debug, Args)]
pub struct GlobalArgs {
    /// Configuration file (default: ~/.config/carwash/config.toml).
    #[arg(long, global = true, value_name = "FILE", env = "CARWASH_CONFIG")]
    pub config: Option<PathBuf>,

    /// I/O threads (default: derived from the CPU count).
    #[arg(long, global = true, value_name = "N")]
    pub threads: Option<usize>,

    /// When to use colors.
    #[arg(long, global = true, value_enum, default_value_t = ColorChoice::Auto)]
    pub color: ColorChoice,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ColorChoice {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// List reclaimable artifacts without deleting anything.
    Scan(ScanArgs),
    /// Delete artifacts, after showing the plan and asking for confirmation.
    Clean(CleanArgs),
    /// List project tasks: package.json scripts, just/make/Taskfile/mise targets, standard commands.
    Tasks(TasksArgs),
    /// Run a task in every project that has it.
    Run(RunArgs),
    /// Show outdated and vulnerable dependencies (Rust, JavaScript, Python, Go).
    Outdated(OutdatedArgs),
    /// List the ecosystems carwash recognises.
    Ecosystems(JsonArgs),
    /// Show how much space carwash has reclaimed.
    History(JsonArgs),
    /// Print shell completions.
    Completions {
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
}

#[derive(Debug, Args)]
pub struct WalkArgs {
    /// Directory to scan.
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Never enter this path (repeatable).
    #[arg(long, value_name = "PATH")]
    pub exclude: Vec<PathBuf>,

    /// Also descend into hidden directories.
    #[arg(long)]
    pub hidden: bool,

    /// Maximum directory depth below PATH.
    #[arg(long, value_name = "N")]
    pub max_depth: Option<usize>,

    /// Cross filesystem boundaries (mount points).
    #[arg(long)]
    pub cross_fs: bool,

    /// Ignore projects enclosing PATH (e.g. a workspace's shared target/ when PATH is a member).
    #[arg(long)]
    pub no_enclosing: bool,
}

impl WalkArgs {
    /// Defaults for `path`, as when no walk flags are given.
    pub fn for_path(path: PathBuf) -> Self {
        Self {
            path,
            exclude: Vec::new(),
            hidden: false,
            max_depth: None,
            cross_fs: false,
            no_enclosing: false,
        }
    }
}

#[derive(Debug, Clone, Args)]
pub struct FilterArgs {
    /// Only artifacts freeing at least this much, e.g. 100MB, 1.5GB.
    #[arg(long, value_name = "SIZE", value_parser = parse_size)]
    pub min_size: Option<u64>,

    /// Only artifacts not modified for this long, e.g. 30d, 6w, 3mo, 1y.
    #[arg(long, value_name = "AGE", value_parser = parse_age)]
    pub older_than: Option<Duration>,

    /// Only these kinds (build, deps, cache, env, other, leftover).
    #[arg(long, value_name = "KIND", value_delimiter = ',', value_parser = parse_kind)]
    pub kind: Vec<ArtifactKind>,

    /// Only these ecosystems, by id (see `carwash ecosystems`).
    #[arg(
        long = "ecosystem",
        short = 'e',
        value_name = "ID",
        value_delimiter = ','
    )]
    pub ecosystems: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, ValueEnum)]
pub enum SortKey {
    /// Largest reclaimable first.
    #[default]
    Size,
    /// Oldest first.
    Age,
    Path,
}

#[derive(Debug, Args)]
pub struct ScanArgs {
    #[command(flatten)]
    pub walk: WalkArgs,

    #[command(flatten)]
    pub filter: FilterArgs,

    /// Machine-readable output.
    #[arg(long)]
    pub json: bool,

    /// Skip git inspection (faster; ambiguous directories stay "needs review").
    #[arg(long)]
    pub no_git: bool,

    /// Skip size measurement.
    #[arg(long)]
    pub no_size: bool,

    #[arg(long, value_enum, default_value_t)]
    pub sort: SortKey,

    /// Show at most N rows.
    #[arg(long, short = 'n', value_name = "N")]
    pub limit: Option<usize>,
}

#[derive(Debug, Args)]
pub struct CleanArgs {
    #[command(flatten)]
    pub walk: WalkArgs,

    #[command(flatten)]
    pub filter: FilterArgs,

    /// Show what would be deleted, then stop.
    #[arg(long)]
    pub dry_run: bool,

    /// Do not ask for confirmation.
    #[arg(long, short = 'y')]
    pub yes: bool,

    /// Move to the trash instead of deleting (frees nothing until the trash is emptied).
    #[arg(long)]
    pub trash: bool,

    /// Include directories with generic names that git does not confirm as ignored.
    #[arg(long)]
    pub include_review: bool,

    /// Include artifacts modified recently (default window: 7 days).
    #[arg(long)]
    pub include_recent: bool,

    /// Machine-readable output.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct TasksArgs {
    #[command(flatten)]
    pub walk: WalkArgs,

    /// Only projects of these ecosystems.
    #[arg(
        long = "ecosystem",
        short = 'e',
        value_name = "ID",
        value_delimiter = ','
    )]
    pub ecosystems: Vec<String>,

    /// List every project's tasks instead of a summary.
    #[arg(long)]
    pub all: bool,

    /// Machine-readable output.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct RunArgs {
    /// Task name, e.g. `test` or `build`.
    pub task: String,

    #[command(flatten)]
    pub walk: WalkArgs,

    /// Only projects of these ecosystems.
    #[arg(
        long = "ecosystem",
        short = 'e',
        value_name = "ID",
        value_delimiter = ','
    )]
    pub ecosystems: Vec<String>,

    /// Only projects whose path (relative to PATH) matches this glob (repeatable).
    #[arg(long, value_name = "GLOB")]
    pub filter: Vec<String>,

    /// Projects to run at once; output is prefixed when above 1.
    #[arg(long, short = 'j', value_name = "N", default_value_t = 1)]
    pub jobs: usize,

    /// Stop starting new projects after the first failure.
    #[arg(long)]
    pub fail_fast: bool,

    /// Print the commands without running them.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Args)]
pub struct OutdatedArgs {
    #[command(flatten)]
    pub walk: WalkArgs,

    /// Only projects of these ecosystems.
    #[arg(
        long = "ecosystem",
        short = 'e',
        value_name = "ID",
        value_delimiter = ','
    )]
    pub ecosystems: Vec<String>,

    /// Also list dependencies that are up to date.
    #[arg(long)]
    pub all: bool,

    /// Skip the vulnerability lookup (OSV).
    #[arg(long)]
    pub no_vulns: bool,

    /// Ignore cached registry data.
    #[arg(long)]
    pub refresh: bool,

    /// Exit with status 1 when anything is outdated or vulnerable (for CI).
    #[arg(long)]
    pub exit_code: bool,

    /// Machine-readable output.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct JsonArgs {
    /// Machine-readable output.
    #[arg(long)]
    pub json: bool,
}

fn parse_size(s: &str) -> Result<u64, String> {
    carwash_core::fmt::parse_bytes(s)
        .ok_or_else(|| format!("invalid size `{s}` (try 100MB or 1.5GB)"))
}

fn parse_age(s: &str) -> Result<Duration, String> {
    carwash_core::fmt::parse_age(s)
        .ok_or_else(|| format!("invalid age `{s}` (try 30d, 6w, 3mo or 1y)"))
}

fn parse_kind(s: &str) -> Result<ArtifactKind, String> {
    ArtifactKind::parse(s).ok_or_else(|| {
        format!("unknown kind `{s}` (expected build, deps, cache, env, other or leftover)")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parses_filters() {
        let cli = Cli::try_parse_from([
            "carwash",
            "scan",
            "~/work",
            "--min-size",
            "1GB",
            "--older-than",
            "30d",
            "--kind",
            "build,deps",
            "-e",
            "rust,node",
        ])
        .unwrap();
        let Some(Command::Scan(scan)) = cli.command else {
            panic!("expected scan");
        };
        assert_eq!(scan.filter.min_size, Some(1_000_000_000));
        assert_eq!(
            scan.filter.kind,
            vec![ArtifactKind::Build, ArtifactKind::Dependencies]
        );
        assert_eq!(scan.filter.ecosystems, vec!["rust", "node"]);
    }

    #[test]
    fn bare_path_opens_the_ui() {
        let cli = Cli::try_parse_from(["carwash", "/tmp"]).unwrap();
        assert!(cli.command.is_none());
        assert_eq!(cli.path, Some(PathBuf::from("/tmp")));
    }
}
