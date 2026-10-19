//! The selector: a local program of the bot that chooses the room and the
//! tile of the first spawn. The `selector` key of a spawn table names the
//! program. The selector chooses from the candidates that `poll` offers.
//!
//! The selector runs without a shell, in the directory of the configuration.
//! It has a time limit and a limit on its output.
//!
//! The selector reads one JSON record on stdin:
//!
//! ```json
//! {"version": 1, "shard": "shard3", "rooms": [{"name": "W1N1",
//!   "terrain": "2500 digits 0..3", "objects": [...], "status": {...}}]}
//! ```
//!
//! The selector writes one JSON record on stdout, and nothing else. It
//! writes diagnostics on stderr:
//!
//! ```json
//! {"version": 1, "room": "W1N1", "x": 25, "y": 25, "reason": "..."}
//! {"version": 1, "room": null, "reason": "no suitable room"}
//! ```
//!
//! The answer must name a candidate and a tile from 0 to 49. Before a
//! placement, `poll` checks the tile again.

use std::io::{self, Read, Write as _};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use overseer::model::world::{GameRoomObject, GameRoomStatus, Terrain};
use serde::{Deserialize, Serialize, Serializer};

/// The version of the protocol.
const VERSION: u64 = 1;
/// The maximum run time of the selector.
pub(crate) const TIMEOUT: Duration = Duration::from_secs(60);
/// The maximum number of bytes that the selector can write to stdout, and
/// to stderr.
pub(crate) const OUTPUT: usize = 64 * 1024;
/// The time that the output can stay open after the selector exits.
const GRACE: Duration = Duration::from_secs(2);
/// The interval between two checks for the exit of the selector.
const TICK: Duration = Duration::from_millis(10);

/// A room that the selector can choose.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Candidate {
    /// The name of the room.
    pub(crate) name: String,
    /// The terrain of the room. The CLI encodes it only when it writes the
    /// input of the selector.
    #[serde(serialize_with = "serialize_terrain")]
    pub(crate) terrain: Arc<Terrain>,
    /// The objects of the room.
    pub(crate) objects: Vec<GameRoomObject>,
    /// The status of the room.
    pub(crate) status: GameRoomStatus,
}

/// Writes the terrain as row-major digits for the selector. The `Terrain`
/// from `overseer` is already valid.
fn serialize_terrain<S: Serializer>(terrain: &Terrain, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.collect_str(&terrain.encoded())
}

/// The input of the selector.
#[derive(Serialize)]
struct Input<'a> {
    version: u64,
    shard: Option<&'a str>,
    rooms: &'a [Candidate],
}

/// The answer of the selector, as the selector writes it.
#[derive(Deserialize)]
struct Answer {
    version: u64,
    room: Option<String>,
    #[serde(default)]
    x: Option<i64>,
    #[serde(default)]
    y: Option<i64>,
    reason: String,
}

/// What the selector chose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Choice {
    /// The spawn at `x`, `y` in the candidate `room`.
    Room {
        /// The room.
        room: String,
        /// The column.
        x: u8,
        /// The row.
        y: u8,
        /// The reason of the selector.
        reason: String,
    },
    /// No candidate is suitable.
    Nothing {
        /// The reason of the selector.
        reason: String,
    },
}

/// What a run of the selector chose, and what it wrote to stderr.
#[derive(Debug)]
pub(crate) struct Selection {
    /// The choice.
    pub(crate) choice: Choice,
    /// The diagnostics of the selector, at most [`OUTPUT`] bytes.
    pub(crate) stderr: String,
}

