use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum EncoreError {
    #[error("listmonk API {status} on {op}: {body}")]
    Api {
        op: String,
        status: u16,
        body: String,
    },

    #[error("source campaign {0} is {1}; let it finish instead of resending")]
    CampaignInFlight(i64, String),

    #[error("source campaign {0} has status {1}; only finished campaigns can be resent")]
    CampaignNotFinished(i64, String),

    #[error("cohort size {got} does not match expected {expected}")]
    SizeMismatch { got: i64, expected: i64 },

    #[error("another encore process holds {0}")]
    Locked(PathBuf),

    #[error("cohort {0} not found")]
    CohortNotFound(i64),

    #[error("campaign {0} not found")]
    CampaignNotFound(i64),

    #[error("cohort {0} is {1}; use --resume to continue it")]
    CohortNotResumable(i64, String),

    #[error("send did not finish cleanly: {0}")]
    PartialSend(String),

    #[error("refusing: {0}")]
    Preflight(String),

    #[error("invalid upstream data: {0}")]
    InvalidData(String),

    #[error("config error: {0}")]
    Config(String),

    #[error(transparent)]
    Db(#[from] sqlx::Error),

    #[error(transparent)]
    Migrate(#[from] sqlx::migrate::MigrateError),

    #[error(transparent)]
    Http(#[from] reqwest::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error(transparent)]
    TomlDe(#[from] toml::de::Error),

    #[error(transparent)]
    TomlSer(#[from] toml::ser::Error),

    #[error(transparent)]
    Url(#[from] url::ParseError),

    #[error("{0}")]
    Other(String),
}

impl EncoreError {
    /// Exit codes are part of the CLI contract:
    /// 0 ok, 1 unexpected, 2 preflight refusal, 4 partial send, 5 lock held.
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::CampaignInFlight(..)
            | Self::CampaignNotFinished(..)
            | Self::SizeMismatch { .. }
            | Self::Preflight(_) => 2,
            Self::PartialSend(_) => 4,
            Self::Locked(_) => 5,
            _ => 1,
        }
    }
}

pub type Result<T> = std::result::Result<T, EncoreError>;
