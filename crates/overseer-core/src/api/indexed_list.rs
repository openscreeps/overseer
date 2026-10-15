//! Lists that servers send either as arrays or as index-keyed objects.

use std::fmt;
use std::marker::PhantomData;
use std::ops::Deref;

use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::{Serialize, Serializer};

/// A list sent as a JSON array or as an object keyed by index (`{"0": a, "1": b}`), as stored
/// documents serialize JavaScript arrays that were written element by element (room object
/// `effects`, for example).
///
/// Object entries are ordered by numeric key; keys that are not indices follow in document
/// order. The list serializes as an array.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct IndexedList<T>(pub Vec<T>);

impl<T> IndexedList<T> {
    /// The elements.
    #[must_use]
    pub fn into_inner(self) -> Vec<T> {
        self.0
    }
}

impl<T> Deref for IndexedList<T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        &self.0
    }
}

impl<T> From<Vec<T>> for IndexedList<T> {
    fn from(items: Vec<T>) -> Self {
        Self(items)
    }
}

impl<T> IntoIterator for IndexedList<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a, T> IntoIterator for &'a IndexedList<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl<T: Serialize> Serialize for IndexedList<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for IndexedList<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(ListVisitor(PhantomData))
    }
}

struct ListVisitor<T>(PhantomData<T>);

impl<'de, T: Deserialize<'de>> Visitor<'de> for ListVisitor<T> {
    type Value = IndexedList<T>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an array or an index-keyed object")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut items = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(4096));
        while let Some(item) = seq.next_element()? {
            items.push(item);
        }
        Ok(IndexedList(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut entries: Vec<(Option<u64>, T)> = Vec::new();
        while let Some((key, value)) = map.next_entry::<String, T>()? {
            entries.push((key.parse().ok(), value));
        }
        // Stable: equal keys and non-index keys keep their document order.
        entries.sort_by_key(|(index, _)| index.unwrap_or(u64::MAX));
        Ok(IndexedList(
            entries.into_iter().map(|(_, value)| value).collect(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::IndexedList;

    #[test]
    fn arrays_and_index_keyed_objects_read_alike() {
        let array: IndexedList<u32> = serde_json::from_str("[5, 6, 7]").unwrap();
        let object: IndexedList<u32> = serde_json::from_str(r#"{"2": 7, "0": 5, "1": 6}"#).unwrap();
        assert_eq!(array, object);
        assert_eq!(&*object, [5, 6, 7]);
        assert_eq!(serde_json::to_string(&object).unwrap(), "[5,6,7]");
        let empty: IndexedList<u32> = serde_json::from_str("{}").unwrap();
        assert!(empty.is_empty());
        assert!(serde_json::from_str::<IndexedList<u32>>("3").is_err());
    }

    #[test]
    fn non_index_keys_follow_in_document_order() {
        let list: IndexedList<u32> =
            serde_json::from_str(r#"{"b": 9, "1": 2, "a": 8, "0": 1}"#).unwrap();
        assert_eq!(list.into_inner(), [1, 2, 9, 8]);
    }
}
