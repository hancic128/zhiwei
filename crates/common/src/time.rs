use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Unix nanosecond timestamp wrapper.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Timestamp(i64);

impl Timestamp {
    #[must_use]
    pub const fn from_unix_nano(nanos: i64) -> Self {
        Self(nanos)
    }

    #[must_use]
    pub fn now() -> Self {
        let now = Utc::now();
        Self(now.timestamp() * 1_000_000_000 + i64::from(now.timestamp_subsec_nanos()))
    }

    #[must_use]
    pub fn from_datetime(dt: DateTime<Utc>) -> Self {
        Self(dt.timestamp_nanos_opt().unwrap_or(0))
    }

    #[must_use]
    pub const fn unix_nano(self) -> i64 {
        self.0
    }

    #[must_use]
    pub fn to_datetime(self) -> Option<DateTime<Utc>> {
        DateTime::from_timestamp_nanos(self.0).into()
    }
}

impl std::fmt::Display for Timestamp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
