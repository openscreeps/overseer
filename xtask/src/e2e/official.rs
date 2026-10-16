//! The `official` target: screeps.com, read-only, with the user's API token.
//!
//! The token comes from `SCREEPS_TOKEN`, else from the file `SCREEPS_TOKEN_FILE` names (a
//! `SCREEPS_TOKEN=<token>` line, as in a `.env` file, or the token alone). It is handed to the
//! tests in their environment only (`OVERSEER_LIVE_TOKEN`): never printed, logged or written.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::Error;

/// The server.
pub const URL: &str = "https://screeps.com/";

/// The API token (see the module docs).
pub fn token() -> Result<String, Error> {
    if let Some(token) = std::env::var("SCREEPS_TOKEN")
        .ok()
        .filter(|token| !token.trim().is_empty())
    {
        return Ok(token.trim().to_owned());
    }
    let Some(path) = std::env::var_os("SCREEPS_TOKEN_FILE") else {
        return Err(
            "the official target needs an API token: set SCREEPS_TOKEN or SCREEPS_TOKEN_FILE"
                .into(),
        );
    };
    let path = Path::new(&path);
    // The error names the file only: its content is secret.
    let text = fs::read_to_string(path)
        .map_err(|error| format!("cannot read SCREEPS_TOKEN_FILE {}: {error}", path.display()))?;
    token_in(&text).ok_or_else(|| {
        format!(
            "SCREEPS_TOKEN_FILE {} holds no SCREEPS_TOKEN=<token> line nor a lone token",
            path.display()
        )
        .into()
    })
}

/// The token in a token file: the value of a `SCREEPS_TOKEN=` line (optionally `export`ed and
/// quoted), else the file's only non-empty line when it has no `=`.
fn token_in(text: &str) -> Option<String> {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    let assigned = lines.iter().find_map(|line| {
        line.trim_start_matches("export ")
            .trim_start()
            .strip_prefix("SCREEPS_TOKEN=")
    });
    let token = match (assigned, lines.as_slice()) {
        (Some(value), _) => value.trim().trim_matches(['"', '\'']),
        (None, [only]) if !only.contains('=') => only,
        _ => return None,
    };
    (!token.is_empty()).then(|| token.to_owned())
}

/// The requests `overseer-proxy` forwarded to the upstream, from its log (`log`): how many per
/// endpoint (method and path, most first), and how many socket connections.
pub fn forwarded(log: &Path) -> Result<String, Error> {
    let text = fs::read_to_string(log)
        .map_err(|error| format!("cannot read {}: {error}", log.display()))?;
    Ok(count(&text))
}

/// [`forwarded`] of the log text `text`.
fn count(text: &str) -> String {
    let mut endpoints = BTreeMap::<String, usize>::new();
    let mut sockets = 0_usize;
    for line in text.lines().map(strip_ansi) {
        let field = |name: &str| {
            line.split_whitespace()
                .find_map(|word| word.strip_prefix(name)?.strip_prefix('='))
                .map(str::to_owned)
        };
        let (Some(method), Some(path)) = (field("method"), field("path")) else {
            continue;
        };
        if path.starts_with("/socket/") {
            sockets += 1;
        } else if path.starts_with("/api/") {
            *endpoints.entry(format!("{method} {path}")).or_default() += 1;
        } else if path.starts_with("/room-history/") {
            // One path per room and tick.
            *endpoints
                .entry(format!("{method} /room-history/…"))
                .or_default() += 1;
        }
    }
    let total: usize = endpoints.values().sum();
    let mut by_endpoint: Vec<(&String, &usize)> = endpoints.iter().collect();
    by_endpoint.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    let listed: Vec<String> = by_endpoint
        .iter()
        .map(|(endpoint, count)| format!("{endpoint} {count}"))
        .collect();
    format!(
        "{total} HTTP requests ({}), {sockets} socket connections",
        if listed.is_empty() {
            "none".to_owned()
        } else {
            listed.join(", ")
        }
    )
}

/// `line` without ANSI escape sequences (`ESC [ … letter`), which colour the proxy's log.
fn strip_ansi(line: &str) -> String {
    let mut plain = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(character) = chars.next() {
        if character == '\u{1b}' {
            // Skips `[`, the parameters and the final letter.
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            plain.push(character);
        }
    }
    plain
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_files() {
        assert_eq!(
            token_in("# comment\nOTHER=1\nexport SCREEPS_TOKEN=\"abc\"\n").as_deref(),
            Some("abc")
        );
        assert_eq!(token_in("SCREEPS_TOKEN=abc").as_deref(), Some("abc"));
        assert_eq!(token_in("  abc  \n").as_deref(), Some("abc"));
        assert_eq!(token_in("OTHER=1"), None);
        assert_eq!(token_in("SCREEPS_TOKEN=\n"), None);
        assert_eq!(token_in(""), None);
    }

    #[test]
    fn proxy_log_lines() {
        let log = "INFO overseer_proxy::serve: request method=GET path=/api/auth/me status=200\n\
            INFO overseer_proxy::serve: request \u{1b}[3mmethod\u{1b}[0m\u{1b}[2m=\u{1b}[0mPOST \u{1b}[3mpath\u{1b}[0m\u{1b}[2m=\u{1b}[0m/api/game/map-stats status=200\n\
            INFO overseer_proxy::serve: request method=POST path=/api/game/map-stats status=200\n\
            INFO overseer_proxy::serve: request method=GET path=/room-history/shard0/W1N1/100.json status=200\n\
            INFO overseer_proxy::serve: request method=GET path=/index.html status=200\n\
            INFO overseer_proxy::serve: request method=GET path=/socket/websocket status=101\n\
            INFO overseer_proxy::socket: socket closed path=/socket/websocket duration_ms=5\n";
        assert_eq!(
            count(log),
            "4 HTTP requests (POST /api/game/map-stats 2, GET /api/auth/me 1, \
             GET /room-history/… 1), 1 socket connections"
        );
        assert_eq!(count(""), "0 HTTP requests (none), 0 socket connections");
    }
}
