//! Signing in: first for audio playback, then for browsing.
//!
//! librespot only accepts a sign-in made through its own Spotify client; a token from our own
//! developer app is refused. The refresh token is kept by `credentials`.

use crate::config::{self, Config};
use crate::paths::Paths;
use crate::{Result, client, credentials, web};
use librespot_core::config::SessionConfig;
use librespot_oauth::{OAuthClient, OAuthClientBuilder};

/// Fixed by librespot's client registration with Spotify.
const REDIRECT_URI: &str = "http://127.0.0.1:8898/login";
const SCOPES: [&str; 1] = ["streaming"];

pub fn oauth_client(client_id: &str, open_browser: bool) -> Result<OAuthClient> {
    let builder = OAuthClientBuilder::new(client_id, REDIRECT_URI, SCOPES.to_vec());
    let builder = if open_browser {
        builder.open_in_browser()
    } else {
        builder
    };
    builder
        .build()
        .map_err(|error| format!("Sign-in setup failed: {error}").into())
}

pub fn run(paths: &Paths) -> Result<()> {
    println!("Opening Spotify in your browser to sign in for audio playback.");
    let token = oauth_client(&SessionConfig::default().client_id, true)?
        .get_access_token()
        .map_err(|error| format!("Sign-in failed: {error}"))?;
    if token.refresh_token.is_empty() {
        return Err("Spotify returned no refresh token; nothing was saved".into());
    }
    credentials::set_audio(paths, &token.refresh_token)?;
    println!("Signed in. Audio playback is ready on this computer.");

    let config = Config::load()?;
    if config.has_app() {
        println!("Opening Spotify once more, to sign in for browsing playlists and search.");
        web::auth::login(paths, &config)?;
        println!("Signed in. Playlists and search are ready.");
    } else {
        println!(
            "Playlists and search need a Spotify developer app: set client_id and redirect_uri in {}",
            config::path()?.display()
        );
    }
    // A running daemon still holds the old sessions, or none; the next client starts anew.
    client::stop(paths)?;
    Ok(())
}
