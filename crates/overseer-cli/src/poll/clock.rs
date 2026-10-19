//! The time calculations of `poll`: the cooldown after a reset, the current
//! time for room statuses, and timestamps for the output.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use overseer::model::account::RESPAWN_COOLDOWN_MS;

/// A margin on the cooldown for differences between the server clock and
/// the host clock.
const COOLDOWN_MARGIN: Duration = Duration::from_secs(10);

/// Returns the time left in the cooldown at `now`, or `None` when the
/// cooldown is over. The cooldown starts at the later reset: `server` is the
/// last reset that the server reports (milliseconds since the Unix epoch),
/// and `local` is the last reset from this process.
pub(super) fn cooldown(
    server: Option<f64>,
    local: Option<SystemTime>,
    now: SystemTime,
) -> Option<Duration> {
    let wait = Duration::from_secs_f64(RESPAWN_COOLDOWN_MS / 1000.0) + COOLDOWN_MARGIN;
    let server = server
        .filter(|ms| ms.is_finite() && *ms > 0.0)
        .and_then(|ms| Duration::try_from_secs_f64(ms / 1000.0).ok())
        .map(|since| UNIX_EPOCH + since + wait);
    let local = local.map(|at| at + wait);
    let until = server.max(local)?;
    until
        .duration_since(now)
        .ok()
        .filter(|left| !left.is_zero())
}

/// Returns the current time in milliseconds since the Unix epoch.
pub(super) fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64() * 1000.0)
}

/// Formats `time` in UTC: `2026-10-10T12:00:00Z`.
pub(super) fn utc(time: SystemTime) -> String {
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    let (year, month, day) = civil(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// Returns the year, month and day of `days` since 1970-01-01. The
/// algorithm is `civil_from_days` from Howard Hinnant.
fn civil(days: u64) -> (u64, u64, u64) {
    let shifted = days + 719_468;
    let era = shifted / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Dates in UTC.
    #[test]
    fn dates() {
        assert_eq!(utc(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        assert_eq!(
            utc(UNIX_EPOCH + Duration::from_secs(1_700_000_000)),
            "2023-11-14T22:13:20Z"
        );
        assert_eq!(
            utc(UNIX_EPOCH + Duration::from_hours(264_384)),
            "2000-02-29T00:00:00Z"
        );
    }

    /// The cooldown starts at the later reset, from the server or from this
    /// process, and includes a margin.
    #[test]
    fn cooldowns() {
        let now = UNIX_EPOCH + Duration::from_secs(1_000_000);
        let ms = |seconds: u32| Some(f64::from(seconds) * 1000.0);
        assert_eq!(cooldown(None, None, now), None);
        assert_eq!(cooldown(ms(1_000_000 - 500), None, now), None);
        assert_eq!(
            cooldown(ms(1_000_000 - 100), None, now),
            Some(Duration::from_secs(90))
        );
        assert_eq!(
            cooldown(
                ms(1_000_000 - 100),
                Some(now - Duration::from_secs(10)),
                now
            ),
            Some(Duration::from_secs(180))
        );
        assert_eq!(cooldown(Some(f64::NAN), None, now), None);
    }
}
