//! The daemon's life as a process, through the real binary and without a Spotify sign-in.

mod common;

use common::{Sandbox, alive, eventually};
use std::os::unix::net::UnixListener;
use std::process::Command;
use std::time::Duration;

#[test]
fn the_first_client_starts_the_daemon_and_it_outlives_that_client() {
    let sandbox = Sandbox::new();
    assert!(!sandbox.socket().exists());

    let first = sandbox.status();
    assert_eq!(first["state"]["status"], "signed_out");
    assert_eq!(first["state"]["playing"], false);
    let pid = u32::try_from(first["pid"].as_u64().expect("pid")).expect("pid fits");
    assert!(
        alive(pid),
        "the daemon should still run after `status` exited"
    );

    assert_eq!(
        sandbox.daemon_pid(),
        pid,
        "a second client must reuse the daemon"
    );
    assert_eq!(
        sandbox.ok(&["status"]),
        "Not signed in. Run: herdr-spotify login"
    );
}

#[test]
fn only_one_daemon_runs_per_state_directory() {
    let sandbox = Sandbox::new();
    let pid = sandbox.daemon_pid();

    let second = sandbox.run(&["daemon"]);
    assert!(second.status.success());
    assert!(String::from_utf8_lossy(&second.stderr).contains("another daemon is running"));
    assert_eq!(sandbox.daemon_pid(), pid);
}

#[test]
fn stop_ends_the_daemon_and_removes_its_socket() {
    let sandbox = Sandbox::new();
    let pid = sandbox.daemon_pid();

    assert_eq!(sandbox.ok(&["daemon", "stop"]), "Stopped.");
    assert!(!sandbox.socket().exists());
    assert!(eventually(Duration::from_secs(5), || !alive(pid)));
    assert_eq!(sandbox.ok(&["daemon", "stop"]), "Not running.");
}

#[test]
fn a_socket_left_by_a_dead_daemon_is_replaced() {
    let sandbox = Sandbox::new();
    drop(UnixListener::bind(sandbox.socket()).expect("leave a dead socket behind"));
    assert!(sandbox.socket().exists());

    assert_eq!(sandbox.status()["state"]["status"], "signed_out");
}

#[test]
fn the_daemon_exits_when_idle_and_the_next_client_starts_a_new_one() {
    let sandbox = Sandbox::new().idle_after(1);
    let pid = sandbox.daemon_pid();

    assert!(
        eventually(Duration::from_secs(6), || !alive(pid)),
        "log:\n{}",
        sandbox.log()
    );
    assert!(!sandbox.socket().exists());
    assert!(sandbox.log().contains("idle for 1s"));

    let next = sandbox.daemon_pid();
    assert_ne!(next, pid);
}

