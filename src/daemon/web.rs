//! Answers browsing requests from the catalog, one at a time.
//!
//! One at a time is deliberate: a second identical request waits for the first and is then
//! served from the cache, so nothing is ever asked of Spotify twice at once.

use super::WebEnd;
use crate::protocol::{Library, Message, Request};
use crate::web::Failure;
use crate::web::catalog::Catalog;

pub async fn run(mut end: WebEnd, mut catalog: Catalog) {
    loop {
        let request = tokio::select! {
            () = end.stop.notified() => return,
            request = end.requests.recv() => match request {
                Some(request) => request,
                None => return,
            },
        };
        match request {
            Request::GetLibrary { refresh } => library(&end, &mut catalog, refresh).await,
            Request::GetTracks { uri } => match catalog.tracks(&uri).await {
                Ok(tracks) => send(&end, Message::Tracks(tracks)),
                Err(failure) => notice(&end, &failure),
            },
            Request::Search { query } => match catalog.search(&query).await {
                Ok(results) => send(&end, Message::SearchResults(results)),
                Err(failure) => notice(&end, &failure),
            },
            // Everything else belongs to the player.
            _ => {}
        }
    }
}

/// Sends the cached library at once. If it is stale or a refresh was requested, asks Spotify
/// and sends the answer too.
async fn library(end: &WebEnd, catalog: &mut Catalog, refresh: bool) {
    let cached = catalog.cached_library();
    if let Some((library, fresh)) = &cached {
        send(end, Message::Library(library.clone()));
        if *fresh && !refresh {
            return;
        }
    }
    match catalog.library().await {
        Ok(library) => send(end, Message::Library(library)),
        // Without a sign-in there is nothing to show; say so once, as data rather than an error.
        Err(Failure::SignedOut) => send(end, Message::Library(Library::default())),
        Err(failure) => notice(end, &failure),
    }
}

fn send(end: &WebEnd, message: Message) {
    let _ = end.events.send(message);
}

fn notice(end: &WebEnd, failure: &Failure) {
    send(
        end,
        Message::Notice {
            text: failure.to_string(),
        },
    );
}
