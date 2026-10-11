//! The two sign-ins, each a refresh token in a file only this user can read: one for audio
//! playback through librespot, one for browsing through the Web API.
//!
//! Not the Keychain: the binary is unsigned, so macOS would ask for the login password after
//! every rebuild or reinstall, and a background daemon would wait on that dialog unseen.

use crate::Result;
use crate::paths::Paths;
use std::fs::{self, File};
use std::io::{ErrorKind, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

/// The refresh token for librespot's audio session, if this computer is signed in.
pub fn audio(paths: &Paths) -> Result<Option<String>> {
    read(&paths.audio_credentials())
}

pub fn set_audio(paths: &Paths, token: &str) -> Result<()> {
    write(paths, &paths.audio_credentials(), token)
}

pub fn forget_audio(paths: &Paths) -> Result<()> {
    forget(&paths.audio_credentials())
}

/// The refresh token for the Web API, if this computer is signed in for browsing.
pub fn web(paths: &Paths) -> Result<Option<String>> {
    read(&paths.web_credentials())
}

pub fn set_web(paths: &Paths, token: &str) -> Result<()> {
    write(paths, &paths.web_credentials(), token)
}

pub fn forget_web(paths: &Paths) -> Result<()> {
    forget(&paths.web_credentials())
}

fn read(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(token) => Ok(Some(token.trim().to_owned()).filter(|token| !token.is_empty())),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("Cannot read the stored sign-in: {error}").into()),
    }
}

/// Replaces the stored token in one step, so a crash never leaves half of one.
fn write(paths: &Paths, path: &Path, token: &str) -> Result<()> {
    paths.prepare()?;
    let partial = path.with_extension("partial");
    let _ = fs::remove_file(&partial);
    let mut file = File::options()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&partial)?;
    file.write_all(token.as_bytes())?;
    file.sync_all()?;
    fs::set_permissions(&partial, fs::Permissions::from_mode(0o600))?;
    fs::rename(&partial, path)?;
    Ok(())
}

fn forget(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn paths(name: &str) -> Paths {
        let dir = PathBuf::from("/tmp").join(format!("hs-cred-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        Paths::at(dir).expect("paths")
    }

    #[test]
    fn a_token_is_stored_privately_replaced_and_forgotten() -> Result<()> {
        let paths = paths("store");
        assert_eq!(audio(&paths)?, None);

        set_audio(&paths, "first")?;
        set_audio(&paths, "second")?;
        assert_eq!(audio(&paths)?.as_deref(), Some("second"));
        let mode = fs::metadata(paths.audio_credentials())?
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        assert!(!paths.audio_credentials().with_extension("partial").exists());

        forget_audio(&paths)?;
        forget_audio(&paths)?;
        assert_eq!(audio(&paths)?, None);
        fs::remove_dir_all(paths.state())?;
        Ok(())
    }

    #[test]
    fn the_two_sign_ins_are_kept_apart() -> Result<()> {
        let paths = paths("apart");
        set_audio(&paths, "audio")?;
        set_web(&paths, "web")?;
        forget_audio(&paths)?;
        assert_eq!(audio(&paths)?, None);
        assert_eq!(web(&paths)?.as_deref(), Some("web"));
        fs::remove_dir_all(paths.state())?;
        Ok(())
    }

    #[test]
    fn an_empty_file_is_not_a_sign_in() -> Result<()> {
        let paths = paths("empty");
        paths.prepare()?;
        fs::write(paths.audio_credentials(), "\n")?;
        assert_eq!(audio(&paths)?, None);
        fs::remove_dir_all(paths.state())?;
        Ok(())
    }
}
