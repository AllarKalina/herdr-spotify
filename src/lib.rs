//! Herdr Spotify: a local Spotify player. A background daemon plays the audio and owns every
//! conversation with Spotify; everything else talks to the daemon over a Unix socket.

pub mod cli;
pub mod client;
pub mod credentials;
pub mod daemon;
pub mod login;
pub mod paths;
pub mod protocol;

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Result<T> = std::result::Result<T, Error>;
