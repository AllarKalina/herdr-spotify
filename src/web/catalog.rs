//! Playlists, their tracks and search results, fetched as rarely as the cache allows.
//!
//! The budget, per kind of data:
//! - playlists and the Liked Songs count: at most once per `LIBRARY_TTL_MS`, or on request;
//! - a playlist's tracks: once per snapshot, so only when its contents changed;
//! - Liked Songs' tracks: at most once per `LIBRARY_TTL_MS`;
//! - a search: once per query per `SEARCH_TTL`.

use super::cache::{self, Entry};
use super::{Api, Failure};
use crate::protocol::{LIKED, Library, Playlist, SearchResults, Track, TrackList, now_ms};
use serde_json::Value;
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub const LIBRARY_TTL_MS: u64 = 30 * 60 * 1000;
const SEARCH_TTL: Duration = Duration::from_secs(5 * 60);
const SEARCHES_KEPT: usize = 20;
/// Spotify's page size for playlists and tracks.
const PAGE: usize = 50;
/// Pages fetched for one list: 500 playlists, or the first 300 tracks of a long playlist.
const PLAYLIST_PAGES: usize = 10;
const TRACK_PAGES: usize = 6;
const SEARCH_LIMIT: &str = "10";
const TRACK_FIELDS: &str = "items(item(name,artists(name),album(name),uri,duration_ms,type),\
                            track(name,artists(name),album(name),uri,duration_ms,type)),next,total";

pub struct Catalog {
    api: Api,
    searches: HashMap<String, (SearchResults, Instant)>,
}

impl Catalog {
    pub fn new(api: Api) -> Self {
        Self {
            api,
            searches: HashMap::new(),
        }
    }

    /// The library as last fetched, and whether it is recent enough to use without asking.
    pub fn cached_library(&self) -> Option<(Library, bool)> {
        let entry: Entry<Library> = cache::read(self.api.paths(), "library")?;
        let fresh = now_ms().saturating_sub(entry.fetched_at_ms) < LIBRARY_TTL_MS;
        Some((entry.value, fresh))
    }

    /// Fetches the playlists and the Liked Songs count, and caches them.
    pub async fn library(&mut self) -> Result<Library, Failure> {
        let mut playlists = Vec::new();
        for page in 0..PLAYLIST_PAGES {
            let offset = (page * PAGE).to_string();
            let query = [("limit", "50"), ("offset", offset.as_str())];
            let body = self.api.get("/me/playlists", &query).await?;
            playlists.extend(items(&body).filter_map(playlist));
            if body["next"].is_null() {
                break;
            }
        }
        let liked = self.api.get("/me/tracks", &[("limit", "1")]).await?;
        let fetched_at_ms = now_ms();
        let library = Library {
            signed_in: true,
            playlists,
            liked_total: count(&liked["total"]),
            fetched_at_ms,
        };
        let entry = Entry {
            fetched_at_ms,
            snapshot_id: String::new(),
            value: library.clone(),
        };
        cache::write(self.api.paths(), "library", &entry);
        Ok(library)
    }

    /// The tracks of a playlist, or of Liked Songs for `LIKED`. A playlist is fetched again
    /// only when its snapshot differs from the cached one.
    pub async fn tracks(&mut self, uri: &str) -> Result<TrackList, Failure> {
        let liked = uri == LIKED || uri.ends_with(":collection");
        let (name, path, snapshot) = if liked {
            (
                "tracks-liked".to_owned(),
                "/me/tracks".to_owned(),
                String::new(),
            )
        } else {
            let id = uri
                .strip_prefix("spotify:playlist:")
                .filter(|id| !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_alphanumeric()))
                .ok_or_else(|| Failure::Failed(format!("Not a playlist: {uri}")))?;
            let known = self.cached_library().and_then(|(library, _)| {
                library
                    .playlists
                    .into_iter()
                    .find(|playlist| playlist.uri == uri)
            });
            (
                format!("tracks-{id}"),
                format!("/playlists/{id}/items"),
                known.map(|p| p.snapshot_id).unwrap_or_default(),
            )
        };

        if let Some(entry) = cache::read::<TrackList>(self.api.paths(), &name) {
            let current = if liked {
                now_ms().saturating_sub(entry.fetched_at_ms) < LIBRARY_TTL_MS
            } else {
                !snapshot.is_empty() && entry.snapshot_id == snapshot
            };
            if current {
                return Ok(entry.value);
            }
        }

