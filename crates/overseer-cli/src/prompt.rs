//! Reads the secrets of `login` from stdin. When stdin is a terminal, the
//! CLI asks for each secret and does not show what the user types. Else the
//! CLI reads one line for each secret. The CLI never reads a secret from the
//! command line.

use std::io::{self, BufRead as _, IsTerminal as _};

use crate::config::Sensitive;

/// Reads the secret `what` (for example `password`) from stdin.
///
/// # Errors
///
/// Returns an error when stdin is not readable, ends before the line, or
/// gives an empty secret.
pub(crate) fn secret(what: &'static str) -> Result<Sensitive, Error> {
    let stdin = io::stdin();
    let line = if stdin.is_terminal() {
        rpassword::prompt_password(format!("{what}: "))
            .map_err(|source| Error::Read { what, source })?
    } else {
        let mut line = String::new();
        let read = stdin
            .lock()
            .read_line(&mut line)
            .map_err(|source| Error::Read { what, source })?;
        if read == 0 {
            return Err(Error::End(what));
        }
        line
    };
    let value = line.trim_end_matches(['\r', '\n']);
    if value.is_empty() {
        return Err(Error::Empty(what));
    }
    Ok(Sensitive::new(value.to_owned()))
}

/// A secret that the CLI cannot read from stdin. The message never contains
/// the secret.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    /// stdin is not readable.
    #[error("cannot read the {what} from stdin: {source}")]
    Read {
        /// The secret.
        what: &'static str,
        /// The read error.
        source: io::Error,
    },
    /// stdin ends before the line of the secret.
    #[error("stdin ends before the {0}: give each secret on its own line")]
    End(&'static str),
    /// The secret is empty.
    #[error("the {0} is empty")]
    Empty(&'static str),
}
