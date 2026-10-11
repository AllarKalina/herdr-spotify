//! The Spotify Web API, used only for browsing: playlists, their tracks, and search.
//!
//! Every request goes through `Api::get`, which is where the call budget is kept: one
//! request at a time, no automatic retries, and after a 429 nothing is sent until Spotify's
//! `Retry-After` has passed, even across daemon restarts. Player state never comes from here.

pub mod auth;
mod cache;
pub mod catalog;

use crate::config::Config;
use crate::credentials;
use crate::paths::Paths;
use crate::protocol::now_ms;
use serde_json::Value;
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(15);
/// What to wait after a 429 that names no `Retry-After`.
const DEFAULT_COOLDOWN_SECS: u64 = 30;
/// A token this close to expiry is renewed before use.
const EXPIRY_MARGIN: Duration = Duration::from_secs(30);

/// Where Spotify lives. Tests point these at a local stand-in.
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub api: String,
    pub accounts: String,
}

impl Endpoints {
    /// The real Spotify, unless the environment names a stand-in; end-to-end tests do.
    pub fn from_env() -> Self {
        let real = Self::default();
        Self {
            api: std::env::var("HERDR_SPOTIFY_API_URL").unwrap_or(real.api),
            accounts: std::env::var("HERDR_SPOTIFY_ACCOUNTS_URL").unwrap_or(real.accounts),
        }
    }
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            api: "https://api.spotify.com/v1".into(),
            accounts: "https://accounts.spotify.com".into(),
        }
    }
}

/// Why a request produced nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// No browsing sign-in on this computer, or Spotify revoked it.
    SignedOut,
    /// Spotify's rate limit; nothing is sent for this many more seconds.
    Limited {
        seconds: u64,
    },
    Failed(String),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SignedOut => {
                write!(
                    formatter,
                    "Browsing needs its own sign-in. Run: herdr-spotify login"
                )
            }
            Self::Limited { seconds } => {
                write!(
                    formatter,
                    "Spotify is rate limiting requests; trying again in {seconds}s"
                )
            }
            Self::Failed(problem) => write!(formatter, "{problem}"),
        }
    }
}

fn failed(error: impl std::fmt::Display) -> Failure {
    Failure::Failed(error.to_string())
}

pub struct Api {
    http: reqwest::Client,
    endpoints: Endpoints,
    paths: Paths,
    config: Config,
    access: Option<(String, Instant)>,
}

impl Api {
    pub fn new(paths: Paths, config: Config, endpoints: Endpoints) -> crate::Result<Self> {
        let http = reqwest::Client::builder().timeout(TIMEOUT).build()?;
        Ok(Self {
            http,
            endpoints,
            paths,
            config,
            access: None,
        })
    }

    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    /// One GET against the Web API. `path` starts with a slash.
    pub async fn get(&mut self, path: &str, query: &[(&str, &str)]) -> Result<Value, Failure> {
        if let Some(seconds) = self.cooldown_left() {
            return Err(Failure::Limited { seconds });
        }
        let mut response = self.send(path, query).await?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            // The token was refused, not the request: renew it and ask once more.
            self.access = None;
            response = self.send(path, query).await?;
        }
        let status = response.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let seconds = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.trim().parse().ok())
                .unwrap_or(DEFAULT_COOLDOWN_SECS);
            self.start_cooldown(seconds);
            log::warn!("Spotify rate limit on {path}: waiting {seconds}s");
            return Err(Failure::Limited { seconds });
        }
        let body = response.text().await.map_err(failed)?;
        if !status.is_success() {
            log::warn!("Spotify answered {status} for {path}");
            return Err(Failure::Failed(format!(
                "Spotify answered {status} for {path}"
            )));
        }
        serde_json::from_str(&body)
            .map_err(|_| failed(format!("Spotify sent an unreadable answer for {path}")))
    }

    async fn send(
        &mut self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<reqwest::Response, Failure> {
        let token = self.token().await?;
        log::info!("Web API GET {path}");
        self.http
            .get(format!("{}{path}", self.endpoints.api))
            .query(query)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|error| failed(format!("Cannot reach Spotify: {}", error.without_url())))
    }

    /// A valid access token, renewed from the stored refresh token when needed.
    async fn token(&mut self) -> Result<String, Failure> {
        if let Some((token, expires)) = &self.access
            && Instant::now() + EXPIRY_MARGIN < *expires
        {
            return Ok(token.clone());
        }
        if !self.config.has_app() {
            return Err(Failure::SignedOut);
        }
        let Some(refresh) = credentials::web(&self.paths).map_err(failed)? else {
            return Err(Failure::SignedOut);
        };
        let granted = auth::refresh(&self.http, &self.endpoints, &self.config, &refresh).await;
        let granted = match granted {
            Err(Failure::SignedOut) => {
                // Spotify revoked it; only a new login helps.
                let _ = credentials::forget_web(&self.paths);
                return Err(Failure::SignedOut);
            }
            other => other?,
        };
        if let Some(rotated) = granted.refresh_token.filter(|rotated| *rotated != refresh) {
            credentials::set_web(&self.paths, &rotated).map_err(failed)?;
        }
        let expires = Instant::now() + Duration::from_secs(granted.expires_in);
        self.access = Some((granted.access_token.clone(), expires));
        Ok(granted.access_token)
    }

    /// Seconds until requests may be sent again, if Spotify asked us to wait.
    pub fn cooldown_left(&self) -> Option<u64> {
        let until: u64 = std::fs::read_to_string(self.paths.web_cooldown())
            .ok()?
            .trim()
            .parse()
            .ok()?;
        let left = until.checked_sub(now_ms())?;
        (left > 0).then(|| left.div_ceil(1000))
    }

    /// Written to disk so a restarted daemon keeps waiting too.
    fn start_cooldown(&self, seconds: u64) {
        let until = now_ms().saturating_add(seconds.saturating_mul(1000));
        if let Err(error) = std::fs::write(self.paths.web_cooldown(), until.to_string()) {
            log::error!("cannot record the rate-limit cooldown: {error}");
        }
    }
}
