//! Save times of [`UserCodeTimestamp`].

use super::types::UserCodeTimestamp;

impl UserCodeTimestamp {
    /// The save time in milliseconds since the Unix epoch: the integer as sent, or the
    /// ISO 8601 UTC date-time (`2026-10-17T01:10:59.388Z`, as a JavaScript `Date` serializes)
    /// that screeps.com and the Screeps server store for branches last saved by
    /// `POST /api/user/clone-branch`.
    /// `None` for a string of another form or a date before the epoch.
    #[must_use]
    pub fn millis(&self) -> Option<u64> {
        match self {
            Self::Integer(millis) => Some(*millis),
            Self::String(text) => iso_millis(text),
        }
    }
}

/// Milliseconds since the Unix epoch of `YYYY-MM-DDTHH:MM:SS[.fff]Z`.
fn iso_millis(text: &str) -> Option<u64> {
    let (date, time) = text.strip_suffix('Z')?.split_once('T')?;
    let mut date = date.splitn(3, '-').map(str::parse::<u64>);
    let (year, month, day) = (date.next()?.ok()?, date.next()?.ok()?, date.next()?.ok()?);
    let (clock, fraction) = time.split_once('.').unwrap_or((time, ""));
    let mut clock = clock.splitn(3, ':').map(str::parse::<u64>);
    let (hour, minute, second) = (
        clock.next()?.ok()?,
        clock.next()?.ok()?,
        clock.next()?.ok()?,
    );
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 {
        return None;
    }
    if second > 60 || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    // Milliseconds: the first three digits of the fraction.
    let millis = fraction
        .bytes()
        .chain(std::iter::repeat(b'0'))
        .take(3)
        .fold(0, |millis, digit| millis * 10 + u64::from(digit - b'0'));
    let days = days_since_epoch(year, month, day)?;
    Some((((days * 24 + hour) * 60 + minute) * 60 + second) * 1000 + millis)
}

/// Days from 1970-01-01 to the proleptic Gregorian date (Howard Hinnant's `days_from_civil`);
/// `None` before the epoch.
fn days_since_epoch(year: u64, month: u64, day: u64) -> Option<u64> {
    let year = if month <= 2 {
        year.checked_sub(1)?
    } else {
        year
    };
    let era = year / 400;
    let year_of_era = year - era * 400;
    let month_from_march = (month + 9) % 12;
    let day_of_year = (153 * month_from_march + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    (era * 146_097 + day_of_era).checked_sub(719_468)
}

#[cfg(test)]
mod tests {
    use super::UserCodeTimestamp;

    #[test]
    fn integers_and_serialized_dates_are_milliseconds_since_the_epoch() {
        let millis = |text: &str| UserCodeTimestamp::String(text.to_owned()).millis();
        assert_eq!(UserCodeTimestamp::Integer(5).millis(), Some(5));
        // `new Date(1792199459388).toISOString()`, as sent by the Screeps server.
        assert_eq!(millis("2026-10-17T01:10:59.388Z"), Some(1_792_199_459_388));
        assert_eq!(millis("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(millis("2000-02-29T12:00:00.5Z"), Some(951_825_600_500));
        for invalid in [
            "2026-10-17",
            "2026-10-17T01:10:59",
            "2026-13-01T00:00:00Z",
            "x",
        ] {
            assert_eq!(millis(invalid), None, "{invalid}");
        }
        assert_eq!(millis("1969-12-31T23:59:59Z"), None);
    }
}
