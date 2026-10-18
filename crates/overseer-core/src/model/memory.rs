//! Memory: values at Memory paths (plain or gzip-encoded), the paths themselves
//! ([`MemoryPath`]), queued changes ([`MemoryChange`]) and raw Memory segments with their
//! limits ([`SEGMENT_MAX_UNITS`]).
//!
//! Every contract defines the Memory operations identically (codegen re-exports the official
//! types from the others), so each conversion here is written once.

mod change;
mod path;
mod segment;

pub use change::{MemoryChange, json_at};
pub use path::{MemoryPath, MemoryPathError};
pub use segment::{
    MAX_SEGMENT, SEGMENT_MAX_UNITS, SegmentError, check_segment, check_segment_data, segment_units,
};

use crate::api::official::types::{
    UserMemoryGetResponse, UserMemorySegmentGetResponse, UserMemorySegmentGetResponseData,
};
use crate::gz::{self, GzError};

/// The most serialised Memory that the server keeps for the account on each shard: 2 MiB, in
/// bytes. The usage of each run of the code reports the current size
/// ([`UserCpu::memory`](crate::model::socket::UserCpu::memory)). This is not the limit of one
/// write request, which each server sets on its own.
pub const MEMORY_CAPACITY: u64 = 2 * 1024 * 1024;

/// A `Memory` value (`GET /api/user/memory`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MemoryValue {
    /// Nothing is stored at the path.
    Undefined,
    /// The path cannot be traversed, for example through a number.
    IncorrectPath,
    /// The stored value.
    Json(serde_json::Value),
}

/// A `Memory` value that does not decode.
#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    /// The `gz:` payload does not decompress.
    #[error(transparent)]
    Gz(#[from] GzError),
    /// The value is not JSON.
    #[error("memory value is not JSON")]
    Json(#[from] serde_json::Error),
}

/// The `data` the server sends for a path that cannot be traversed.
const INCORRECT_PATH: &str = "Incorrect memory path";

/// Decodes the `GET /api/user/memory` reply, which every contract shares, like the official
/// client: an absent `data` is [`MemoryValue::Undefined`], `gz:` payloads are decompressed and
/// parsed as JSON, and any other string is the value itself.
impl TryFrom<UserMemoryGetResponse> for MemoryValue {
    type Error = MemoryError;

    fn try_from(reply: UserMemoryGetResponse) -> Result<Self, MemoryError> {
        match reply.data {
            None => Ok(Self::Undefined),
            Some(data) if data == INCORRECT_PATH => Ok(Self::IncorrectPath),
            Some(data) if data.starts_with(gz::PREFIX) => {
                Ok(Self::Json(serde_json::from_str(&gz::decode(&data)?)?))
            }
            Some(data) => Ok(Self::Json(serde_json::Value::String(data))),
        }
    }
}

/// The contents of the one segment asked for in the `GET /api/user/memory-segment` reply,
/// which every contract shares: `None` for a segment never written, whether `data` is `null`
/// (Screeps server) or absent. screeps.com answers a list in request order.
#[must_use]
pub fn segment_data(reply: UserMemorySegmentGetResponse) -> Option<String> {
    match reply.data? {
        UserMemorySegmentGetResponseData::String(data) => Some(data),
        UserMemorySegmentGetResponseData::List(list) => list.into_iter().next().flatten(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn memory(reply: &str) -> Result<MemoryValue, MemoryError> {
        MemoryValue::try_from(serde_json::from_str::<UserMemoryGetResponse>(reply).unwrap())
    }

    fn segment(reply: &str) -> Option<String> {
        segment_data(serde_json::from_str(reply).unwrap())
    }

    #[test]
    fn memory_replies_decode() {
        assert_eq!(memory(r#"{"ok":1}"#).unwrap(), MemoryValue::Undefined);
        assert_eq!(
            memory(r#"{"ok":1,"data":"Incorrect memory path"}"#).unwrap(),
            MemoryValue::IncorrectPath
        );
        // Python: base64(gzip.compress(b'{"creeps":{"Harvester1":{"role":"harvester"}},"tick":42}', mtime=0)).
        let gzipped = r#"{"ok":1,"data":"gz:H4sIAAAAAAAC/6tWSi5KTS0oVrKqVvJILCpLLS5JLTIE8Yryc1KVrJQyYIJKtbU6SiWZydlKViZGtQB/b1NiOAAAAA=="}"#;
        assert_eq!(
            memory(gzipped).unwrap(),
            MemoryValue::Json(json!({"creeps": {"Harvester1": {"role": "harvester"}}, "tick": 42}))
        );
        assert_eq!(
            memory(r#"{"ok":1,"data":"[1,2]"}"#).unwrap(),
            MemoryValue::Json(json!("[1,2]")),
            "uncompressed data is the value itself, as in the official client"
        );
        assert!(matches!(
            memory(r#"{"ok":1,"data":"gz:AAAA"}"#),
            Err(MemoryError::Gz(_))
        ));
        // Python: base64(gzip.compress(b'not json content', mtime=0)).
        let not_json = r#"{"ok":1,"data":"gz:H4sIAAAAAAAC/8vLL1HIKs7PU0jOzytJzSsBAH35RCgQAAAA"}"#;
        assert!(matches!(memory(not_json), Err(MemoryError::Json(_))));
    }

    #[test]
    fn segment_replies_decode() {
        assert_eq!(
            segment(r#"{"ok":1,"data":"{\"a\":1}"}"#).as_deref(),
            Some(r#"{"a":1}"#)
        );
        // A segment never written: the Screeps server sends `null`, others omit `data`.
        assert_eq!(segment(r#"{"ok":1,"data":null}"#), None);
        assert_eq!(segment(r#"{"ok":1}"#), None);
        // screeps.com: a list in request order.
        assert_eq!(
            segment(r#"{"ok":1,"data":["text"]}"#).as_deref(),
            Some("text")
        );
        assert_eq!(segment(r#"{"ok":1,"data":[null]}"#), None);
    }
}
