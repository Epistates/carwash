//! carwash: reclaim disk space across every project on your machine.

mod cli;
mod commands;
mod config;
mod context;
mod report;
mod table;

use clap::{CommandFactory, Parser};
use cli::{Cli, ColorChoice, Command};
use context::Context;
use std::process::ExitCode;

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.global.color {
        ColorChoice::Always => anstream::ColorChoice::Always.write_global(),
        ColorChoice::Never => anstream::ColorChoice::Never.write_global(),
        ColorChoice::Auto => {}
    }
    init_logging();
    match run(cli) {
        Ok(code) => code,
        Err(error) => {
            anstream::eprintln!(
                "{}error:{} {error:#}",
                anstyle::AnsiColor::Red.on_default().bold().render(),
                anstyle::Reset.render()
            );
            ExitCode::from(2)
        }
    }
}

/// CLI logging goes to stderr and is off unless `CARWASH_LOG` is set (e.g. `debug`).
fn init_logging() {
    if let Ok(filter) = tracing_subscriber::EnvFilter::try_from_env("CARWASH_LOG") {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .init();
    }
}

fn run(cli: Cli) -> anyhow::Result<ExitCode> {
    if let Some(Command::Completions { shell }) = cli.command {
        clap_complete::generate(
            shell,
            &mut Cli::command(),
            "carwash",
            &mut std::io::stdout(),
        );
        return Ok(ExitCode::SUCCESS);
    }
    let ctx = Context::load(&cli.global)?;
    match &cli.command {
        Some(Command::Scan(args)) => commands::scan::run(&ctx, args),
        Some(Command::Clean(args)) => commands::clean::run(&ctx, args),
        Some(Command::Ecosystems(args)) => commands::info::ecosystems(&ctx, args),
        Some(Command::History(args)) => commands::info::history(&ctx, args),
        Some(Command::Completions { .. }) => unreachable!("handled above"),
        None => {
            // The interactive UI lands in the next phase; scan meanwhile.
            let path = cli.path.clone().unwrap_or_else(|| ".".into());
            let args = cli::ScanArgs {
                walk: cli::WalkArgs {
                    path,
                    exclude: Vec::new(),
                    hidden: false,
                    max_depth: None,
                    cross_fs: false,
                    no_enclosing: false,
                },
                filter: cli::FilterArgs {
                    min_size: None,
                    older_than: None,
                    kind: Vec::new(),
                    ecosystems: Vec::new(),
                },
                json: false,
                no_git: false,
                no_size: false,
                sort: cli::SortKey::Size,
                limit: None,
            };
            commands::scan::run(&ctx, &args)
        }
    }
}
