//! Where the daemon keeps its files: one directory, private to the user.

use crate::Result;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const PLUGIN_ID: &str = "personal.spotify";
/// macOS refuses Unix socket paths longer than this, terminator included.
const SOCKET_PATH_LIMIT: usize = 104;

#[derive(Debug, Clone)]
pub struct Paths {
    state: PathBuf,
}

impl Paths {
    /// Herdr's state directory for this plugin. Herdr passes it to plugin commands; the same
    /// location is used when the binary is run by hand, so both find the same daemon.
    pub fn from_env() -> Result<Self> {
        let state = match std::env::var_os("HERDR_PLUGIN_STATE_DIR") {
            Some(dir) if !dir.is_empty() => PathBuf::from(dir),
            _ => PathBuf::from(std::env::var_os("HOME").ok_or("HOME is not set")?)
                .join(".local/state/herdr/plugins")
                .join(PLUGIN_ID),
        };
        Self::at(state)
    }

    pub fn at(state: PathBuf) -> Result<Self> {
        let paths = Self { state };
        let socket = paths.socket();
        if socket.as_os_str().len() >= SOCKET_PATH_LIMIT {
            return Err(format!(
                "State directory path is too long for a socket: {}",
                socket.display()
            )
            .into());
        }
        Ok(paths)
    }

    /// Creates the state directory, private to the user.
    pub fn prepare(&self) -> Result<()> {
        fs::create_dir_all(&self.state)?;
        fs::set_permissions(&self.state, fs::Permissions::from_mode(0o700))?;
        Ok(())
    }

    pub fn state(&self) -> &Path {
        &self.state
    }

    pub fn socket(&self) -> PathBuf {
        self.state.join("daemon.sock")
    }

    /// Held with an exclusive file lock for as long as a daemon runs. Never deleted: removing a
    /// locked file would let a second daemon lock a fresh one.
    pub fn lock(&self) -> PathBuf {
        self.state.join("daemon.lock")
    }

    pub fn log(&self) -> PathBuf {
        self.state.join("daemon.log")
    }

    /// The audio sign-in; see `credentials`.
    pub fn audio_credentials(&self) -> PathBuf {
        self.state.join("audio-credentials")
    }

    /// Cover images of recently played tracks.
    pub fn covers(&self) -> PathBuf {
        self.state.join("covers")
    }

    /// The identity this computer presents to Spotify, kept so restarts are the same device.
    pub fn device_id(&self) -> PathBuf {
        self.state.join("device-id")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_a_state_directory_too_deep_for_a_socket() {
        assert!(Paths::at(PathBuf::from("/tmp/hs")).is_ok());
        assert!(Paths::at(PathBuf::from(format!("/tmp/{}", "x".repeat(100)))).is_err());
    }
}
