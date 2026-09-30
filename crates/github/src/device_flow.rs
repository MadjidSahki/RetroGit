//! OAuth Device Flow as a pure state machine (no I/O, no clock).

use std::time::Duration;

use serde::Deserialize;

/// Response of `POST /login/device/code`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    /// Seconds until `device_code` expires.
    pub expires_in: u64,
    /// Minimum seconds between two polls.
    pub interval: u64,
}

/// Interpreted response of `POST /login/oauth/access_token`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PollResponse {
    Token(String),
    Pending,
    SlowDown,
    Expired,
    Denied,
    Other(String),
}

impl PollResponse {
    pub fn from_json(v: &serde_json::Value) -> PollResponse {
        if let Some(t) = v.get("access_token").and_then(|t| t.as_str()) {
            return PollResponse::Token(t.to_string());
        }
        match v.get("error").and_then(|e| e.as_str()) {
            Some("authorization_pending") => PollResponse::Pending,
            Some("slow_down") => PollResponse::SlowDown,
            Some("expired_token") => PollResponse::Expired,
            Some("access_denied") => PollResponse::Denied,
            Some(other) => PollResponse::Other(other.to_string()),
            None => PollResponse::Other("unexpected response".to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceFlowFailure {
    Expired,
    Denied,
    Other(String),
}

/// What the caller must do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Sleep this long, then poll again.
    Wait(Duration),
    Done(String),
    Failed(DeviceFlowFailure),
}

#[derive(Debug, Clone)]
pub struct DeviceFlow {
    interval: Duration,
    expires_in: Duration,
}

impl DeviceFlow {
    pub fn new(code: &DeviceCode) -> DeviceFlow {
        DeviceFlow {
            interval: Duration::from_secs(code.interval.max(1)),
            expires_in: Duration::from_secs(code.expires_in),
        }
    }

    /// Delay before the very first poll.
    pub fn first_wait(&self) -> Duration {
        self.interval
    }

    /// Feed the latest poll response. `elapsed` is the time since the device code was issued.
    pub fn on_response(&mut self, response: PollResponse, elapsed: Duration) -> Step {
        match response {
            PollResponse::Token(t) => Step::Done(t),
            PollResponse::Expired => Step::Failed(DeviceFlowFailure::Expired),
            PollResponse::Denied => Step::Failed(DeviceFlowFailure::Denied),
            PollResponse::Other(e) => Step::Failed(DeviceFlowFailure::Other(e)),
            PollResponse::Pending | PollResponse::SlowDown => {
                if matches!(response, PollResponse::SlowDown) {
                    self.interval += Duration::from_secs(5);
                }
                if elapsed + self.interval >= self.expires_in {
                    Step::Failed(DeviceFlowFailure::Expired)
                } else {
                    Step::Wait(self.interval)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn code(interval: u64, expires_in: u64) -> DeviceCode {
        DeviceCode {
            device_code: "dc".into(),
            user_code: "ABCD-1234".into(),
            verification_uri: "https://github.com/login/device".into(),
            expires_in,
            interval,
        }
    }

    const S: fn(u64) -> Duration = Duration::from_secs;

    #[test]
    fn pending_waits_for_interval() {
        let mut f = DeviceFlow::new(&code(5, 900));
        assert_eq!(f.first_wait(), S(5));
        assert_eq!(f.on_response(PollResponse::Pending, S(5)), Step::Wait(S(5)));
    }

    #[test]
    fn slow_down_adds_five_seconds_permanently() {
        let mut f = DeviceFlow::new(&code(5, 900));
        assert_eq!(
            f.on_response(PollResponse::SlowDown, S(5)),
            Step::Wait(S(10))
        );
        assert_eq!(
            f.on_response(PollResponse::Pending, S(15)),
            Step::Wait(S(10))
        );
    }

    #[test]
    fn zero_interval_is_clamped_to_one_second() {
        assert_eq!(DeviceFlow::new(&code(0, 900)).first_wait(), S(1));
    }

    #[test]
    fn token_finishes() {
        let mut f = DeviceFlow::new(&code(5, 900));
        assert_eq!(
            f.on_response(PollResponse::Token("gho_x".into()), S(5)),
            Step::Done("gho_x".into())
        );
    }

    #[test]
    fn terminal_errors_fail() {
        let mut f = DeviceFlow::new(&code(5, 900));
        assert_eq!(
            f.on_response(PollResponse::Expired, S(5)),
            Step::Failed(DeviceFlowFailure::Expired)
        );
        assert_eq!(
            f.on_response(PollResponse::Denied, S(5)),
            Step::Failed(DeviceFlowFailure::Denied)
        );
        assert_eq!(
            f.on_response(
                PollResponse::Other("incorrect_client_credentials".into()),
                S(5)
            ),
            Step::Failed(DeviceFlowFailure::Other(
                "incorrect_client_credentials".into()
            ))
        );
    }

    #[test]
    fn pending_past_expiry_fails_without_polling_again() {
        let mut f = DeviceFlow::new(&code(5, 900));
        assert_eq!(
            f.on_response(PollResponse::Pending, S(896)),
            Step::Failed(DeviceFlowFailure::Expired)
        );
    }

    #[test]
    fn parses_poll_json() {
        assert_eq!(
            PollResponse::from_json(&json!({"access_token":"gho_1","token_type":"bearer"})),
            PollResponse::Token("gho_1".into())
        );
        assert_eq!(
            PollResponse::from_json(&json!({"error":"authorization_pending"})),
            PollResponse::Pending
        );
        assert_eq!(
            PollResponse::from_json(&json!({"error":"slow_down","interval":10})),
            PollResponse::SlowDown
        );
        assert_eq!(
            PollResponse::from_json(&json!({"error":"expired_token"})),
            PollResponse::Expired
        );
        assert_eq!(
            PollResponse::from_json(&json!({"error":"access_denied"})),
            PollResponse::Denied
        );
        assert_eq!(
            PollResponse::from_json(&json!({"error":"device_flow_disabled"})),
            PollResponse::Other("device_flow_disabled".into())
        );
        assert_eq!(
            PollResponse::from_json(&json!({})),
            PollResponse::Other("unexpected response".into())
        );
    }
}
