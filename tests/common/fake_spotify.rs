//! A stand-in for Spotify's Web API and token endpoint that counts every request it gets.
//! The call budget is asserted against these counts.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Reply {
    pub fn json(body: serde_json::Value) -> Self {
        Self {
            status: 200,
            headers: Vec::new(),
            body: body.to_string(),
        }
    }

    pub fn status(status: u16) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: "{}".into(),
        }
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
}

#[derive(Default)]
struct Shared {
    /// Every request as `METHOD /path?query`, in order.
    calls: Vec<String>,
    /// Replies used once each, ahead of the standing ones.
    once: Vec<(String, VecDeque<Reply>)>,
    standing: Vec<(String, Reply)>,
}

pub struct FakeSpotify {
    pub url: String,
    shared: Arc<Mutex<Shared>>,
}

impl FakeSpotify {
    /// Starts with a small library: two playlists, three liked songs, and search results.
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake Spotify");
        let url = format!("http://{}", listener.local_addr().expect("address"));
        let shared = Arc::new(Mutex::new(Shared::default()));
        let fake = Self {
            url,
            shared: shared.clone(),
        };
        fake.set(
            "POST /api/token",
            Reply::json(serde_json::json!({
                "access_token": "access-1", "expires_in": 3600, "refresh_token": "refresh-1"
            })),
        );
        fake.set("GET /v1/me/playlists", Reply::json(serde_json::json!({
            "items": [playlist("p1", "Focus", 3, "snap-1"), playlist("p2", "Running", 40, "snap-9")],
            "next": null, "total": 2
        })));
        fake.set(
            "GET /v1/me/tracks",
            Reply::json(serde_json::json!({
                "items": [{"track": track("l1", "Liked One")}, {"track": track("l2", "Liked Two")}],
                "next": null, "total": 3
            })),
        );
        fake.set("GET /v1/playlists/p1/items", Reply::json(serde_json::json!({
            "items": [{"item": track("t1", "First")}, {"item": track("t2", "Second")}, {"item": null}],
            "next": null, "total": 3
        })));
        fake.set(
            "GET /v1/search",
            Reply::json(serde_json::json!({
                "tracks": {"items": [track("s1", "Found"), null]},
                "playlists": {"items": [null, playlist("p7", "Found List", 12, "snap-3")]}
            })),
        );
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let shared = shared.clone();
                std::thread::spawn(move || serve(stream, &shared));
            }
        });
        fake
    }

    /// The standing reply for requests starting with `prefix`, e.g. `GET /v1/me/playlists`.
    pub fn set(&self, prefix: &str, reply: Reply) {
        let mut shared = self.shared.lock().expect("lock");
        shared.standing.retain(|(known, _)| known != prefix);
        shared.standing.push((prefix.into(), reply));
    }

    /// A reply for the next matching request only.
    pub fn once(&self, prefix: &str, reply: Reply) {
        let mut shared = self.shared.lock().expect("lock");
        match shared.once.iter_mut().find(|(known, _)| known == prefix) {
            Some((_, queue)) => queue.push_back(reply),
            None => shared.once.push((prefix.into(), VecDeque::from([reply]))),
        }
    }

    pub fn calls(&self) -> Vec<String> {
        self.shared.lock().expect("lock").calls.clone()
    }

    /// How many requests so far started with `prefix`.
    pub fn count(&self, prefix: &str) -> usize {
        self.calls()
            .iter()
            .filter(|call| call.starts_with(prefix))
            .count()
    }

    /// Requests to the Web API itself, leaving out token renewals.
    pub fn api_calls(&self) -> usize {
        self.count("GET /v1/")
    }
}

pub fn playlist(id: &str, name: &str, total: u32, snapshot: &str) -> serde_json::Value {
    serde_json::json!({
        "uri": format!("spotify:playlist:{id}"), "name": name,
        "owner": {"display_name": "Allar"}, "items": {"total": total}, "snapshot_id": snapshot
    })
}

pub fn track(id: &str, name: &str) -> serde_json::Value {
    serde_json::json!({
        "uri": format!("spotify:track:{id}"), "name": name, "artists": [{"name": "Artist"}],
        "album": {"name": "Album"}, "duration_ms": 180_000, "type": "track"
    })
}

fn serve(mut stream: std::net::TcpStream, shared: &Mutex<Shared>) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
        return;
    }
    let mut length = 0;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).unwrap_or(0) == 0 || header.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0; length];
    let _ = reader.read_exact(&mut body);

    let mut parts = request_line.split_whitespace();
    let call = format!(
        "{} {}",
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or_default()
    );
    let reply = {
        let mut shared = shared.lock().expect("lock");
        shared.calls.push(call.clone());
        let queued = shared
            .once
            .iter_mut()
            .find(|(prefix, queue)| call.starts_with(prefix.as_str()) && !queue.is_empty())
            .and_then(|(_, queue)| queue.pop_front());
        queued.or_else(|| {
            // The longest matching prefix wins, so a specific path beats a general one.
            shared
                .standing
                .iter()
                .filter(|(prefix, _)| call.starts_with(prefix.as_str()))
                .max_by_key(|(prefix, _)| prefix.len())
                .map(|(_, reply)| reply.clone())
        })
    };
    let reply = reply.unwrap_or_else(|| Reply::status(404));
    let mut head = format!(
        "HTTP/1.1 {} Fake\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
        reply.status,
        reply.body.len()
    );
    for (name, value) in &reply.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    let _ = stream.write_all(format!("{head}\r\n{}", reply.body).as_bytes());
}
