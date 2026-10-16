//! Why a web provider could not answer, in terms the pool can act on: move to the next account, rest this one, or
//! give up because asking again elsewhere would not help.

use std::fmt;
use std::time::Duration;

use reqwest::StatusCode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The service could not be reached, or its answer was unreadable.
    Network,
    /// The service failed on its side.
    Server,
    /// The account has no credits left.
    Exhausted,
    /// The key was refused.
    BadKey,
    /// Too many requests; try again after a while.
    RateLimited,
    /// The request itself was refused (bad arguments, an address it will not fetch): another account would say the same.
    Rejected,
}

impl Kind {
    /// Whether the next account should be tried.
    pub fn fails_over(self) -> bool {
        self != Kind::Rejected
    }

    /// Said to an agent when the account was passed over because of it.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Network => "could not be reached",
            Kind::Server => "had a server error",
            Kind::Exhausted => "is out of credits",
            Kind::BadKey => "refused its key",
            Kind::RateLimited => "is rate limited",
            Kind::Rejected => "refused the request",
        }
    }
}

/// How long an account that is out of credits, or whose key was refused, is left alone.
pub const REST_LONG: Duration = Duration::from_secs(10 * 60);
/// How long a rate-limited account rests when the service does not say.
pub const REST_RATE_LIMIT: Duration = Duration::from_secs(60);
/// The longest a service's own `Retry-After` is believed.
const REST_MAX: Duration = Duration::from_secs(60 * 60);

#[derive(Debug)]
pub struct ProviderError {
    pub kind: Kind,
    pub retry_after: Option<Duration>,
    pub message: String,
}

impl ProviderError {
    pub fn new(kind: Kind, message: impl Into<String>) -> Self {
        Self { kind, retry_after: None, message: message.into() }
    }

    /// How long the account should rest after this, if at all.
    pub fn rest(&self) -> Option<Duration> {
        match self.kind {
            Kind::Exhausted | Kind::BadKey => Some(REST_LONG),
            Kind::RateLimited => Some(self.retry_after.unwrap_or(REST_RATE_LIMIT).min(REST_MAX)),
            _ => None,
        }
    }
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ProviderError {}

/// What an HTTP status means to a provider that follows the usual conventions. A provider with its own codes (Tavily's
/// 432 and 433) checks those first.
pub fn classify(status: StatusCode) -> Kind {
    match status.as_u16() {
        401 | 403 => Kind::BadKey,
        402 => Kind::Exhausted,
        429 => Kind::RateLimited,
        500..=599 => Kind::Server,
        _ => Kind::Rejected,
    }
}

/// The seconds in a `Retry-After` header.
pub fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?.trim().parse::<u64>().ok().map(Duration::from_secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_say_whether_to_try_another_account() {
        let k = |c: u16| classify(StatusCode::from_u16(c).unwrap());
        assert_eq!((k(401), k(403), k(402), k(429), k(503), k(400), k(422)), (Kind::BadKey, Kind::BadKey, Kind::Exhausted, Kind::RateLimited, Kind::Server, Kind::Rejected, Kind::Rejected));
        assert!(Kind::Server.fails_over() && Kind::Network.fails_over() && !Kind::Rejected.fails_over());
    }

    #[test]
    fn only_some_failures_make_an_account_rest() {
        let e = |kind| ProviderError::new(kind, "x");
        assert_eq!(e(Kind::Exhausted).rest(), Some(REST_LONG));
        assert_eq!(e(Kind::BadKey).rest(), Some(REST_LONG));
        assert_eq!(e(Kind::Server).rest(), None);
        assert_eq!(e(Kind::RateLimited).rest(), Some(REST_RATE_LIMIT));
        let told = ProviderError { retry_after: Some(Duration::from_secs(5)), ..e(Kind::RateLimited) };
        assert_eq!(told.rest(), Some(Duration::from_secs(5)));
        let silly = ProviderError { retry_after: Some(Duration::from_secs(999_999)), ..e(Kind::RateLimited) };
        assert_eq!(silly.rest(), Some(REST_MAX));
    }
}
