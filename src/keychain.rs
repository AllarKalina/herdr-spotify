//! Credentials in the macOS login Keychain. Nothing secret is written anywhere else.

use crate::Result;
use security_framework::passwords;

const SERVICE: &str = "herdr-spotify";
/// `errSecItemNotFound`
const NOT_FOUND: i32 = -25300;

/// The refresh token for librespot's audio session.
pub const AUDIO: &str = "audio-refresh-token";

/// Tests name their own service so they can never read or replace a real sign-in.
fn service() -> String {
    std::env::var("HERDR_SPOTIFY_KEYCHAIN").unwrap_or_else(|_| SERVICE.into())
}

pub fn get(account: &str) -> Result<Option<String>> {
    match passwords::get_generic_password(&service(), account) {
        Ok(secret) => Ok(Some(String::from_utf8(secret)?)),
        Err(error) if error.code() == NOT_FOUND => Ok(None),
        Err(error) => Err(format!("Keychain read failed: {error}").into()),
    }
}

pub fn set(account: &str, secret: &str) -> Result<()> {
    passwords::set_generic_password(&service(), account, secret.as_bytes())
        .map_err(|error| format!("Keychain write failed: {error}").into())
}

pub fn delete(account: &str) -> Result<()> {
    match passwords::delete_generic_password(&service(), account) {
        Ok(()) => Ok(()),
        Err(error) if error.code() == NOT_FOUND => Ok(()),
        Err(error) => Err(format!("Keychain delete failed: {error}").into()),
    }
}