/// Runs the selector `argv` in `cwd` on the candidates `rooms` of `shard`.
///
/// # Errors
///
/// Returns an error when the selector does not run, goes above its limits,
/// fails, or gives an answer that does not obey the protocol.
pub(crate) fn choose(
    argv: &[String],
    cwd: &Path,
    shard: Option<&str>,
    rooms: &[Candidate],
) -> Result<Selection, Error> {
    let input = serde_json::to_vec(&Input {
        version: VERSION,
        shard,
        rooms,
    })
    .map_err(Error::Input)?;
    let run = run(argv, cwd, input, TIMEOUT, OUTPUT)?;
    let stderr = String::from_utf8_lossy(&run.stderr).into_owned();
    if !run.status.success() {
        return Err(Error::Failed {
            status: run.status,
            stderr: last_line(&stderr),
        });
    }
    if run.overflow {
        return Err(Error::TooLong);
    }
    Ok(Selection {
        choice: parse(&run.stdout, rooms)?,
        stderr,
    })
}

/// Parses the choice in `stdout` among the candidates `rooms`.
fn parse(stdout: &[u8], rooms: &[Candidate]) -> Result<Choice, Error> {
    let answer: Answer = serde_json::from_slice(stdout)
        .map_err(|error| Error::Protocol(format!("not one JSON answer: {error}")))?;
    if answer.version != VERSION {
        return Err(Error::Protocol(format!(
            "version {}, not {VERSION}",
            answer.version
        )));
    }
    let Some(room) = answer.room else {
        if answer.x.is_some() || answer.y.is_some() {
            return Err(Error::Protocol("x and y without a room".to_owned()));
        }
        return Ok(Choice::Nothing {
            reason: answer.reason,
        });
    };
    if !rooms.iter().any(|candidate| candidate.name == room) {
        return Err(Error::Protocol(format!("room {room} is not a candidate")));
    }
    let tile = |value: Option<i64>, axis| {
        value
            .and_then(|value| u8::try_from(value).ok())
            .filter(|&value| value < 50)
            .ok_or_else(|| Error::Protocol(format!("{axis} is not a tile 0 to 49")))
    };
    Ok(Choice::Room {
        x: tile(answer.x, "x")?,
        y: tile(answer.y, "y")?,
        room,
        reason: answer.reason,
    })
}

/// Returns the last line of `text` that is not empty, for an error.
fn last_line(text: &str) -> String {
    text.lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("no diagnostics on stderr")
        .trim()
        .to_owned()
}

/// A finished run of a program.
#[derive(Debug)]
struct Run {
    /// The exit status.
    status: ExitStatus,
    /// The stdout, at most the limit.
    stdout: Vec<u8>,
    /// Whether stdout went beyond the limit.
    overflow: bool,
    /// The stderr, at most the limit.
    stderr: Vec<u8>,
}

/// An output stream of the program.
enum Stream {
    /// The stdout stream.
    Out,
    /// The stderr stream.
    Err,
}

/// Runs `argv` in `cwd` with `input` on stdin, for a maximum of `timeout`.
/// Keeps a maximum of `limit` bytes of each output.
fn run(
    argv: &[String],
    cwd: &Path,
    input: Vec<u8>,
    timeout: Duration,
    limit: usize,
) -> Result<Run, Error> {
    let (program, arguments) = argv.split_first().ok_or(Error::NoProgram)?;
    let mut child = Command::new(program)
        .args(arguments)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| Error::Spawn {
            program: program.clone(),
            source,
        })?;
    let (Some(mut stdin), Some(stdout), Some(stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        stop(&mut child);
        return Err(Error::Pipes);
    };
    // The selector can exit before it reads all the input. Its exit status
    // and its answer show the result, not a broken pipe.
    thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let (sender, receiver) = mpsc::channel();
    let out = sender.clone();
    thread::spawn(move || {
        let _ = out.send((Stream::Out, read_bounded(stdout, limit)));
    });
    thread::spawn(move || {
        let _ = sender.send((Stream::Err, read_bounded(stderr, limit)));
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(TICK),
            Ok(None) => {
                stop(&mut child);
                return Err(Error::Timeout(timeout));
            }
            Err(source) => {
                stop(&mut child);
                return Err(Error::Wait(source));
            }
        }
    };
    let grace = Instant::now() + GRACE;
    let (mut stdout, mut stderr) = (None, None);
    while stdout.is_none() || stderr.is_none() {
        let left = grace.saturating_duration_since(Instant::now());
        match receiver.recv_timeout(left) {
            Ok((Stream::Out, read)) => stdout = Some(read.map_err(Error::Read)?),
            Ok((Stream::Err, read)) => stderr = Some(read.map_err(Error::Read)?),
            Err(_) => return Err(Error::Unclosed),
        }
    }
    let (Some((stdout, overflow)), Some((stderr, _))) = (stdout, stderr) else {
        return Err(Error::Unclosed);
    };
    Ok(Run {
        status,
        stdout,
        overflow,
        stderr,
    })
}

