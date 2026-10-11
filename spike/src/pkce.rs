//! OAuth authorization code flow with PKCE against our own Spotify developer app.
//! The token stays in memory; the spike persists nothing.

use super::Result;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::PathBuf;

const SCOPES: &str = "streaming user-read-private user-read-email playlist-read-private \
                      playlist-read-collaborative user-library-read";

#[derive(Deserialize)]
pub struct Config {
    pub client_id: String,
    pub redirect_uri: String,
}

impl Config {
    pub fn load() -> Result<Self> {
        let dir = std::env::var_os("HERDR_PLUGIN_CONFIG_DIR").map_or_else(
            || {
                PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
                    .join(".config/herdr/plugins/config/personal.spotify")
            },
            PathBuf::from,
        );
        let path = dir.join("config.toml");
        let config: Self = toml::from_str(&std::fs::read_to_string(&path)?)?;
        if config.client_id.trim().is_empty() {
            return Err(format!("Set client_id in {}", path.display()).into());
        }
        Ok(config)
    }
}

/// Returns an access token. Opens the browser and waits for the redirect.
pub fn login(config: &Config) -> Result<String> {
    let verifier = random_token()?;
    let state = random_token()?;
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));

    let (address, path) = listen_target(&config.redirect_uri)?;
    let listener = TcpListener::bind(&address)
        .map_err(|error| format!("Cannot listen on {address}: {error}"))?;

    let url = format!(
        "https://accounts.spotify.com/authorize?response_type=code&client_id={}&scope={}\
         &redirect_uri={}&state={state}&code_challenge_method=S256&code_challenge={challenge}",
        encode(&config.client_id),
        encode(SCOPES),
        encode(&config.redirect_uri),
    );
    std::process::Command::new("open").arg(&url).status()?;
    println!("waiting for the browser redirect on {address}{path}");

    let code = wait_for_code(&listener, &path, &state)?;
    let mut response = ureq::post("https://accounts.spotify.com/api/token")
        .config()
        .http_status_as_error(false)
        .build()
        .send_form([
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", config.redirect_uri.as_str()),
            ("client_id", config.client_id.as_str()),
            ("code_verifier", verifier.as_str()),
        ])?;
    let status = response.status();
    let body: serde_json::Value = response.body_mut().read_json()?;
    let Some(token) = body["access_token"].as_str() else {
        return Err(format!(
            "Token exchange failed ({status}): {} {}",
            body["error"].as_str().unwrap_or("?"),
            body["error_description"].as_str().unwrap_or("")
        )
        .into());
    };
    println!(
        "RESULT token: ok, scopes granted: {}, refresh token: {}",
        body["scope"].as_str().unwrap_or("?"),
        if body["refresh_token"].is_string() { "yes" } else { "no" }
    );
    Ok(token.to_owned())
}

fn wait_for_code(listener: &TcpListener, path: &str, state: &str) -> Result<String> {
    loop {
        let (mut stream, _) = listener.accept()?;
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line)?;
        let target = line.split_whitespace().nth(1).unwrap_or_default();
        let (request_path, query) = target.split_once('?').unwrap_or((target, ""));
        if request_path != path {
            stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n")?;
            continue;
        }
        let param = |name: &str| {
            query
                .split('&')
                .filter_map(|pair| pair.split_once('='))
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.to_owned())
        };
        let outcome = match (param("code"), param("error")) {
            _ if param("state").as_deref() != Some(state) => Err("state mismatch".to_owned()),
            (Some(code), _) => Ok(code),
            (None, error) => Err(error.unwrap_or_else(|| "no code in redirect".into())),
        };
        let page = match &outcome {
            Ok(_) => "Herdr Spotify: signed in. You can close this tab.".to_owned(),
            Err(error) => format!("Herdr Spotify: sign-in failed: {error}"),
        };
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{page}",
            page.len()
        )?;
        return outcome.map_err(Into::into);
    }
}

/// Splits `http://127.0.0.1:3000/some/path` into `127.0.0.1:3000` and `/some/path`.
fn listen_target(redirect_uri: &str) -> Result<(String, String)> {
    let rest = redirect_uri
        .strip_prefix("http://")
        .ok_or("redirect_uri must start with http://")?;
    let (address, path) = rest.split_once('/').unwrap_or((rest, ""));
    if !address.starts_with("127.0.0.1:") {
        return Err("redirect_uri must be http://127.0.0.1:<port>/...".into());
    }
    Ok((address.to_owned(), format!("/{path}")))
}

fn random_token() -> Result<String> {
    let mut bytes = [0u8; 48];
    getrandom::fill(&mut bytes).map_err(|error| format!("no randomness: {error}"))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}
