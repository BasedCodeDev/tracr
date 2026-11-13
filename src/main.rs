use clap::{Parser, Subcommand, builder::TypedValueParser};
use serde::Serialize;
use std::{path::PathBuf, process::ExitCode};
use tracr::{
    browser::{Browser, Discovery, SourceDiagnostic, SourceSpec},
    config::{self, Environment},
    domain::{FolderStatus, SessionSummary, classify_folder},
};

#[derive(Parser)]
#[command(
    version,
    about = "Browse coding-agent sessions by folder",
    subcommand_precedence_over_arg = true
)]
struct Cli {
    /// Open sessions recorded directly in this folder.
    #[arg(value_name = "PATH", value_parser = clap::builder::OsStringValueParser::new().try_map(parse_folder))]
    path: Option<PathBuf>,
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[arg(long, global = true)]
    codex_home: Vec<PathBuf>,
    #[arg(long, global = true, default_value = "codex")]
    codex_bin: PathBuf,
    #[command(subcommand)]
    command: Option<Command>,
}
#[derive(Subcommand)]
enum Command {
    /// Browse sessions in the terminal.
    Browse {
        #[arg(value_name = "PATH", value_parser = clap::builder::OsStringValueParser::new().try_map(parse_folder))]
        path: Option<PathBuf>,
        #[arg(long, conflicts_with = "codex_home")]
        fixture: Option<PathBuf>,
        #[arg(long)]
        include_archived: bool,
    },
    /// List session metadata without opening the terminal interface.
    List {
        #[arg(long, conflicts_with = "codex_home")]
        fixture: Option<PathBuf>,
        #[arg(long)]
        include_archived: bool,
        #[arg(long)]
        json: bool,
    },
}
fn parse_folder(value: std::ffi::OsString) -> Result<PathBuf, String> {
    let path = PathBuf::from(&value);
    if value.is_empty() {
        return Err("folder path must not be empty".into());
    }
    if std::fs::metadata(&path).is_ok_and(|metadata| !metadata.is_dir()) {
        return Err("expected a folder path, but this path is not a directory".into());
    }
    Ok(path)
}

#[derive(Serialize)]
struct Row {
    #[serde(flatten)]
    session: SessionSummary,
    folder_status: FolderStatus,
}
#[derive(Serialize)]
struct Output {
    sessions: Vec<Row>,
    complete: bool,
    error: Option<String>,
    sources: Vec<SourceDiagnostic>,
}