/// Kills `child` and reaps it.
fn stop(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Reads a maximum of `limit` bytes of `reader`, and tells whether `reader`
/// had more. Reads the remaining bytes to the end and drops them, so that
/// the writer does not block.
fn read_bounded(mut reader: impl Read, limit: usize) -> io::Result<(Vec<u8>, bool)> {
    let mut bytes = Vec::new();
    let most = u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1);
    (&mut reader).take(most).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        bytes.truncate(limit);
        io::copy(&mut reader, &mut io::sink())?;
        Ok((bytes, true))
    } else {
        Ok((bytes, false))
    }
}

/// A failed run of the selector.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    /// The configuration names no program.
    #[error("no program")]
    NoProgram,
    /// The input does not serialize.
    #[error("the input: {0}")]
    Input(#[source] serde_json::Error),
    /// The program does not start.
    #[error("{program}: {source}")]
    Spawn {
        /// The program.
        program: String,
        /// Why the program does not start.
        source: io::Error,
    },
    /// The pipes of the program are not available.
    #[error("the pipes of the program are not available")]
    Pipes,
    /// Waiting for the program failed.
    #[error("waiting: {0}")]
    Wait(#[source] io::Error),
    /// Reading the output failed.
    #[error("reading the output: {0}")]
    Read(#[source] io::Error),
    /// The program ran too long, and the CLI killed it.
    #[error("no answer within {} s; killed", .0.as_secs())]
    Timeout(Duration),
    /// The output stayed open after the program exited. A child process of
    /// the program holds it.
    #[error("the output stayed open after the program exited")]
    Unclosed,
    /// The program failed.
    #[error("failed ({status}): {stderr}")]
    Failed {
        /// The exit status.
        status: ExitStatus,
        /// The last line of its stderr.
        stderr: String,
    },
    /// The program wrote more than [`OUTPUT`] bytes to stdout.
    #[error("more than {} KiB on stdout", OUTPUT / 1024)]
    TooLong,
    /// The answer does not obey the protocol.
    #[error("the answer: {0}")]
    Protocol(String),
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    /// A candidate named `name`.
    fn candidate(name: &str) -> Candidate {
        Candidate {
            name: name.to_owned(),
            terrain: Arc::new(Terrain::decode(&"0".repeat(2500)).expect("plain room")),
            objects: Vec::new(),
            status: GameRoomStatus::default(),
        }
    }

    /// Parses the answer `answer` among W1N1 and W2N2.
    fn answer(answer: &Value) -> Result<Choice, Error> {
        parse(
            answer.to_string().as_bytes(),
            &[candidate("W1N1"), candidate("W2N2")],
        )
    }

    /// An answer names a candidate and a tile, or no room. All other
    /// answers do not obey the protocol.
    #[test]
    fn answers() {
        assert_eq!(
            answer(&json!({"version": 1, "room": "W2N2", "x": 25, "y": 30, "reason": "best"})).ok(),
            Some(Choice::Room {
                room: "W2N2".to_owned(),
                x: 25,
                y: 30,
                reason: "best".to_owned()
            })
        );
        assert_eq!(
            answer(&json!({"version": 1, "room": null, "reason": "none"})).ok(),
            Some(Choice::Nothing {
                reason: "none".to_owned()
            })
        );
        for bad in [
            json!({"version": 2, "room": null, "reason": "r"}),
            json!({"version": 1, "room": "W3N3", "x": 25, "y": 25, "reason": "r"}),
            json!({"version": 1, "room": "W1N1", "x": 50, "y": 25, "reason": "r"}),
            json!({"version": 1, "room": "W1N1", "x": -1, "y": 25, "reason": "r"}),
            json!({"version": 1, "room": "W1N1", "x": 25.5, "y": 25, "reason": "r"}),
            json!({"version": 1, "room": "W1N1", "x": 25, "reason": "r"}),
            json!({"version": 1, "room": null, "x": 1, "y": 1, "reason": "r"}),
            json!({"version": 1, "room": "W1N1", "x": 25, "y": 25}),
        ] {
            assert!(matches!(answer(&bad), Err(Error::Protocol(_))), "{bad}");
        }
        let two = b"{\"version\":1,\"room\":null,\"reason\":\"a\"}\n{\"version\":1,\"room\":null,\"reason\":\"b\"}";
        assert!(matches!(parse(two, &[]), Err(Error::Protocol(_))));
        assert_eq!(last_line("a\nlast \n\n"), "last");
    }

    /// The bounded reader keeps the limit and drains the remaining bytes.
    #[test]
    fn bounded() {
        let (kept, over) = read_bounded(&b"0123456789"[..], 4).expect("reads");
        assert_eq!((kept.as_slice(), over), (&b"0123"[..], true));
        let (kept, over) = read_bounded(&b"0123"[..], 4).expect("reads");
        assert_eq!((kept.as_slice(), over), (&b"0123"[..], false));
    }

    /// A program that copies stdin to stdout.
    fn echo() -> Vec<String> {
        if cfg!(windows) {
            vec!["findstr".to_owned(), "/R".to_owned(), "^".to_owned()]
        } else {
            vec!["cat".to_owned()]
        }
    }

    /// A program that runs for some seconds.
    fn slow() -> Vec<String> {
        if cfg!(windows) {
            ["ping", "-n", "6", "127.0.0.1"].map(String::from).to_vec()
        } else {
            ["sleep", "5"].map(String::from).to_vec()
        }
    }

    /// A real program returns its stdout within the limit. The CLI kills a
    /// slow program. A missing program does not start.
    #[test]
    fn programs() {
        let text = br#"{"version":1,"room":null,"reason":"none"}"#;
        let echoed = run(&echo(), Path::new("."), text.to_vec(), TIMEOUT, OUTPUT).expect("runs");
        assert!(echoed.status.success());
        assert!(!echoed.overflow);
        assert_eq!(
            parse(&echoed.stdout, &[]).ok(),
            Some(Choice::Nothing {
                reason: "none".to_owned()
            })
        );

        let small = super::run(&echo(), Path::new("."), text.to_vec(), TIMEOUT, 8).expect("runs");
        assert!(small.overflow);
        assert_eq!(small.stdout.len(), 8);

        let started = Instant::now();
        let slow = super::run(
            &slow(),
            Path::new("."),
            Vec::new(),
            Duration::from_millis(300),
            OUTPUT,
        );
        assert!(matches!(slow, Err(Error::Timeout(_))), "{slow:?}");
        assert!(started.elapsed() < Duration::from_secs(4));

        let missing = vec!["overseer-no-such-selector".to_owned()];
        assert!(matches!(
            super::run(&missing, Path::new("."), Vec::new(), TIMEOUT, OUTPUT),
            Err(Error::Spawn { .. })
        ));
        assert!(matches!(
            super::run(&[], Path::new("."), Vec::new(), TIMEOUT, OUTPUT),
            Err(Error::NoProgram)
        ));
    }
}
