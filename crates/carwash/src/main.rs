//! carwash: reclaim disk space across every project on your machine.

mod cli;
mod commands;
mod config;
mod context;
mod report;
mod table;
mod tui;

use anyhow::bail;
use clap::{CommandFactory, Parser};
use cli::{Cli, ColorChoice, Command};
use context::Context;
use std::io::IsTerminal;
use std::process::ExitCode;

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.global.color {
        ColorChoice::Always => anstream::ColorChoice::Always.write_global(),
        ColorChoice::Never => anstream::ColorChoice::Never.write_global(),
        ColorChoice::Auto => {}
    }
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

/// Logging is off unless `CARWASH_LOG` is set (e.g. `debug`). Commands log to stderr; the
/// interactive UI logs to a file, since stderr would corrupt the screen.
fn init_logging(ctx: Option<&Context>) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let filter = tracing_subscriber::EnvFilter::try_from_env("CARWASH_LOG").ok()?;
    match ctx.and_then(|c| c.dirs.as_ref()) {
        Some(dirs) => {
            let appender = tracing_appender::rolling::daily(dirs.log_dir(), "carwash.log");
            let (writer, guard) = tracing_appender::non_blocking(appender);
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_ansi(false)
                .with_writer(writer)
                .init();
            Some(guard)
        }
        None => {
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_writer(std::io::stderr)
                .init();
            None
        }
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
    let interactive = cli.command.is_none();
    let _log_guard = init_logging(interactive.then_some(&ctx));
    match &cli.command {
        Some(Command::Scan(args)) => commands::scan::run(&ctx, args),
        Some(Command::Clean(args)) => commands::clean::run(&ctx, args),
        Some(Command::Tasks(args)) => commands::tasks::list(&ctx, args),
        Some(Command::Run(args)) => commands::tasks::run(&ctx, args),
        Some(Command::Ecosystems(args)) => commands::info::ecosystems(&ctx, args),
        Some(Command::History(args)) => commands::info::history(&ctx, args),
        Some(Command::Completions { .. }) => unreachable!("handled above"),
        None => {
            if !std::io::stdout().is_terminal() || !std::io::stdin().is_terminal() {
                bail!(
                    "the interactive UI needs a terminal; use `carwash scan` or `carwash clean` in scripts"
                );
            }
            let walk = cli::WalkArgs::for_path(cli.path.clone().unwrap_or_else(|| ".".into()));
            let (root, options) = ctx.scan_options(&walk, true, true)?;
            tui::run(&ctx, root, options)?;
            Ok(ExitCode::SUCCESS)
        }
    }
}