async fn run(cli: Cli) -> Result<u8, Box<dyn std::error::Error>> {
    let env = Environment::capture()?;
    let config = config::resolve(&env, cli.config.as_deref(), &cli.codex_home)?;
    let command = cli.command.unwrap_or(Command::Browse {
        path: cli.path,
        fixture: None,
        include_archived: false,
    });
    let (fixture, include_archived) = match &command {
        Command::Browse {
            fixture,
            include_archived,
            ..
        }
        | Command::List {
            fixture,
            include_archived,
            ..
        } => (fixture, *include_archived),
    };
    let binary = if cli.codex_bin.components().count() > 1 {
        config::absolute_path(&cli.codex_bin, &env.cwd)?
    } else {
        cli.codex_bin
    };
    let specs = if let Some(path) = fixture {
        vec![SourceSpec::Fixture(config::absolute_path(path, &env.cwd)?)]
    } else {
        config
            .codex_sources
            .into_iter()
            .map(|source| SourceSpec::Codex {
                binary: binary.clone(),
                source,
            })
            .collect()
    };
    let mut browser = Browser::new(specs);
    browser.include_archived = include_archived;
    if let Command::Browse { path, fixture, .. } = &command {
        let state_path = config.state_dir.join("browser-state.json");
        let remembered = if fixture.is_none() && path.is_none() {
            match tracr::state::load(&state_path) {
                Ok(folder) => folder,
                Err(error) => {
                    browser.state_warning = Some(error);
                    None
                }
            }
        } else {
            None
        };
        let folder = match path {
            Some(path) => config::absolute_path(path, &env.cwd)?,
            None => remembered.unwrap_or_else(|| env.cwd.clone()),
        };
        browser.open_folder(folder);
        if fixture.is_none() {
            browser.state_path = Some(state_path);
        }
    }
    match command {
        Command::Browse { .. } => {
            tracr::tui::run(browser).await?;
            Ok(0)
        }
        Command::List { json, .. } => {
            let mut discovery = Discovery::new();
            discovery.refresh(&mut browser);
            while browser.loading() {
                let update = discovery
                    .rx
                    .recv()
                    .await
                    .ok_or("discovery stopped unexpectedly")?;
                browser.apply(update);
            }
            let sources = browser.diagnostics();
            let complete = sources.iter().all(|s| s.complete);
            let errors: Vec<_> = sources
                .iter()
                .filter(|s| !s.complete)
                .map(|s| {
                    format!(
                        "{}: {}",
                        s.source,
                        s.error.clone().unwrap_or_else(|| s.warnings.join("; "))
                    )
                })
                .collect();
            let sessions = browser.visible();
            // CLI has no interactive input to block; classify each unique folder once.
            let mut folders = std::collections::BTreeMap::new();
            for session in &sessions {
                if let Some(path) = &session.cwd {
                    folders
                        .entry(path.clone())
                        .or_insert_with(|| classify_folder(Some(path)));
                }
            }
            let output = Output {
                sessions: sessions
                    .into_iter()
                    .map(|session| Row {
                        folder_status: session
                            .cwd
                            .as_ref()
                            .and_then(|p| folders.get(p))
                            .cloned()
                            .unwrap_or(FolderStatus::Unknown),
                        session,
                    })
                    .collect(),
                complete,
                error: (!errors.is_empty()).then(|| errors.join("; ")),
                sources,
            };
            discovery.close(&mut browser).await;
            if json {
                println!("{}", serde_json::to_string_pretty(&output)?);
            } else {
                println!("TITLE\tDIRECTORY\tARCHIVED");
                for row in &output.sessions {
                    println!(
                        "{:?}\t{:?}\t{}",
                        row.session.title, row.session.cwd, row.session.archived
                    );
                }
                if let Some(error) = &output.error {
                    eprintln!(
                        "tracr: incomplete inventory: {}",
                        tracr::browser::safe_text(error)
                    );
                }
            }
            Ok(if complete { 0 } else { 1 })
        }
    }
}
#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    // Validate after global arguments have propagated from the parent command.
    // Clap's subcommand conflict check alone misses --codex-home before `list`.
    let fixture = matches!(
        &cli.command,
        Some(
            Command::Browse {
                fixture: Some(_),
                ..
            } | Command::List {
                fixture: Some(_),
                ..
            }
        )
    );
    let conflict = if cli.path.is_some() && cli.command.is_some() {
        Some("use either tracr PATH or tracr browse PATH, not a path before a subcommand")
    } else if fixture && !cli.codex_home.is_empty() {
        Some("--fixture cannot be combined with --codex-home")
    } else {
        None
    };
    if let Some(message) = conflict {
        use clap::CommandFactory;
        Cli::command()
            .error(clap::error::ErrorKind::ArgumentConflict, message)
            .exit();
    }
    match run(cli).await {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("tracr: {}", tracr::browser::safe_text(&error.to_string()));
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn folder_arguments_preserve_commands_and_boundaries() {
        for path in [
            ".",
            "../project",
            "/missing/project",
            "a b",
            "./list",
            "foo/../bar/",
        ] {
            assert_eq!(
                Cli::try_parse_from(["tracr", path]).unwrap().path,
                Some(path.into())
            );
            assert!(matches!(
                Cli::try_parse_from(["tracr", "browse", path])
                    .unwrap()
                    .command,
                Some(Command::Browse { path: Some(_), .. })
            ));
        }
        assert!(matches!(
            Cli::try_parse_from(["tracr", "list"]).unwrap().command,
            Some(Command::List { .. })
        ));
        assert_eq!(
            Cli::try_parse_from(["tracr", "--", "-folder"])
                .unwrap()
                .path,
            Some("-folder".into())
        );
        for args in [
            vec!["tracr", ""],
            vec!["tracr", "one", "two"],
            vec!["tracr", "list", "."],
            vec!["tracr", "Cargo.toml"],
        ] {
            assert!(Cli::try_parse_from(args).is_err());
        }
    }
}
