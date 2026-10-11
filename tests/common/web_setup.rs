//! A signed-in catalog pointed at a stand-in Spotify, with its own state directory.

use super::fake_spotify::FakeSpotify;
use herdr_spotify::config::Config;
use herdr_spotify::paths::Paths;
use herdr_spotify::web::catalog::Catalog;
use herdr_spotify::web::{Api, Endpoints};
use serde_json::json;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

static NEXT: AtomicU32 = AtomicU32::new(0);

pub struct Setup {
    pub fake: FakeSpotify,
    pub paths: Paths,
}

impl Setup {
    pub fn new() -> Self {
        let name = format!(
            "hs-web-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        );
        let dir = PathBuf::from("/tmp").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        let paths = Paths::at(dir).expect("paths");
        paths.prepare().expect("state dir");
        std::fs::write(paths.web_credentials(), "refresh-1").expect("credentials");
        Self {
            fake: FakeSpotify::start(),
            paths,
        }
    }

    /// A catalog as a newly started daemon would build it.
    pub fn catalog(&self) -> Catalog {
        let config = Config {
            client_id: "test-client".into(),
            redirect_uri: "http://127.0.0.1:1/callback".into(),
        };
        let endpoints = Endpoints {
            api: format!("{}/v1", self.fake.url),
            accounts: self.fake.url.clone(),
        };
        Catalog::new(Api::new(self.paths.clone(), config, endpoints).expect("api"))
    }

    /// Rewrites a cached file's fetch time, as if it had been fetched long ago.
    pub fn age_cache(&self, name: &str) {
        let path = self.paths.cache().join(format!("{name}.json"));
        let mut entry: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).expect("cache file")).expect("json");
        entry["fetched_at_ms"] = json!(1);
        entry["value"]["fetched_at_ms"] = json!(1);
        std::fs::write(path, entry.to_string()).expect("write");
    }
}

impl Drop for Setup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.paths.state());
    }
}
