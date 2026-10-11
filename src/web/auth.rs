//! Signing in for browsing: OAuth authorization code with PKCE against our own developer
//! app. No client secret exists; the refresh token is kept by `credentials`.

use super::{Endpoints, Failure, failed};
use crate::config::Config;
use crate::{Result, credentials, paths::Paths};
use serde::Deserialize;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::process::Command;

/// Read-only access to what the user can play. Nothing here can change the account.
const SCOPES: &str = "playlist-read-private playlist-read-collaborative user-library-read";
/// PKCE verifier and state, as URL-safe characters.
const RANDOM_LENGTH: usize = 64;
const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

#[derive(Debug, Deserialize)]
pub struct Granted {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default = "an_hour")]
    pub expires_in: u64,
}

fn an_hour() -> u64 {
    3600
}

/// A new access token from the refresh token. `SignedOut` when Spotify no longer honours it.
pub async fn refresh(
    http: &reqwest::Client,
    endpoints: &Endpoints,
    config: &Config,
    refresh_token: &str,
) -> std::result::Result<Granted, Failure> {
    let form = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", config.client_id.as_str()),
    ];
    token_request(http, endpoints, &form).await
}

async fn token_request(
    http: &reqwest::Client,
    endpoints: &Endpoints,
    form: &[(&str, &str)],
) -> std::result::Result<Granted, Failure> {
    let response = http
        .post(format!("{}/api/token", endpoints.accounts))
        .form(form)
        .send()
        .await
        .map_err(|error| failed(format!("Cannot reach Spotify: {}", error.without_url())))?;
    let status = response.status();
    let body = response.text().await.map_err(failed)?;
    if status.is_success() {
        return serde_json::from_str(&body)
            .map_err(|_| failed("Spotify sent an unreadable sign-in answer"));
    }
    let error = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|body| body["error"].as_str().map(str::to_owned))
        .unwrap_or_default();
    if error == "invalid_grant" {
        return Err(Failure::SignedOut);
    }
    Err(failed(format!(
        "Spotify refused the sign-in ({status} {error})"
    )))
}

/// Opens the browser, waits for the redirect, and stores the refresh token.
pub fn login(paths: &Paths, config: &Config) -> Result<()> {
    let (address, path) = listen_target(&config.redirect_uri)?;
    let listener = TcpListener::bind(&address)
        .map_err(|error| format!("Cannot listen on {address} for the sign-in redirect: {error}"))?;
    let verifier = random()?;
    let state = random()?;
    let url = format!(
        "{}/authorize?response_type=code&client_id={}&scope={}&redirect_uri={}&state={state}\
         &code_challenge_method=S256&code_challenge={}",
        Endpoints::default().accounts,
        encode(&config.client_id),
        encode(SCOPES),
        encode(&config.redirect_uri),
        challenge(&verifier),
    );
    Command::new("open").arg(&url).status()?;
    let code = wait_for_code(&listener, &path, &state)?;

    let form = [
        ("grant_type", "authorization_code"),
        ("code", code.as_str()),
        ("redirect_uri", config.redirect_uri.as_str()),
        ("client_id", config.client_id.as_str()),
        ("code_verifier", verifier.as_str()),
    ];
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let http = reqwest::Client::new();
    let granted = runtime
        .block_on(token_request(&http, &Endpoints::default(), &form))
        .map_err(|failure| failure.to_string())?;
    let refresh_token = granted
        .refresh_token
        .ok_or("Spotify returned no refresh token")?;
    credentials::set_web(paths, &refresh_token)
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
            _ if param("state").as_deref() != Some(state) => {
                Err("the reply did not match the request".to_owned())
            }
            (Some(code), _) => Ok(code),
            (None, error) => Err(error.unwrap_or_else(|| "Spotify sent no code".into())),
        };
        let page = match &outcome {
            Ok(_) => "Herdr Spotify: signed in for browsing. You can close this tab.".to_owned(),
            Err(error) => format!("Herdr Spotify: sign-in failed: {error}"),
        };
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{page}",
            page.len()
        )?;
        return outcome.map_err(|error| format!("Sign-in failed: {error}").into());
    }
}

/// Splits `http://127.0.0.1:3000/some/path` into `127.0.0.1:3000` and `/some/path`.
fn listen_target(redirect_uri: &str) -> Result<(String, String)> {
    let rest = redirect_uri
        .strip_prefix("http://127.0.0.1:")
        .ok_or("redirect_uri must look like http://127.0.0.1:<port>/<path>")?;
    let (port, path) = rest.split_once('/').unwrap_or((rest, ""));
    if port.parse::<u16>().is_err() {
        return Err("redirect_uri has no valid port".into());
    }
    Ok((format!("127.0.0.1:{port}"), format!("/{path}")))
}

/// Random URL-safe characters from the system's generator.
fn random() -> Result<String> {
    let mut bytes = [0u8; RANDOM_LENGTH];
    std::io::Read::read_exact(&mut std::fs::File::open("/dev/urandom")?, &mut bytes)?;
    Ok(bytes
        .iter()
        .map(|byte| char::from(ALPHABET[usize::from(byte & 63)]))
        .collect())
}

/// The PKCE challenge: the verifier's SHA-256, base64url without padding.
fn challenge(verifier: &str) -> String {
    let digest = <sha2::Sha256 as sha2::Digest>::digest(verifier.as_bytes());
    let mut encoded = String::new();
    for chunk in digest.chunks(3) {
        let bits = chunk
            .iter()
            .fold(0u32, |bits, byte| (bits << 8) | u32::from(*byte))
            << (8 * (3 - chunk.len()));
        for index in 0..=chunk.len() {
            let sextet = (bits >> (18 - 6 * index)) & 63;
            encoded.push(char::from(ALPHABET[sextet as usize]));
        }
    }
    encoded
}

fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                char::from(byte).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_challenge_matches_the_published_pkce_example() {
        // RFC 7636, appendix B.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            challenge(verifier),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn random_values_are_url_safe_and_differ() -> Result<()> {
        let (one, two) = (random()?, random()?);
        assert_eq!(one.len(), RANDOM_LENGTH);
        assert!(one.bytes().all(|byte| ALPHABET.contains(&byte)));
        assert_ne!(one, two);
        Ok(())
    }

    #[test]
    fn only_a_loopback_redirect_is_accepted() -> Result<()> {
        let (address, path) = listen_target("http://127.0.0.1:3000/v1/cli/auth/callback/public")?;
        assert_eq!(
            (address.as_str(), path.as_str()),
            ("127.0.0.1:3000", "/v1/cli/auth/callback/public")
        );
        assert_eq!(listen_target("http://127.0.0.1:8898")?.1, "/");
        assert!(listen_target("http://localhost:3000/callback").is_err());
        assert!(listen_target("https://example.com/callback").is_err());
        assert!(listen_target("http://127.0.0.1:port/callback").is_err());
        Ok(())
    }

    #[test]
    fn scopes_and_uris_are_percent_encoded() {
        assert_eq!(encode("a b:/~"), "a%20b%3A%2F~");
    }
}
