//! The Web API call budget, asserted by counting requests at a stand-in Spotify.

mod common;

use common::fake_spotify::{Reply, playlist};
use common::web_setup::Setup;
use herdr_spotify::config::Config;
use herdr_spotify::protocol::LIKED;
use herdr_spotify::web::catalog::Catalog;
use herdr_spotify::web::{Api, Endpoints, Failure};
use serde_json::json;

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

fn liked_page(ids: &[&str], total: u32, more: bool) -> Reply {
    let items: Vec<_> = ids
        .iter()
        .map(|id| json!({"track": common::fake_spotify::track(id, &format!("Liked {id}"))}))
        .collect();
    let next = if more { json!("more") } else { json!(null) };
    Reply::json(json!({"items": items, "next": next, "total": total}))
}

#[tokio::test]
async fn a_long_list_loads_one_page_at_a_time_and_only_when_asked() {
    let setup = Setup::new();
    setup
        .fake
        .once("GET /v1/me/tracks", liked_page(&["a", "b"], 5, true));
    setup
        .fake
        .once("GET /v1/me/tracks", liked_page(&["c", "d"], 5, true));
    setup
        .fake
        .once("GET /v1/me/tracks", liked_page(&["e"], 5, false));
    let mut catalog = setup.catalog();

    let first = catalog.tracks(LIKED).await.expect("first page");
    assert_eq!(
        (first.tracks.len(), first.total, first.complete),
        (2, 5, false)
    );
    catalog.tracks(LIKED).await.expect("opening it again");
    assert_eq!(
        setup.fake.count("GET /v1/me/tracks"),
        1,
        "opening a list costs one call"
    );

    let second = catalog.more_tracks(LIKED).await.expect("second page");
    assert_eq!((second.tracks.len(), second.complete), (4, false));
    let all = catalog.more_tracks(LIKED).await.expect("last page");
    assert_eq!((all.tracks.len(), all.complete), (5, true));
    let names: Vec<&str> = all.tracks.iter().map(|track| track.name.as_str()).collect();
    assert_eq!(
        names,
        ["Liked a", "Liked b", "Liked c", "Liked d", "Liked e"]
    );
    assert!(
        setup
            .fake
            .calls()
            .iter()
            .any(|call| call.contains("/me/tracks?limit=50&offset=50"))
    );
    assert!(
        setup
            .fake
            .calls()
            .iter()
            .any(|call| call.contains("/me/tracks?limit=50&offset=100"))
    );

    // Nothing is left to load, and a new daemon has every page on disk.
    catalog.more_tracks(LIKED).await.expect("nothing more");
    let reopened = setup.catalog().tracks(LIKED).await.expect("from disk");
    assert_eq!(reopened, all);
    assert_eq!(setup.fake.count("GET /v1/me/tracks"), 3);
}

#[tokio::test]
async fn stale_liked_songs_are_checked_with_one_call_and_keep_their_loaded_pages() {
    let setup = Setup::new();
    setup
        .fake
        .once("GET /v1/me/tracks", liked_page(&["a", "b"], 3, true));
    setup
        .fake
        .once("GET /v1/me/tracks", liked_page(&["c"], 3, false));
    let mut catalog = setup.catalog();
    catalog.tracks(LIKED).await.expect("first page");
    catalog.more_tracks(LIKED).await.expect("second page");

    // Half an hour later nothing has changed: one call confirms it, and all pages are kept.
    setup.age_cache("tracks-liked");
    setup
        .fake
        .once("GET /v1/me/tracks", liked_page(&["a", "b"], 3, true));
    let unchanged = catalog.tracks(LIKED).await.expect("revalidated");
    assert_eq!((unchanged.tracks.len(), unchanged.complete), (3, true));
    assert_eq!(setup.fake.count("GET /v1/me/tracks"), 3);
    catalog.tracks(LIKED).await.expect("fresh again");
    assert_eq!(setup.fake.count("GET /v1/me/tracks"), 3);

    // A newly liked song changes the first page: the list starts over from it.
    setup.age_cache("tracks-liked");
    setup
        .fake
        .once("GET /v1/me/tracks", liked_page(&["new", "a"], 4, true));
    let changed = catalog.tracks(LIKED).await.expect("changed");
    assert_eq!(
        (changed.tracks.len(), changed.total, changed.complete),
        (2, 4, false)
    );
    assert_eq!(changed.tracks[0].name, "Liked new");
}

#[tokio::test]
async fn a_changed_playlist_starts_over_from_its_first_page() {
    let setup = Setup::new();
    let page = |ids: &[&str], more: bool| {
        let items: Vec<_> = ids
            .iter()
            .map(|id| json!({"item": common::fake_spotify::track(id, id)}))
            .collect();
        let next = if more { json!("more") } else { json!(null) };
        Reply::json(json!({"items": items, "next": next, "total": 3}))
    };
    setup
        .fake
        .once("GET /v1/playlists/p1/items", page(&["a", "b"], true));
    setup
        .fake
        .once("GET /v1/playlists/p1/items", page(&["c"], false));
    let mut catalog = setup.catalog();
    catalog.library().await.expect("library");
    catalog
        .tracks("spotify:playlist:p1")
        .await
        .expect("first page");
    let all = catalog
        .more_tracks("spotify:playlist:p1")
        .await
        .expect("second page");
    assert_eq!(all.tracks.len(), 3);

    setup.fake.set(
        "GET /v1/me/playlists",
        Reply::json(json!({"items": [playlist("p1", "Focus", 3, "snap-2")], "next": null})),
    );
    catalog
        .library()
        .await
        .expect("library with the new snapshot");
    let fresh = catalog
        .tracks("spotify:playlist:p1")
        .await
        .expect("after the change");
    assert_eq!(
        fresh.tracks.len(),
        2,
        "the standing first page, not the old three"
    );
    assert_eq!(setup.fake.count("GET /v1/playlists/p1/items"), 3);
}
