use crate::api::Client;
use crate::config::Config;
use crate::error::Result;
use crate::lock::LockGuard;
use crate::store::Store;

/// Shared runtime context. The optional lock is held for the whole process
/// lifetime by `send`, `cohort` and `close`.
pub struct App {
    pub cfg: Config,
    pub api: Client,
    pub store: Store,
    _lock: Option<LockGuard>,
}

impl App {
    pub async fn load(cfg: Config, write: bool) -> Result<Self> {
        cfg.require_credentials()?;
        let api = Client::new(&cfg)?;
        let lock = if write {
            Some(LockGuard::acquire(&cfg.lock_path)?)
        } else {
            None
        };
        let store = Store::open(&cfg.db_path).await?;

        Ok(Self {
            cfg,
            api,
            store,
            _lock: lock,
        })
    }
}
