use crate::domain::{ProviderId, SourceId};
use serde::Deserialize;
use std::path::{Component, Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("cannot read configuration {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid TOML configuration {path}: {source}")]
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("home directory is unavailable; set HOME or configure explicit paths")]
    MissingHome,
    #[error("path must not be empty")]
    EmptyPath,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    #[serde(default)]
    codex_homes: Vec<PathBuf>,
}

/// Explicit environment snapshot keeps configuration tests independent of process globals.
#[derive(Debug, Clone)]
pub struct Environment {
    pub cwd: PathBuf,
    pub home: Option<PathBuf>,
    pub codex_home: Option<PathBuf>,
    pub config_home: Option<PathBuf>,
    pub state_home: Option<PathBuf>,
    pub cache_home: Option<PathBuf>,
}

impl Environment {
    pub fn capture() -> std::io::Result<Self> {
        fn variable(key: &str) -> Option<PathBuf> {
            std::env::var_os(key)
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
        }
        Ok(Self {
            cwd: std::env::current_dir()?,
            home: dirs::home_dir(),
            codex_home: variable("CODEX_HOME"),
            config_home: variable("XDG_CONFIG_HOME"),
            state_home: variable("XDG_STATE_HOME"),
            cache_home: variable("XDG_CACHE_HOME"),
        })
    }

    fn xdg(&self, value: &Option<PathBuf>, fallback: &str) -> Result<PathBuf, ConfigError> {
        if let Some(path) = value.as_ref().filter(|p| p.is_absolute()) {
            return Ok(path.clone());
        }
        Ok(self
            .home
            .as_ref()
            .ok_or(ConfigError::MissingHome)?
            .join(fallback))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceConfig {
    pub id: SourceId,
    pub home: PathBuf,
}

#[derive(Debug)]
pub struct ResolvedConfig {
    pub codex_sources: Vec<SourceConfig>,
    pub state_dir: PathBuf,
    pub cache_dir: PathBuf,
}

/// Lexical normalization deliberately does not resolve symlinks or require existence.
pub fn absolute_path(path: &Path, base: &Path) -> Result<PathBuf, ConfigError> {
    if path.as_os_str().is_empty() {
        return Err(ConfigError::EmptyPath);
    }
    let joined = if path.is_absolute() {
        path.to_owned()
    } else {
        base.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    Ok(normalized)
}

pub fn source_id(provider: &ProviderId, home: &Path) -> SourceId {
    // Linux bytes are preserved; lossy Unicode conversion could collide.
    use std::os::unix::ffi::OsStrExt;
    let hex: String = home
        .as_os_str()
        .as_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    SourceId(format!("{}:{hex}", provider.0))
}

pub fn resolve(
    env: &Environment,
    explicit: Option<&Path>,
    cli_homes: &[PathBuf],
) -> Result<ResolvedConfig, ConfigError> {
    let config_path = match explicit {
        Some(path) => absolute_path(path, &env.cwd)?,
        None => env
            .xdg(&env.config_home, ".config")?
            .join("tracr/config.toml"),
    };
    let file = match std::fs::read_to_string(&config_path) {
        Ok(text) => toml::from_str::<FileConfig>(&text).map_err(|source| ConfigError::Parse {
            path: config_path.clone(),
            source,
        })?,
        Err(error) if explicit.is_none() && error.kind() == std::io::ErrorKind::NotFound => {
            FileConfig::default()
        }
        Err(source) => {
            return Err(ConfigError::Read {
                path: config_path,
                source,
            });
        }
    };
    let (homes, base) = if !cli_homes.is_empty() {
        (cli_homes.to_vec(), env.cwd.clone())
    } else if !file.codex_homes.is_empty() {
        (
            file.codex_homes,
            config_path
                .parent()
                .expect("absolute config has a parent")
                .to_owned(),
        )
    } else if let Some(path) = &env.codex_home {
        (vec![path.clone()], env.cwd.clone())
    } else {
        (
            vec![
                env.home
                    .as_ref()
                    .ok_or(ConfigError::MissingHome)?
                    .join(".codex"),
            ],
            env.cwd.clone(),
        )
    };
    let mut codex_sources: Vec<SourceConfig> = Vec::new();
    for path in homes {
        let home = absolute_path(&path, &base)?;
        let id = source_id(&ProviderId("codex".into()), &home);
        if !codex_sources.iter().any(|s| s.id == id) {
            codex_sources.push(SourceConfig { id, home });
        }
    }
    Ok(ResolvedConfig {
        codex_sources,
        state_dir: env.xdg(&env.state_home, ".local/state")?.join("tracr"),
        cache_dir: env.xdg(&env.cache_home, ".cache")?.join("tracr"),
    })
}
