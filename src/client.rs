//! A connection to the daemon, starting it when it is not running.

use crate::Result;
use crate::paths::Paths;
use crate::protocol::{Message, PROTOCOL, Request, encode};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::os::unix::net::UnixStream;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, UNIX_EPOCH};

const START_WAIT: Duration = Duration::from_secs(8);
const STOP_WAIT: Duration = Duration::from_secs(8);
const POLL: Duration = Duration::from_millis(50);
const LOG_LIMIT: u64 = 1024 * 1024;

pub struct Connection {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    pub daemon_pid: u32,
    pub daemon_build: String,
}

impl Connection {
    /// Connects to the daemon. Starts one when none is running, and replaces one that was
    /// built from a different executable.
    pub fn open(paths: &Paths) -> Result<Self> {
        for _ in 0..2 {
            let mut connection = match Self::existing(paths)? {
                Some(connection) => connection,
                None => {
                    spawn_daemon(paths)?;
                    wait_for(START_WAIT, || Self::existing(paths))?.ok_or_else(|| {
                        format!("The daemon did not start. See {}", paths.log().display())
                    })?
                }
            };
            if connection.daemon_build == build_stamp() {
                return Ok(connection);
            }
            connection.send(&Request::Shutdown)?;
            wait_for_exit(paths)?;
        }
        Err("Could not replace the daemon from another build".into())
    }

    /// Connects only if a daemon is already running.
    pub fn existing(paths: &Paths) -> Result<Option<Self>> {
        let stream = match UnixStream::connect(paths.socket()) {
            Ok(stream) => stream,
            Err(error)
                if matches!(
                    error.kind(),
                    ErrorKind::NotFound | ErrorKind::ConnectionRefused
                ) =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error.into()),
        };
        let mut connection = Self {
            reader: BufReader::new(stream.try_clone()?),
            writer: stream,
            daemon_pid: 0,
            daemon_build: String::new(),
        };
        connection.set_timeout(Some(START_WAIT))?;
        match connection.read() {
            Ok(Some(Message::Hello {
                protocol,
                build,
                pid,
            })) => {
                connection.daemon_pid = pid;
                // A different protocol is a different build by definition.
                connection.daemon_build = if protocol == PROTOCOL {
                    build
                } else {
                    format!("protocol-{protocol}")
                };
            }
            // Nothing is serving this socket: a daemon that is gone, or one on its way out.
            Ok(None) => return Ok(None),
            Err(error) if hung_up(&error) => return Ok(None),
            Ok(Some(other)) => {
                return Err(format!("Unexpected greeting from the daemon: {other:?}").into());
            }
            Err(error) => return Err(error),
        }
        connection.set_timeout(None)?;
        Ok(Some(connection))
    }

    pub fn send(&mut self, request: &Request) -> Result<()> {
        self.writer.write_all(encode(request)?.as_bytes())?;
        Ok(())
    }

    /// The next message; `None` when the daemon closed the connection. With a timeout set, a
    /// quiet daemon is an error of kind `WouldBlock` or `TimedOut`.
    pub fn read(&mut self) -> Result<Option<Message>> {
        let mut line = String::new();
        if self.reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        Ok(Some(serde_json::from_str(&line)?))
    }

    /// Like `read`, but a quiet daemon is `Ok(None)` after `wait`.
    pub fn read_within(&mut self, wait: Duration) -> Result<Option<Message>> {
        self.set_timeout(Some(wait))?;
        let mut line = String::new();
        let read = self.reader.read_line(&mut line);
        self.set_timeout(None)?;
        match read {
            Ok(0) => Err("The daemon closed the connection".into()),
            Ok(_) => Ok(Some(serde_json::from_str(&line)?)),
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                Ok(None)
            }
            Err(error) => Err(error.into()),
        }
    }

    /// A second handle for sending, so one thread can read while another writes.
    pub fn sender(&self) -> Result<Sender> {
        Ok(Sender(self.writer.try_clone()?))
    }

    fn set_timeout(&mut self, timeout: Option<Duration>) -> Result<()> {
        self.reader.get_ref().set_read_timeout(timeout)?;
        Ok(())
    }
}

/// The sending half of a connection.
pub struct Sender(UnixStream);

impl Sender {
    pub fn send(&mut self, request: &Request) -> Result<()> {
        self.0.write_all(encode(request)?.as_bytes())?;
        Ok(())
    }
}

/// True when a read failed because the other end went away.
fn hung_up(error: &crate::Error) -> bool {
    error.downcast_ref::<std::io::Error>().is_some_and(|error| {
        matches!(
            error.kind(),
            ErrorKind::ConnectionReset | ErrorKind::BrokenPipe
        )
    })
}

/// Stops the daemon if one is running. Returns whether there was one.
pub fn stop(paths: &Paths) -> Result<bool> {
    let Some(mut connection) = Connection::existing(paths)? else {
        return Ok(false);
    };
    connection.send(&Request::Shutdown)?;
    wait_for_exit(paths)?;
    Ok(true)
}

/// Identifies an executable: its version, and its size and modification time, so a rebuilt
/// binary never keeps talking to the daemon of the previous build.
pub fn build_stamp() -> String {
    static STAMP: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    STAMP
        .get_or_init(|| {
            let version = env!("CARGO_PKG_VERSION");
            let file = std::env::current_exe().and_then(fs::metadata);
            let Ok(file) = file else {
                return version.to_owned();
            };
            let modified = file
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |since| since.as_secs());
            format!("{version}-{modified}-{}", file.len())
        })
        .clone()
}

/// Starts the daemon detached, with its output in the log file.
fn spawn_daemon(paths: &Paths) -> Result<()> {
    paths.prepare()?;
    let log_path = paths.log();
    let oversized = fs::metadata(&log_path).is_ok_and(|file| file.len() > LOG_LIMIT);
    let log = File::options()
        .create(true)
        .append(true)
        .truncate(false)
        .open(&log_path)?;
    if oversized {
        log.set_len(0)?;
    }
    let mut child = Command::new(std::env::current_exe()?)
        .args(["daemon", "--detached"])
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .spawn()?;
    // Reap it if it exits while this process is still alive.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

fn wait_for_exit(paths: &Paths) -> Result<()> {
    let gone = wait_for(STOP_WAIT, || {
        Ok(match UnixStream::connect(paths.socket()) {
            Ok(_) => None,
            Err(_) => Some(()),
        })
    })?;
    gone.ok_or_else(|| "The daemon did not stop".into())
}

/// Polls `attempt` until it yields a value or `limit` passes.
fn wait_for<T>(
    limit: Duration,
    mut attempt: impl FnMut() -> Result<Option<T>>,
) -> Result<Option<T>> {
    let deadline = Instant::now() + limit;
    loop {
        if let Some(value) = attempt()? {
            return Ok(Some(value));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        std::thread::sleep(POLL);
    }
}