#[test]
fn a_daemon_from_another_build_is_replaced() {
    let sandbox = Sandbox::new();
    let before = sandbox.status();

    // The same program with another modification time is another build.
    let copy = sandbox.dir.join("herdr-spotify-copy");
    std::fs::copy(env!("CARGO_BIN_EXE_herdr-spotify"), &copy).expect("copy binary");
    let earlier = std::time::SystemTime::now() - Duration::from_secs(3600);
    std::fs::File::options()
        .write(true)
        .open(&copy)
        .and_then(|file| file.set_modified(earlier))
        .expect("change modification time");

    let output = sandbox.run_with(&copy, &["status", "--json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let after: serde_json::Value = serde_json::from_slice(&output.stdout).expect("status is JSON");
    assert_ne!(after["pid"], before["pid"]);
    assert_ne!(after["build"], before["build"]);
    let old = u32::try_from(before["pid"].as_u64().expect("pid")).expect("pid fits");
    assert!(eventually(Duration::from_secs(5), || !alive(old)));
}

#[test]
fn a_hangup_does_not_stop_the_daemon_but_a_terminate_does() {
    let sandbox = Sandbox::new();
    let pid = sandbox.daemon_pid();
    let signal = |name: &str| {
        Command::new("kill")
            .args([name, &pid.to_string()])
            .status()
            .expect("send signal");
    };

    signal("-HUP");
    assert!(eventually(Duration::from_secs(3), || sandbox
        .log()
        .contains("ignoring hangup")));
    assert_eq!(sandbox.daemon_pid(), pid);

    signal("-TERM");
    assert!(eventually(Duration::from_secs(5), || !alive(pid)));
    assert!(!sandbox.socket().exists());
}

#[test]
fn a_request_while_signed_out_explains_how_to_sign_in() {
    let sandbox = Sandbox::new();
    let output = sandbox.run(&["ctl", "play"]);
    assert!(!output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stderr).trim(),
        "Not signed in. Run: herdr-spotify login"
    );
}

#[test]
fn the_state_directory_and_socket_are_private() {
    use std::os::unix::fs::PermissionsExt;
    let sandbox = Sandbox::new();
    sandbox.status();
    let mode = |path: &std::path::Path| {
        std::fs::metadata(path)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777
    };
    assert_eq!(mode(&sandbox.dir), 0o700);
    assert_eq!(mode(&sandbox.socket()), 0o600);
}

#[test]
fn browsing_works_end_to_end_and_survives_a_daemon_restart_without_new_calls() {
    let spotify = common::fake_spotify::FakeSpotify::start();
    let sandbox = Sandbox::new().browsing(&spotify.url);

    let listing = sandbox.ok(&["library"]);
    assert!(listing.starts_with("Liked Songs · 3 songs"), "{listing}");
    assert!(
        listing.contains("Focus · 3 songs · Allar  spotify:playlist:p1"),
        "{listing}"
    );
    assert_eq!(spotify.api_calls(), 2, "{:#?}", spotify.calls());

    let tracks = sandbox.ok(&["tracks", "spotify:playlist:p1"]);
    assert!(
        tracks.starts_with("First — Artist · 3:00  spotify:track:t1"),
        "{tracks}"
    );
    assert!(tracks.ends_with("… 2 of 3 shown"), "{tracks}");
    assert!(
        sandbox
            .ok(&["search", "found", "it"])
            .contains("playlist · Found List")
    );
    let warm = spotify.calls().len();

    // A new daemon answers the same questions from disk.
    assert_eq!(sandbox.ok(&["daemon", "stop"]), "Stopped.");
    assert_eq!(sandbox.ok(&["library"]), listing);
    assert_eq!(sandbox.ok(&["tracks", "spotify:playlist:p1"]), tracks);
    assert_eq!(spotify.calls().len(), warm, "{:#?}", spotify.calls());
}

#[test]
fn browsing_without_a_sign_in_says_how_to_get_one() {
    let sandbox = Sandbox::new();
    let output = sandbox.run(&["library"]);
    assert!(!output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stderr).trim(),
        "Browsing needs its own sign-in. Run: herdr-spotify login"
    );
}

#[test]
fn a_new_daemon_shows_what_was_playing_before_as_a_paused_track() {
    let sandbox = Sandbox::new();
    let resume = serde_json::json!({
        "context_uri": "spotify:playlist:p1",
        "track": {
            "uri": "spotify:track:t1", "name": "Hide Away", "artists": ["Daya"],
            "album": "Sit Still, Look Pretty", "duration_ms": 191_000
        },
        "position_ms": 68_000, "volume": 35, "shuffle": true, "repeat": "context"
    });
    std::fs::write(sandbox.dir.join("resume.json"), resume.to_string()).expect("resume file");

    let state = &sandbox.status()["state"];
    assert_eq!(state["track"]["name"], "Hide Away");
    assert_eq!(state["playing"], false);
    assert_eq!(state["position_ms"], 68_000);
    assert_eq!(state["context_uri"], "spotify:playlist:p1");
    assert_eq!(
        (&state["volume"], &state["shuffle"], &state["repeat"]),
        (&35.into(), &true.into(), &"context".into())
    );
}

#[test]
fn a_damaged_resume_file_is_ignored() {
    let sandbox = Sandbox::new();
    std::fs::write(sandbox.dir.join("resume.json"), "{not json").expect("resume file");
    let state = &sandbox.status()["state"];
    assert!(state["track"].is_null());
    assert_eq!(state["status"], "signed_out");
}
