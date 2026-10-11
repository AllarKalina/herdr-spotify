//! Browsing requests as the daemon answers them: what the popup is sent, and at what cost.

mod common;

use common::fake_spotify::{Reply, playlist};
use common::web_setup::Setup;
use herdr_spotify::daemon::{self, web};
use herdr_spotify::protocol::{LIKED, Message, Request};
use herdr_spotify::web::catalog::LIBRARY_TTL_MS;
use serde_json::json;
use std::time::Duration;

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

#[tokio::test]
async fn asking_for_more_of_a_list_never_opened_just_opens_it() {
    let setup = Setup::new();
    let mut daemon = Daemon::start(&setup);
    let Message::Tracks(list) = daemon.ask(Request::MoreTracks { uri: LIKED.into() }).await else {
        panic!("expected tracks")
    };
    assert!(list.complete);
    assert_eq!(setup.fake.count("GET /v1/me/tracks"), 1);
}
