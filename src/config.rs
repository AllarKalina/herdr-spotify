//! The plugin's settings file, `config.toml` in Herdr's config directory for this plugin.

use crate::Result;
use serde::Deserialize;
use std::path::PathBuf;

const PLUGIN_ID: &str = "personal.spotify";

/// The Spotify developer app used for browsing. Neither value is a secret.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub client_id: String,
    /// Must match a redirect URI registered on the developer app, exactly.
    #[serde(default)]
    pub redirect_uri: String,
}

impl Config {
    /// Reads the file; a missing file is an empty config, since browsing is optional.
    pub fn load() -> Result<Self> {
        let path = path()?;
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text)
                .map_err(|error| format!("Cannot read {}: {error}", path.display()).into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(format!("Cannot read {}: {error}", path.display()).into()),
        }
    }

    /// True when a developer app is configured, which browsing needs.
    pub fn has_app(&self) -> bool {
        !self.client_id.trim().is_empty() && !self.redirect_uri.trim().is_empty()
    }
}

/// Herdr passes the directory to plugin commands; the same place is used when run by hand.
pub fn path() -> Result<PathBuf> {
    let dir = match std::env::var_os("HERDR_PLUGIN_CONFIG_DIR") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => PathBuf::from(std::env::var_os("HOME").ok_or("HOME is not set")?)
            .join(".config/herdr/plugins/config")
            .join(PLUGIN_ID),
    };
    Ok(dir.join("config.toml"))
}
