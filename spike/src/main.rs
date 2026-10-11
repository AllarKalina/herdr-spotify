//! Throwaway spike. Answers: can librespot sign in with a token from our own developer app,
//! and can it start a context at a chosen track without the Web API?
//!
//!   cargo run --release                         liked songs, own client ID
//!   cargo run --release -- <context-uri> [index]
//!   SPIKE_AUTH=librespot cargo run --release    librespot's own OAuth, for comparison
//!   SPIKE_SESSION_CLIENT_ID=own                 also give the session our client ID

mod pkce;

use librespot_connect::{ConnectConfig, LoadRequest, LoadRequestOptions, PlayingTrack, Spirc};
use librespot_core::{authentication::Credentials, config::SessionConfig, session::Session};
use librespot_playback::{
    audio_backend,
    config::{AudioFormat, PlayerConfig},
    mixer::{self, MixerConfig},
    player::Player,
};
use std::time::{Duration, Instant};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("warn,librespot=info"),
    )
    .init();

    let mut args = std::env::args().skip(1);
    let context = args.next();
    let index = args.next().map(|value| value.parse::<u32>()).transpose()?;
    let own_auth = std::env::var("SPIKE_AUTH").as_deref() != Ok("librespot");

    let mut session_config = SessionConfig::default();
    let token = if own_auth {
        let config = pkce::Config::load()?;
        println!("auth: own client ID, redirect {}", config.redirect_uri);
        if std::env::var("SPIKE_SESSION_CLIENT_ID").as_deref() == Ok("own") {
            session_config.client_id.clone_from(&config.client_id);
            println!("session: using own client ID");
        }
        pkce::login(&config)?
    } else {
        println!("auth: librespot OAuth client");
        librespot_oauth::OAuthClientBuilder::new(
            &session_config.client_id,
            "http://127.0.0.1:8898/login",
            vec!["streaming"],
        )
        .open_in_browser()
        .build()?
        .get_access_token()?
        .access_token
    };

    let started = Instant::now();
    let session = Session::new(session_config, None);
    let mixer = mixer::find(None).ok_or("no mixer")?(MixerConfig::default())?;
    let sink = audio_backend::find(None).ok_or("no audio backend")?;
    let player = Player::new(
        PlayerConfig::default(),
        session.clone(),
        mixer.get_soft_volume(),
        move || sink(None, AudioFormat::default()),
    );

    let mut events = player.get_player_event_channel();
    tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            println!("event +{:>6.1}s {event:?}", started.elapsed().as_secs_f32());
        }
    });

    let connect_config = ConnectConfig {
        name: format!("Herdr · {}", hostname()),
        ..ConnectConfig::default()
    };
    let (spirc, spirc_task) = Spirc::new(
        connect_config,
        session.clone(),
        Credentials::with_access_token(token),
        player,
        mixer,
    )
    .await?;
    println!(
        "RESULT sign-in: ok as {} in {:.1}s",
        session.username(),
        started.elapsed().as_secs_f32()
    );

    let context =
        context.unwrap_or_else(|| format!("spotify:user:{}:collection", session.username()));
    println!("loading {context} at index {index:?}");
    spirc.activate()?;
    spirc.load(LoadRequest::from_context_uri(
        context,
        LoadRequestOptions {
            start_playing: true,
            playing_track: index.map(PlayingTrack::Index),
            ..LoadRequestOptions::default()
        },
    ))?;
    spirc.play()?;

    let task = tokio::spawn(spirc_task);
    for (seconds, label) in [(20, "next"), (10, "seek 60s"), (8, "pause"), (4, "shutdown")] {
        tokio::time::sleep(Duration::from_secs(seconds)).await;
        println!("command: {label}");
        match label {
            "next" => spirc.next()?,
            "seek 60s" => spirc.set_position_ms(60_000)?,
            "pause" => spirc.pause()?,
            _ => spirc.shutdown()?,
        }
    }
    let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
    Ok(())
}

fn hostname() -> String {
    std::process::Command::new("scutil")
        .args(["--get", "ComputerName"])
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Mac".into())
}
