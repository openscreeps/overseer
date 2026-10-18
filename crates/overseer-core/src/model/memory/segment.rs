//! The limits of raw Memory segments.

/// The highest Memory segment id; segments are numbered `0..=MAX_SEGMENT`.
pub const MAX_SEGMENT: u8 = 99;

/// The most UTF-16 units that a segment holds (100 KiB): the servers count the length of the
/// JavaScript string. Count a text with [`segment_units`].
pub const SEGMENT_MAX_UNITS: usize = 100 * 1024;

/// Why a segment cannot be read or written.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SegmentError {
    /// The id is above [`MAX_SEGMENT`].
    #[error("segment {0} is above {MAX_SEGMENT}")]
    Id(u8),
    /// The data is longer than [`SEGMENT_MAX_UNITS`].
    #[error("segment {id} gets {units} UTF-16 units, more than {SEGMENT_MAX_UNITS}")]
    TooLong {
        /// The segment.
        id: u8,
        /// The length of the data, in UTF-16 units.
        units: usize,
    },
}

/// The length of `text` as the servers count it: UTF-16 units (`é` is 1, `😀` is 2).
#[must_use]
pub fn segment_units(text: &str) -> usize {
    text.encode_utf16().count()
}

/// Rejects a segment id above [`MAX_SEGMENT`].
///
/// # Errors
///
/// Returns [`SegmentError::Id`] for such an id.
pub const fn check_segment(id: u8) -> Result<(), SegmentError> {
    if id > MAX_SEGMENT {
        Err(SegmentError::Id(id))
    } else {
        Ok(())
    }
}

/// Rejects the write of `data` into segment `id`: an id above [`MAX_SEGMENT`] or data longer
/// than [`SEGMENT_MAX_UNITS`].
///
/// # Errors
///
/// Returns the [`SegmentError`].
pub fn check_segment_data(id: u8, data: &str) -> Result<(), SegmentError> {
    check_segment(id)?;
    // A UTF-16 unit takes at least one UTF-8 byte: short data needs no count.
    if data.len() <= SEGMENT_MAX_UNITS {
        return Ok(());
    }
    match segment_units(data) {
        units if units > SEGMENT_MAX_UNITS => Err(SegmentError::TooLong { id, units }),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_SEGMENT, SEGMENT_MAX_UNITS, SegmentError, check_segment, check_segment_data,
        segment_units,
    };

    #[test]
    fn counts_segment_length_as_the_server_does() {
        assert_eq!(segment_units("ab"), 2);
        assert_eq!(segment_units("é"), 1);
        assert_eq!(segment_units("😀"), 2);
    }

    #[test]
    fn checks_segment_ids_and_lengths() {
        assert_eq!(check_segment(0), Ok(()));
        assert_eq!(check_segment(MAX_SEGMENT), Ok(()));
        assert_eq!(check_segment(100), Err(SegmentError::Id(100)));
        assert_eq!(check_segment_data(100, ""), Err(SegmentError::Id(100)));
        let full = "é".repeat(SEGMENT_MAX_UNITS);
        assert_eq!(check_segment_data(3, &full), Ok(()));
        let over = format!("{full}a");
        assert_eq!(
            check_segment_data(3, &over),
            Err(SegmentError::TooLong {
                id: 3,
                units: SEGMENT_MAX_UNITS + 1
            })
        );
        // Each emoji is 2 units in 4 bytes.
        let emoji = "😀".repeat(SEGMENT_MAX_UNITS / 2 + 1);
        assert!(check_segment_data(0, &emoji).is_err());
        assert!(check_segment_data(0, &"😀".repeat(SEGMENT_MAX_UNITS / 2)).is_ok());
    }
}
