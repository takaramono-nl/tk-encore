pub mod campaigns;
pub mod lists;
pub mod subscribers;

use std::time::Duration;

use reqwest::{Client as HttpClient, Method, RequestBuilder, StatusCode};
use secrecy::{ExposeSecret, SecretString};
use serde::Serialize;
use serde::de::DeserializeOwned;
use url::Url;

use crate::config::{AuthMode, Config};
use crate::error::{EncoreError, Result};
use crate::model::Envelope;

const USER_AGENT: &str = concat!("encore/", env!("CARGO_PKG_VERSION"));
const MAX_GET_ATTEMPTS: u32 = 3;

enum Auth {
    Basic { user: String, token: SecretString },
    Token { user: String, token: SecretString },
}

pub struct Client {
    http: HttpClient,
    base: String,
    auth: Auth,
    timeout: Duration,
}

impl Client {
    pub fn new(cfg: &Config) -> Result<Self> {
        cfg.require_credentials()?;
        Self::build(
            &cfg.listmonk_url,
            &cfg.user,
            &cfg.token,
            cfg.auth,
            cfg.timeout,
            cfg.insecure,
        )
    }

    pub fn build(
        base: &str,
        user: &str,
        token: &SecretString,
        auth: AuthMode,
        timeout: Duration,
        insecure: bool,
    ) -> Result<Self> {
        let http = HttpClient::builder()
            .user_agent(USER_AGENT)
            .timeout(timeout)
            .danger_accept_invalid_certs(insecure)
            .build()?;

        let auth = match auth {
            AuthMode::Basic => Auth::Basic {
                user: user.to_string(),
                token: token.clone(),
            },
            AuthMode::Token => Auth::Token {
                user: user.to_string(),
                token: token.clone(),
            },
        };

        Ok(Self {
            http,
            base: base.trim_end_matches('/').to_string(),
            auth,
            timeout,
        })
    }

    /// Cheap authenticated call used by `encore init`.
    pub async fn probe(&self) -> Result<()> {
        let _: serde_json::Value = self
            .get_json("probe", "/api/lists", &[("per_page", "1".to_string())])
            .await?;
        Ok(())
    }

    fn endpoint(&self, path: &str, params: &[(&str, String)]) -> Result<String> {
        let mut url = Url::parse(&format!("{}{}", self.base, path))?;
        if !params.is_empty() {
            let mut q = url.query_pairs_mut();
            for (k, v) in params {
                q.append_pair(k, v);
            }
        }
        Ok(url.into())
    }

    fn request(&self, method: Method, url: &str) -> RequestBuilder {
        let rb = self.http.request(method, url).timeout(self.timeout);
        match &self.auth {
            Auth::Basic { user, token } => rb.basic_auth(user, Some(token.expose_secret())),
            Auth::Token { user, token } => rb.header(
                "Authorization",
                format!("token {user}:{}", token.expose_secret()),
            ),
        }
    }

    /// GET with a small retry loop. Writes intentionally do not retry: a
    /// duplicated POST /api/campaigns would create a second campaign.
    async fn get_json<T: DeserializeOwned>(
        &self,
        op: &str,
        path: &str,
        params: &[(&str, String)],
    ) -> Result<T> {
        let url = self.endpoint(path, params)?;
        let mut last: Option<EncoreError> = None;

        for attempt in 0..MAX_GET_ATTEMPTS {
            match self.request(Method::GET, &url).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp.text().await?;
                    if status.is_success() {
                        return decode(op, &text);
                    }
                    last = Some(api_error(op, status, &text));
                    if retryable(status) && attempt + 1 < MAX_GET_ATTEMPTS {
                        backoff(attempt).await;
                        continue;
                    }
                    break;
                }
                Err(e) => {
                    last = Some(EncoreError::Http(e));
                    if attempt + 1 < MAX_GET_ATTEMPTS {
                        backoff(attempt).await;
                        continue;
                    }
                    break;
                }
            }
        }

        Err(last.unwrap_or_else(|| EncoreError::Other(format!("{op}: no response"))))
    }

    async fn send_json<T: DeserializeOwned, B: Serialize>(
        &self,
        op: &str,
        method: Method,
        path: &str,
        params: &[(&str, String)],
        body: &B,
    ) -> Result<T> {
        let url = self.endpoint(path, params)?;
        let resp = self.request(method, &url).json(body).send().await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(api_error(op, status, &text));
        }
        decode(op, &text)
    }

    async fn send_ok<B: Serialize>(
        &self,
        op: &str,
        method: Method,
        path: &str,
        body: &B,
    ) -> Result<()> {
        let _: serde_json::Value = self.send_json(op, method, path, &[], body).await?;
        Ok(())
    }
}

fn decode<T: DeserializeOwned>(op: &str, text: &str) -> Result<T> {
    match serde_json::from_str::<Envelope<T>>(text) {
        Ok(e) => Ok(e.data),
        Err(e) => Err(EncoreError::InvalidData(format!(
            "{op}: {e}; body: {}",
            snippet(text)
        ))),
    }
}

fn api_error(op: &str, status: StatusCode, text: &str) -> EncoreError {
    EncoreError::Api {
        op: op.to_string(),
        status: status.as_u16(),
        body: snippet(text),
    }
}

fn snippet(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= 400 {
        return trimmed.to_string();
    }
    let cut: String = trimmed.chars().take(400).collect();
    format!("{cut}…")
}

fn retryable(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::TOO_MANY_REQUESTS
            | StatusCode::INTERNAL_SERVER_ERROR
            | StatusCode::BAD_GATEWAY
            | StatusCode::SERVICE_UNAVAILABLE
            | StatusCode::GATEWAY_TIMEOUT
    )
}

async fn backoff(attempt: u32) {
    let ms = 250u64 * 2u64.pow(attempt);
    tokio::time::sleep(Duration::from_millis(ms)).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_encodes_repeated_params() {
        let token = SecretString::from("t");
        let c = Client::build(
            "http://localhost:9000/",
            "u",
            &token,
            AuthMode::Basic,
            Duration::from_secs(5),
            false,
        )
        .unwrap();
        let url = c
            .endpoint(
                "/api/subscribers",
                &[("list_id", "5".into()), ("list_id", "6".into())],
            )
            .unwrap();
        assert_eq!(
            url,
            "http://localhost:9000/api/subscribers?list_id=5&list_id=6"
        );
    }

    #[test]
    fn snippet_shortens_long_bodies() {
        let body = "x".repeat(500);
        assert!(snippet(&body).chars().count() <= 401);
    }
}