        let mut tracks = Vec::new();
        let mut total = 0;
        for page in 0..TRACK_PAGES {
            let offset = (page * PAGE).to_string();
            let mut query = vec![("limit", "50"), ("offset", offset.as_str())];
            if !liked {
                query.push(("fields", TRACK_FIELDS));
            }
            let body = self.api.get(&path, &query).await?;
            total = count(&body["total"]);
            // A playlist entry holds its track under `item`; Liked Songs and older answers use `track`.
            tracks.extend(items(&body).filter_map(|entry| {
                track(if entry["item"].is_object() {
                    &entry["item"]
                } else {
                    &entry["track"]
                })
            }));
            if body["next"].is_null() {
                break;
            }
        }
        let list = TrackList {
            uri: if liked { LIKED.into() } else { uri.to_owned() },
            tracks,
            total,
        };
        let entry = Entry {
            fetched_at_ms: now_ms(),
            snapshot_id: snapshot,
            value: list.clone(),
        };
        cache::write(self.api.paths(), &name, &entry);
        Ok(list)
    }

    /// Tracks and playlists matching `query`. An empty query asks Spotify nothing.
    pub async fn search(&mut self, query: &str) -> Result<SearchResults, Failure> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(SearchResults::default());
        }
        let key = query.to_lowercase();
        if let Some((results, at)) = self.searches.get(&key)
            && at.elapsed() < SEARCH_TTL
        {
            return Ok(results.clone());
        }
        let parameters = [
            ("q", query),
            ("type", "track,playlist"),
            ("limit", SEARCH_LIMIT),
        ];
        let body = self.api.get("/search", &parameters).await?;
        let results = SearchResults {
            query: query.to_owned(),
            tracks: items(&body["tracks"]).filter_map(track).collect(),
            playlists: items(&body["playlists"]).filter_map(playlist).collect(),
        };
        if self.searches.len() >= SEARCHES_KEPT {
            let oldest = self
                .searches
                .iter()
                .min_by_key(|(_, (_, at))| *at)
                .map(|(key, _)| key.clone());
            if let Some(oldest) = oldest {
                self.searches.remove(&oldest);
            }
        }
        self.searches.insert(key, (results.clone(), Instant::now()));
        Ok(results)
    }
}

fn items(body: &Value) -> impl Iterator<Item = &Value> {
    body["items"].as_array().into_iter().flatten()
}

fn count(value: &Value) -> u32 {
    value
        .as_u64()
        .and_then(|count| u32::try_from(count).ok())
        .unwrap_or(0)
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_default().to_owned()
}

/// A playlist from Spotify's JSON. Search results contain nulls; those are skipped.
fn playlist(item: &Value) -> Option<Playlist> {
    let uri = item["uri"].as_str()?;
    // The track count moved from `tracks` to `items`; accept either.
    let total = if item["items"]["total"].is_number() {
        &item["items"]["total"]
    } else {
        &item["tracks"]["total"]
    };
    Some(Playlist {
        uri: uri.to_owned(),
        name: text(&item["name"]),
        owner: text(&item["owner"]["display_name"]),
        total: count(total),
        snapshot_id: text(&item["snapshot_id"]),
    })
}

/// A track from Spotify's JSON. Episodes, removed tracks and local files are skipped.
fn track(item: &Value) -> Option<Track> {
    let uri = item["uri"]
        .as_str()
        .filter(|uri| uri.starts_with("spotify:track:"))?;
    Some(Track {
        uri: uri.to_owned(),
        name: text(&item["name"]),
        artists: item["artists"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|artist| text(&artist["name"]))
            .collect(),
        album: text(&item["album"]["name"]),
        duration_ms: count(&item["duration_ms"]),
        cover_path: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_playlist_reads_its_count_from_either_shape() {
        let new = json!({"uri":"spotify:playlist:a","name":"Focus","owner":{"display_name":"Allar"},"items":{"total":42},"snapshot_id":"s1"});
        let old = json!({"uri":"spotify:playlist:b","name":"Run","tracks":{"total":7}});
        assert_eq!(
            playlist(&new).map(|p| (p.name, p.owner, p.total, p.snapshot_id)),
            Some(("Focus".into(), "Allar".into(), 42, "s1".into()))
        );
        assert_eq!(playlist(&old).map(|p| p.total), Some(7));
        assert_eq!(playlist(&Value::Null), None);
    }

    #[test]
    fn only_spotify_tracks_become_tracks() {
        let song = json!({"uri":"spotify:track:1","name":"Hide Away","artists":[{"name":"Daya"}],"album":{"name":"Sit Still"},"duration_ms":191_000});
        let parsed = track(&song).expect("track");
        assert_eq!(
            (
                parsed.name.as_str(),
                parsed.artists.len(),
                parsed.duration_ms
            ),
            ("Hide Away", 1, 191_000)
        );
        assert_eq!(
            track(&json!({"uri":"spotify:episode:9","name":"A podcast"})),
            None
        );
        assert_eq!(track(&json!({"uri":"spotify:local:x"})), None);
        assert_eq!(track(&Value::Null), None);
    }
}
