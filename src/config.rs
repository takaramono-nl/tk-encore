use std::path::{Path, PathBuf};
use std::time::Duration;

use secrecy::SecretString;
use serde::{Deserialize, Serialize};

use crate::error::{EncoreError, Result};

pub const DEFAULT_URL: &str = "http://localhost:9000";
pub const DEFAULT_TIMEOUT_SECS: u64 = 30;
pub const DEFAULT_SEND_TIMEOUT_SECS: u64 = 3600;

fn default_url() -> String {
    DEFAULT_URL.to_string()
}

fn default_timeout() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

fn default_send_timeout() -> u64 {
    DEFAULT_SEND_TIMEOUT_SECS
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum AuthMode {
    /// HTTP Basic auth (`-u user:token`).
    #[default]
    Basic,
    /// `Authorization: token user:token`.
    Token,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListmonkFile {
    #[serde(default = "default_url")]
    pub url: String,
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub auth: AuthMode,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default = "default_send_timeout")]
    pub send_timeout_secs: u64,
    #[serde(default)]
    pub insecure: bool,
}

impl Default for ListmonkFile {
    fn default() -> Self {
        Self {
            url: default_url(),
            user: String::new(),
            token: String::new(),
            auth: AuthMode::default(),
            timeout_secs: DEFAULT_TIMEOUT_SECS,
            send_timeout_secs: DEFAULT_SEND_TIMEOUT_SECS,
            insecure: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FileConfig {
    #[serde(default)]
    pub listmonk: ListmonkFile,
    #[serde(default)]
    pub db: Option<PathBuf>,
}

/// Values assembled from CLI flags, environment variables and the config file.
#[derive(Debug, Clone, Default)]
pub struct Overrides {
    pub config: Option<PathBuf>,
    pub url: Option<String>,
    pub user: Option<String>,
    pub token: Option<String>,
    pub auth: Option<AuthMode>,
    pub db: Option<PathBuf>,
    pub insecure: bool,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub listmonk_url: String,
    pub user: String,
    pub token: SecretString,
    pub auth: AuthMode,
    pub db_path: PathBuf,
    pub lock_path: PathBuf,
    pub config_path: PathBuf,
    pub timeout: Duration,
    pub send_timeout: Duration,
    pub insecure: bool,
}

impl Config {
    /// Resolution order: CLI flag > environment > config file > built-in default.
    pub fn resolve(o: &Overrides) -> Result<Self> {
        let config_path = o
            .config
            .clone()
            .or_else(|| env_path("ENCORE_CONFIG"))
            .unwrap_or_else(default_config_path);

        let file = if config_path.exists() {
            let raw = std::fs::read_to_string(&config_path)?;
            toml::from_str::<FileConfig>(&raw)?
        } else {
            FileConfig::default()
        };

        let url = o
            .url
            .clone()
            .or_else(|| env_string("ENCORE_LISTMONK_URL"))
            .unwrap_or_else(|| file.listmonk.url.clone());

        let user = o
            .user
            .clone()
            .or_else(|| env_string("ENCORE_LISTMONK_USER"))
            .unwrap_or_else(|| file.listmonk.user.clone());

        let token = o
            .token
            .clone()
            .or_else(|| env_string("ENCORE_LISTMONK_TOKEN"))
            .unwrap_or_else(|| file.listmonk.token.clone());

        let auth = o
            .auth
            .or_else(|| env_string("ENCORE_AUTH").and_then(|v| parse_auth(&v)))
            .unwrap_or(file.listmonk.auth);

        let db_path =
            o.db.clone()
                .or_else(|| env_path("ENCORE_DB"))
                .or_else(|| file.db.clone())
                .unwrap_or_else(default_db_path);

        let timeout_secs = env_string("ENCORE_TIMEOUT_SECS")
            .and_then(|v| v.parse().ok())
            .unwrap_or(file.listmonk.timeout_secs)
            .max(1);

        let send_timeout_secs = env_string("ENCORE_SEND_TIMEOUT_SECS")
            .and_then(|v| v.parse().ok())
            .unwrap_or(file.listmonk.send_timeout_secs)
            .max(1);

        let insecure = o.insecure
            || env_string("ENCORE_INSECURE")
                .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
                .unwrap_or(false)
            || file.listmonk.insecure;

        let lock_path = db_path.with_extension("lock");

        Ok(Self {
            listmonk_url: url.trim_end_matches('/').to_string(),
            user,
            token: SecretString::from(token),
            auth,
            db_path,
            lock_path,
            config_path,
            timeout: Duration::from_secs(timeout_secs),
            send_timeout: Duration::from_secs(send_timeout_secs),
            insecure,
        })
    }

    pub fn require_credentials(&self) -> Result<()> {
        use secrecy::ExposeSecret;
        if self.user.trim().is_empty() || self.token.expose_secret().trim().is_empty() {
            return Err(EncoreError::Config(
                "listmonk credentials are missing; set ENCORE_LISTMONK_USER and \
                 ENCORE_LISTMONK_TOKEN, pass --user/--token, or run `encore init`"
                    .into(),
            ));
        }
        Ok(())
    }

    pub fn to_file(&self) -> FileConfig {
        use secrecy::ExposeSecret;
        FileConfig {
            listmonk: ListmonkFile {
                url: self.listmonk_url.clone(),
                user: self.user.clone(),
                token: self.token.expose_secret().to_string(),
                auth: self.auth,
                timeout_secs: self.timeout.as_secs(),
                send_timeout_secs: self.send_timeout.as_secs(),
                insecure: self.insecure,
            },
            db: Some(self.db_path.clone()),
        }
    }

    pub fn write(&self) -> Result<()> {
        if let Some(parent) = self
            .config_path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)?;
        }
        let raw = toml::to_string_pretty(&self.to_file())?;
        std::fs::write(&self.config_path, raw)?;
        restrict_permissions(&self.config_path)?;
        Ok(())
    }
}

pub fn default_config_path() -> PathBuf {
    xdg_dir("XDG_CONFIG_HOME", ".config")
        .join("encore")
        .join("config.toml")
}

pub fn default_db_path() -> PathBuf {
    xdg_dir("XDG_DATA_HOME", ".local/share")
        .join("encore")
        .join("encore.db")
}

fn xdg_dir(env: &str, fallback: &str) -> PathBuf {
    if let Some(v) = env_path(env) {
        return v;
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(fallback)
}

fn env_string(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn env_path(name: &str) -> Option<PathBuf> {
    env_string(name).map(PathBuf::from)
}

fn parse_auth(v: &str) -> Option<AuthMode> {
    match v.trim().to_ascii_lowercase().as_str() {
        "basic" => Some(AuthMode::Basic),
        "token" => Some(AuthMode::Token),
        _ => None,
    }
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_mode_parses() {
        assert_eq!(parse_auth("BASIC"), Some(AuthMode::Basic));
        assert_eq!(parse_auth("token"), Some(AuthMode::Token));
        assert_eq!(parse_auth("nope"), None);
    }
}
