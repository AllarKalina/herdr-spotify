//! The Web API call budget, asserted by counting requests at a stand-in Spotify.

mod common;

use common::fake_spotify::{FakeSpotify, Reply, playlist};
use herdr_spotify::config::Config;
use herdr_spotify::daemon::{self, web};
use herdr_spotify::paths::Paths;
use herdr_spotify::protocol::{LIKED, Message, Request};
use herdr_spotify::web::catalog::{Catalog, LIBRARY_TTL_MS};
use herdr_spotify::web::{Api, Endpoints, Failure};
use serde_json::json;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

static NEXT: AtomicU32 = AtomicU32::new(0);

struct Setup {
    fake: FakeSpotify,
    paths: Paths,
}

impl Setup {
    fn new() -> Self {
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
    fn catalog(&self) -> Catalog {
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
    fn age_cache(&self, name: &str) {
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

#[tokio::test]
async fn the_library_costs_two_calls_and_one_token() {
    let setup = Setup::new();
    let library = setup.catalog().library().await.expect("library");

    assert!(library.signed_in);
    assert_eq!(library.liked_total, 3);
    let names: Vec<&str> = library.playlists.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Focus", "Running"]);
    assert_eq!(library.playlists[0].snapshot_id, "snap-1");
    assert_eq!(setup.fake.count("POST /api/token"), 1);
    assert_eq!(setup.fake.count("GET /v1/me/playlists"), 1);
    assert_eq!(setup.fake.count("GET /v1/me/tracks?limit=1"), 1);
    assert_eq!(setup.fake.api_calls(), 2);
}

#[tokio::test]
async fn one_token_serves_every_request() {
    let setup = Setup::new();
    let mut catalog = setup.catalog();
    catalog.library().await.expect("library");
    catalog.tracks("spotify:playlist:p1").await.expect("tracks");
    catalog.search("found").await.expect("search");
    assert_eq!(setup.fake.count("POST /api/token"), 1);
}

#[tokio::test]
async fn playlists_are_read_page_by_page() {
    let setup = Setup::new();
    setup.fake.once(
        "GET /v1/me/playlists",
        Reply::json(json!({"items": [playlist("a", "A", 1, "s")], "next": "more"})),
    );
    let library = setup.catalog().library().await.expect("library");
    assert_eq!(library.playlists.len(), 3);
    let calls = setup.fake.calls();
    assert!(
        calls
            .iter()
            .any(|call| call.contains("/me/playlists?limit=50&offset=0"))
    );
    assert!(
        calls
            .iter()
            .any(|call| call.contains("/me/playlists?limit=50&offset=50"))
    );
}

#[tokio::test]
async fn a_playlists_tracks_are_fetched_once_per_snapshot() {
    let setup = Setup::new();
    let mut catalog = setup.catalog();
    catalog.library().await.expect("library");

    let list = catalog.tracks("spotify:playlist:p1").await.expect("tracks");
    assert_eq!(
        list.tracks.len(),
        2,
        "a removed track, sent as null, is skipped"
    );
    assert_eq!(list.total, 3);
    assert_eq!(setup.fake.count("GET /v1/playlists/p1/items"), 1);

    // Again, and again from a new daemon: the snapshot has not changed, so nothing is asked.
    catalog.tracks("spotify:playlist:p1").await.expect("tracks");
    setup
        .catalog()
        .tracks("spotify:playlist:p1")
        .await
        .expect("tracks");
    assert_eq!(setup.fake.count("GET /v1/playlists/p1/items"), 1);

    // The playlist changes: the library reports a new snapshot, and the tracks are fetched.
    setup.fake.set(
        "GET /v1/me/playlists",
        Reply::json(json!({"items": [playlist("p1", "Focus", 4, "snap-2")], "next": null})),
    );
    catalog.library().await.expect("library");
    catalog.tracks("spotify:playlist:p1").await.expect("tracks");
    assert_eq!(setup.fake.count("GET /v1/playlists/p1/items"), 2);
}

#[tokio::test]
async fn liked_songs_are_fetched_once_until_they_are_stale() {
    let setup = Setup::new();
    let mut catalog = setup.catalog();
    let list = catalog.tracks(LIKED).await.expect("liked");
    assert_eq!(
        (list.uri.as_str(), list.tracks.len(), list.total),
        (LIKED, 2, 3)
    );
    catalog
        .tracks("spotify:user:allar:collection")
        .await
        .expect("liked by its play URI");
    assert_eq!(setup.fake.count("GET /v1/me/tracks"), 1);

    setup.age_cache("tracks-liked");
    catalog.tracks(LIKED).await.expect("liked");
    assert_eq!(setup.fake.count("GET /v1/me/tracks"), 2);
}

#[tokio::test]
async fn a_search_is_asked_once_per_query_and_never_when_empty() {
    let setup = Setup::new();
    let mut catalog = setup.catalog();
    let results = catalog.search("  Found ").await.expect("search");
    assert_eq!(
        (results.tracks.len(), results.playlists.len()),
        (1, 1),
        "nulls are skipped"
    );
    catalog
        .search("found")
        .await
        .expect("same query, different case");
    catalog.search("").await.expect("empty");
    catalog.search("   ").await.expect("blank");
    assert_eq!(setup.fake.count("GET /v1/search"), 1);
    catalog.search("something else").await.expect("search");
    assert_eq!(setup.fake.count("GET /v1/search"), 2);
}

#[tokio::test]
async fn a_rate_limit_silences_every_request_until_it_passes_even_after_a_restart() {
    let setup = Setup::new();
    setup.fake.once(
        "GET /v1/me/playlists",
        Reply::status(429).header("Retry-After", "120"),
    );
    let mut catalog = setup.catalog();

    assert_eq!(
        catalog.library().await,
        Err(Failure::Limited { seconds: 120 })
    );
    let after_limit = setup.fake.calls().len();

    // Nothing at all is sent while waiting: not the same request, not another, not by a
    // daemon started afterwards.
    assert!(matches!(
        catalog.library().await,
        Err(Failure::Limited { seconds: 119..=120 })
    ));
    assert!(matches!(
        catalog.search("found").await,
        Err(Failure::Limited { .. })
    ));
    assert!(matches!(
        setup.catalog().tracks(LIKED).await,
        Err(Failure::Limited { .. })
    ));
    assert_eq!(setup.fake.calls().len(), after_limit);

    // Once the wait is over, requests go out again.
    std::fs::write(setup.paths.web_cooldown(), "1").expect("expire the cooldown");
    catalog.library().await.expect("library after the cooldown");
}

#[tokio::test]
async fn a_rate_limit_without_a_wait_time_still_waits() {
    let setup = Setup::new();
    setup.fake.once("GET /v1/search", Reply::status(429));
    assert_eq!(
        setup.catalog().search("found").await,
        Err(Failure::Limited { seconds: 30 })
    );
}

#[tokio::test]
async fn a_failure_is_reported_and_not_retried() {
    let setup = Setup::new();
    setup.fake.once("GET /v1/me/playlists", Reply::status(500));
    let outcome = setup.catalog().library().await;
    assert!(matches!(outcome, Err(Failure::Failed(_))), "{outcome:?}");
    assert_eq!(setup.fake.count("GET /v1/me/playlists"), 1);
    assert!(
        setup.catalog().cached_library().is_none(),
        "a failed fetch caches nothing"
    );
}

#[tokio::test]
async fn a_refused_token_is_renewed_and_the_request_sent_once_more() {
    let setup = Setup::new();
    setup.fake.once("GET /v1/search", Reply::status(401));
    setup
        .catalog()
        .search("found")
        .await
        .expect("search after renewal");
    assert_eq!(setup.fake.count("POST /api/token"), 2);
    assert_eq!(setup.fake.count("GET /v1/search"), 2);
}

#[tokio::test]
async fn a_rotated_refresh_token_replaces_the_stored_one() {
    let setup = Setup::new();
    setup.fake.set(
        "POST /api/token",
        Reply::json(json!({"access_token": "a", "expires_in": 3600, "refresh_token": "refresh-2"})),
    );
    setup.catalog().search("found").await.expect("search");
    let stored = std::fs::read_to_string(setup.paths.web_credentials()).expect("credentials");
    assert_eq!(stored, "refresh-2");
}

#[tokio::test]
async fn a_revoked_sign_in_is_forgotten_and_asks_spotify_nothing_more() {
    let setup = Setup::new();
    let mut revoked = Reply::json(json!({"error": "invalid_grant"}));
    revoked.status = 400;
    setup.fake.set("POST /api/token", revoked);
    let mut catalog = setup.catalog();

    assert_eq!(catalog.library().await, Err(Failure::SignedOut));
    assert!(!setup.paths.web_credentials().exists());
    assert_eq!(catalog.search("found").await, Err(Failure::SignedOut));
    assert_eq!(setup.fake.count("POST /api/token"), 1);
    assert_eq!(setup.fake.api_calls(), 0);
}

#[tokio::test]
async fn without_a_sign_in_or_a_developer_app_nothing_is_sent() {
    let setup = Setup::new();
    std::fs::remove_file(setup.paths.web_credentials()).expect("remove");
    assert_eq!(setup.catalog().library().await, Err(Failure::SignedOut));

    std::fs::write(setup.paths.web_credentials(), "refresh-1").expect("credentials");
    let endpoints = Endpoints {
        api: format!("{}/v1", setup.fake.url),
        accounts: setup.fake.url.clone(),
    };
    let api = Api::new(setup.paths.clone(), Config::default(), endpoints).expect("api");
    assert_eq!(Catalog::new(api).library().await, Err(Failure::SignedOut));
    assert!(setup.fake.calls().is_empty());
}

/// Sends requests to the web task the way the socket server does, and collects its answers.
struct Daemon {
    hub: daemon::Hub,
    events: tokio::sync::broadcast::Receiver<Message>,
}

impl Daemon {
    fn start(setup: &Setup) -> Self {
        let (hub, _player, web_end) = daemon::hub();
        let events = hub.events.subscribe();
        tokio::spawn(web::run(web_end, setup.catalog()));
        Self { hub, events }
    }

    async fn ask(&mut self, request: Request) -> Message {
        self.hub.web.send(request).await.expect("send");
        self.next().await.expect("an answer")
    }

    async fn next(&mut self) -> Option<Message> {
        tokio::time::timeout(Duration::from_secs(3), self.events.recv())
            .await
            .ok()?
            .ok()
    }

    async fn is_quiet(&mut self) -> bool {
        tokio::time::timeout(Duration::from_millis(300), self.events.recv())
            .await
            .is_err()
    }
}

#[tokio::test]
async fn opening_with_a_warm_cache_asks_spotify_nothing() {
    let setup = Setup::new();
    let mut daemon = Daemon::start(&setup);
    let Message::Library(first) = daemon.ask(Request::GetLibrary { refresh: false }).await else {
        panic!("expected the library")
    };
    let cold = setup.fake.calls().len();

    // The popup opens again, and again after the daemon restarted: answered from the cache.
    let Message::Library(again) = daemon.ask(Request::GetLibrary { refresh: false }).await else {
        panic!("expected the library")
    };
    assert_eq!(again, first);
    assert!(daemon.is_quiet().await);
    let mut restarted = Daemon::start(&setup);
    assert_eq!(
        restarted.ask(Request::GetLibrary { refresh: false }).await,
        Message::Library(first)
    );
    assert_eq!(
        setup.fake.calls().len(),
        cold,
        "calls: {:#?}",
        setup.fake.calls()
    );
}

#[tokio::test]
async fn a_stale_library_is_shown_at_once_and_then_replaced() {
    let setup = Setup::new();
    let mut daemon = Daemon::start(&setup);
    daemon.ask(Request::GetLibrary { refresh: false }).await;
    setup.age_cache("library");
    setup.fake.set(
        "GET /v1/me/playlists",
        Reply::json(json!({"items": [playlist("p3", "New", 1, "s")], "next": null})),
    );

    let Message::Library(stale) = daemon.ask(Request::GetLibrary { refresh: false }).await else {
        panic!("expected the cached library first")
    };
    assert_eq!(stale.playlists.len(), 2);
    assert!(stale.fetched_at_ms < LIBRARY_TTL_MS);
    let Some(Message::Library(fresh)) = daemon.next().await else {
        panic!("expected the update")
    };
    assert_eq!(fresh.playlists[0].name, "New");
}

#[tokio::test]
async fn a_refresh_asks_spotify_even_when_the_cache_is_fresh() {
    let setup = Setup::new();
    let mut daemon = Daemon::start(&setup);
    daemon.ask(Request::GetLibrary { refresh: false }).await;
    daemon.ask(Request::GetLibrary { refresh: true }).await;
    assert!(matches!(daemon.next().await, Some(Message::Library(_))));
    assert_eq!(setup.fake.count("GET /v1/me/playlists"), 2);
}

#[tokio::test]
async fn failures_reach_the_popup_as_notices_and_signed_out_as_an_empty_library() {
    let setup = Setup::new();
    setup.fake.once(
        "GET /v1/search",
        Reply::status(429).header("Retry-After", "45"),
    );
    let mut daemon = Daemon::start(&setup);
    let notice = daemon
        .ask(Request::Search {
            query: "found".into(),
        })
        .await;
    let expected = "Spotify is rate limiting requests; trying again in 45s";
    assert_eq!(
        notice,
        Message::Notice {
            text: expected.into()
        }
    );

    let signed_out = Setup::new();
    std::fs::remove_file(signed_out.paths.web_credentials()).expect("remove");
    let mut daemon = Daemon::start(&signed_out);
    let Message::Library(library) = daemon.ask(Request::GetLibrary { refresh: false }).await else {
        panic!("expected a library")
    };
    assert!(!library.signed_in);
    assert!(library.playlists.is_empty());
}

#[tokio::test]
async fn tracks_and_search_answer_with_their_own_messages() {
    let setup = Setup::new();
    let mut daemon = Daemon::start(&setup);
    let Message::Tracks(list) = daemon.ask(Request::GetTracks { uri: LIKED.into() }).await else {
        panic!("expected tracks")
    };
    assert_eq!(
        list.tracks[0],
        serde_json::from_value(json!({
            "uri": "spotify:track:l1", "name": "Liked One", "artists": ["Artist"],
            "album": "Album", "duration_ms": 180_000
        }))
        .expect("track")
    );
    assert!(matches!(
        daemon.ask(Request::Search { query: "found".into() }).await,
        Message::SearchResults(results) if results.query == "found"
    ));
}
