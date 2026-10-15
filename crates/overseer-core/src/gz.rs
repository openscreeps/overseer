//! `gz:` payloads: `gz:` followed by the base64 encoding of a compressed UTF-8 text.
//!
//! Socket events sent after `gzip on` use zlib (RFC 1950) streams and HTTP Memory values use
//! gzip (RFC 1952) streams; like the official client (`pako.ungzip`), both are accepted.

use std::io::Read;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use flate2::read::{GzDecoder, ZlibDecoder};

/// Prefix of compressed payloads.
pub const PREFIX: &str = "gz:";

/// A `gz:` payload that does not decode.
#[derive(Debug, thiserror::Error)]
pub enum GzError {
    /// The text after the prefix is not standard base64.
    #[error("invalid base64 in gz: payload")]
    Base64(#[from] base64::DecodeError),
    /// The bytes are not a complete zlib or gzip stream of UTF-8 text.
    #[error("invalid compressed stream in gz: payload")]
    Inflate(#[source] std::io::Error),
}

/// Decodes a payload, with or without the [`PREFIX`], into its text.
///
/// # Errors
///
/// Returns [`GzError`] for invalid base64, compressed data or UTF-8.
pub fn decode(payload: &str) -> Result<String, GzError> {
    let encoded = payload.strip_prefix(PREFIX).unwrap_or(payload);
    let bytes = STANDARD.decode(encoded)?;
    let mut text = String::new();
    let read = if bytes.starts_with(&[0x1f, 0x8b]) {
        GzDecoder::new(bytes.as_slice()).read_to_string(&mut text)
    } else {
        ZlibDecoder::new(bytes.as_slice()).read_to_string(&mut text)
    };
    read.map_err(GzError::Inflate)?;
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::{GzError, decode};

    #[test]
    fn decodes_zlib_socket_frames_and_gzip_memory_values() {
        // zlib level 1, as the socket compresses `["server-message","hi"]`.
        let socket = "gz:eAGLVipOLSpLLdLNTS0uTkxPVdJRyshUigUAY9YH5w==";
        assert_eq!(decode(socket).unwrap(), r#"["server-message","hi"]"#);
        // gzip, as `GET /api/user/memory` compresses `{"a":1}`.
        let memory = "gz:H4sIAAAAAAAC/6tWSlSyMqwFAK+sG1YHAAAA";
        assert_eq!(decode(memory).unwrap(), r#"{"a":1}"#);
    }

    #[test]
    fn rejects_corrupt_payloads() {
        assert!(matches!(decode("gz:not base64!"), Err(GzError::Base64(_))));
        assert!(matches!(decode("gz:AAAA"), Err(GzError::Inflate(_))));
    }
}
