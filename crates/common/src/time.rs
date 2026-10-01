use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Unix 纳秒时间戳包装。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Timestamp(i64);

impl Timestamp {
    pub const fn from_unix_nano(nanos: i64) -> Self {
        Self(nanos)
    }

    pub fn now() -> Self {
        let now = Utc::now();
        Self(now.timestamp() * 1_000_000_000 + now.timestamp_subsec_nanos() as i64)
    }

    pub fn from_datetime(dt: DateTime<Utc>) -> Self {
        Self(dt.timestamp_nanos_opt().unwrap_or(0))
    }

    pub fn unix_nano(self) -> i64 {
        self.0
    }

    pub fn to_datetime(self) -> Option<DateTime<Utc>> {
        DateTime::from_timestamp_nanos(self.0).into()
    }
}

impl std::fmt::Display for Timestamp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
