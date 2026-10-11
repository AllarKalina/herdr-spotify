//! A private state directory per test, so tests never meet a real daemon or a real sign-in.

#![allow(dead_code)]

pub mod fake_spotify;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

static NEXT: AtomicU32 = AtomicU32::new(0);

pub struct Sandbox {
    pub dir: PathBuf,
    idle_secs: Option<u32>,
    spotify: Option<String>,
}

impl Sandbox {
    pub fn new() -> Self {
        // Under /tmp, not the long per-user temp directory: socket paths are length-limited.
        let name = format!(
            "hs-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        );
        let dir = PathBuf::from("/tmp").join(&name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create sandbox");
        Self {
            dir,
            idle_secs: None,
            spotify: None,
        }
    }

    pub fn idle_after(mut self, seconds: u32) -> Self {
        self.idle_secs = Some(seconds);
        self
    }

    /// Signs the sandbox in for browsing against a stand-in Spotify at `url`.
    pub fn browsing(mut self, url: &str) -> Self {
        std::fs::create_dir_all(self.dir.join("config")).expect("config dir");
        let config =
            "client_id = \"test-client\"\nredirect_uri = \"http://127.0.0.1:1/callback\"\n";
        std::fs::write(self.dir.join("config/config.toml"), config).expect("config");
        std::fs::write(self.dir.join("web-credentials"), "refresh-1").expect("credentials");
        self.spotify = Some(url.to_owned());
        self
    }

    pub fn socket(&self) -> PathBuf {
        self.dir.join("daemon.sock")
    }

    pub fn log(&self) -> String {
        std::fs::read_to_string(self.dir.join("daemon.log")).unwrap_or_default()
    }

    pub fn command(&self, binary: &Path, args: &[&str]) -> Command {
        let mut command = Command::new(binary);
        command
            .args(args)
            .env("HERDR_PLUGIN_STATE_DIR", &self.dir)
            // Never the real config: a test must not learn the real developer app.
            .env("HERDR_PLUGIN_CONFIG_DIR", self.dir.join("config"))
            .env_remove("HERDR_SPOTIFY_API_URL")
            .env_remove("HERDR_SPOTIFY_ACCOUNTS_URL")
            .env_remove("HERDR_SPOTIFY_IDLE_SECS");
        if let Some(url) = &self.spotify {
            command.env("HERDR_SPOTIFY_API_URL", format!("{url}/v1"));
            command.env("HERDR_SPOTIFY_ACCOUNTS_URL", url);
        }
        if let Some(seconds) = self.idle_secs {
            command.env("HERDR_SPOTIFY_IDLE_SECS", seconds.to_string());
        }
        command
    }

    pub fn run(&self, args: &[&str]) -> Output {
        self.run_with(Path::new(env!("CARGO_BIN_EXE_herdr-spotify")), args)
    }

    pub fn run_with(&self, binary: &Path, args: &[&str]) -> Output {
        self.command(binary, args)
            .output()
            .expect("run herdr-spotify")
    }

    /// Runs a command that must succeed and returns what it printed.
    pub fn ok(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "`{}` failed: {}\nlog:\n{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr),
            self.log()
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    pub fn status(&self) -> serde_json::Value {
        serde_json::from_str(&self.ok(&["status", "--json"])).expect("status is JSON")
    }

    pub fn daemon_pid(&self) -> u32 {
        u32::try_from(self.status()["pid"].as_u64().expect("pid")).expect("pid fits")
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = self.run(&["daemon", "stop"]);
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub fn alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .output()
        .is_ok_and(|output| output.status.success())
}

/// Waits up to `limit` for `condition`; returns whether it came true.
pub fn eventually(limit: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + limit;
    loop {
        if condition() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
