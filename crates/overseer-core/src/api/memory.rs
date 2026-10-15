//! `Memory` values of [`UserMemoryGetResponse`].

use super::types::UserMemoryGetResponse;
use crate::gz::{self, GzError};

/// A `Memory` value.
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

impl UserMemoryGetResponse {
    /// The `data` the server sends for a path that cannot be traversed.
    pub const INCORRECT_PATH: &'static str = "Incorrect memory path";

    /// Decodes [`Self::data`] like the official client: `gz:` payloads are decompressed and
    /// parsed as JSON; any other string is the value itself.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError`] when a `gz:` payload does not decompress or is not JSON.
    pub fn value(&self) -> Result<MemoryValue, MemoryError> {
        match self.data.as_deref() {
            None => Ok(MemoryValue::Undefined),
            Some(Self::INCORRECT_PATH) => Ok(MemoryValue::IncorrectPath),
            Some(data) if data.starts_with(gz::PREFIX) => {
                Ok(MemoryValue::Json(serde_json::from_str(&gz::decode(data)?)?))
            }
            Some(data) => Ok(MemoryValue::Json(serde_json::Value::String(
                data.to_owned(),
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{MemoryError, MemoryValue};
    use crate::api::types::UserMemoryGetResponse;

    fn memory(data: Option<&str>) -> Result<MemoryValue, MemoryError> {
        UserMemoryGetResponse {
            data: data.map(str::to_owned),
        }
        .value()
    }

    #[test]
    fn memory_value_variants() {
        assert_eq!(memory(None).unwrap(), MemoryValue::Undefined);
        assert_eq!(
            memory(Some("Incorrect memory path")).unwrap(),
            MemoryValue::IncorrectPath
        );
        // Python: base64(gzip.compress(b'{"creeps":{"Harvester1":{"role":"harvester"}},"tick":42}', mtime=0)).
        let gzipped = "gz:H4sIAAAAAAAC/6tWSi5KTS0oVrKqVvJILCpLLS5JLTIE8Yryc1KVrJQyYIJKtbU6SiWZydlKViZGtQB/b1NiOAAAAA==";
        assert_eq!(
            memory(Some(gzipped)).unwrap(),
            MemoryValue::Json(json!({"creeps": {"Harvester1": {"role": "harvester"}}, "tick": 42}))
        );
        assert_eq!(
            memory(Some("[1,2]")).unwrap(),
            MemoryValue::Json(json!("[1,2]")),
            "uncompressed data is the value itself, as in the official client"
        );
        assert!(matches!(memory(Some("gz:AAAA")), Err(MemoryError::Gz(_))));
        // Python: base64(gzip.compress(b'not json content', mtime=0)).
        let not_json = "gz:H4sIAAAAAAAC/8vLL1HIKs7PU0jOzytJzSsBAH35RCgQAAAA";
        assert!(matches!(memory(Some(not_json)), Err(MemoryError::Json(_))));
    }
}
