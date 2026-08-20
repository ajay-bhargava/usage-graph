//! Codex remaining-usage fetch and payload parsing.

use crate::cli::Provider;
use crate::store::StoredAccount;
use eyre::{Result, WrapErr, eyre};
use reqwest::blocking::Client;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue, USER_AGENT};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::time::Duration;

/// `ChatGPT` backend usage endpoint used by Codex.
const CODEX_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
/// Timeout for establishing the usage-status connection.
const LIMIT_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// Whole-request timeout for usage-status refreshes.
const LIMIT_REQUEST_TIMEOUT: Duration = Duration::from_secs(4);
/// User agent used for backend compatibility.
const CODEX_USER_AGENT: &str = "codex-cli";
/// Product selector for Codex quota windows.
const CODEX_PRODUCT_SKU: &str = "codex";
/// Codex short rolling limit window in minutes.
const CODEX_FIVE_HOUR_WINDOW_MINUTES: i64 = 5 * 60;
/// Codex weekly rolling limit window in minutes.
const CODEX_WEEKLY_WINDOW_MINUTES: i64 = 7 * 24 * 60;

/// HTTP transport used to fetch Codex usage JSON.
pub(crate) trait CodexUsageTransport {
    /// Fetch one usage payload body.
    ///
    /// Returns an error when the HTTP client cannot be built or the request fails
    /// before a status is available.
    fn get_usage(&self, account: &StoredAccount) -> Result<UsageHttpResponse>;
}

/// Live blocking transport.
pub(crate) struct LiveCodexUsageTransport;

impl CodexUsageTransport for LiveCodexUsageTransport {
    fn get_usage(&self, account: &StoredAccount) -> Result<UsageHttpResponse> {
        let client = Client::builder()
            .connect_timeout(LIMIT_CONNECT_TIMEOUT)
            .timeout(LIMIT_REQUEST_TIMEOUT)
            .build()
            .wrap_err("failed to build usage HTTP client")?;
        let headers = usage_request_headers(&account.access, account.account_id.as_deref())
            .ok_or_else(|| eyre!("invalid access token or account id header"))?;
        let response = client
            .get(CODEX_USAGE_URL)
            .headers(headers)
            .send()
            .wrap_err("Codex usage request failed")?;
        let status = response.status().as_u16();
        let body = response
            .text()
            .wrap_err("failed to read Codex usage response")?;
        Ok(UsageHttpResponse { status, body })
    }
}

/// Raw HTTP result from the usage endpoint.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UsageHttpResponse {
    /// Status code.
    pub(crate) status: u16,
    /// Response body.
    pub(crate) body: String,
}

/// Remaining-usage snapshot for one stored account.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AccountQuota {
    /// Local account alias.
    pub(crate) name: String,
    /// Provider identifier.
    pub(crate) provider: Provider,
    /// Optional plan name from the usage payload.
    pub(crate) plan: Option<String>,
    /// Displayable quota windows.
    pub(crate) windows: Vec<QuotaWindow>,
    /// Non-fatal fetch or parse failure.
    pub(crate) error: Option<QuotaError>,
}

/// One displayable quota window.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct QuotaWindow {
    /// Bucket id such as `codex` or `spark`.
    pub(crate) bucket: String,
    /// Row label.
    pub(crate) label: String,
    /// Percentage of the window that has been used.
    pub(crate) used_percent: f64,
    /// Window width in whole minutes, when reported.
    pub(crate) window_minutes: Option<i64>,
    /// Unix timestamp for the reset time, when reported.
    pub(crate) resets_at_epoch_seconds: Option<i64>,
}

/// Non-fatal reasons quota bars cannot be shown.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum QuotaError {
    /// Fetching is disabled by `--offline`.
    Offline,
    /// Provider is not implemented yet.
    UnsupportedProvider,
    /// The backend request failed before a usable response was received.
    RequestFailed,
    /// The backend rejected the current access token.
    Unauthorized,
    /// The backend response was not valid usage JSON.
    InvalidResponse,
    /// The response did not include a rate-limit window.
    NoLimitData,
}

impl QuotaError {
    /// Return a short display reason.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Offline => "offline",
            Self::UnsupportedProvider => "unsupported provider",
            Self::RequestFailed => "request failed",
            Self::Unauthorized => "unauthorized",
            Self::InvalidResponse => "invalid response",
            Self::NoLimitData => "no limit data",
        }
    }
}

/// Usage endpoint payload subset.
#[derive(Debug, Deserialize)]
struct UsagePayload {
    /// Optional plan identifier.
    #[serde(default)]
    plan_type: Option<String>,
    /// Protocol-shaped preferred rate-limit snapshot.
    #[serde(default, alias = "rateLimits")]
    rate_limits: Option<ProtocolRateLimitSnapshot>,
    /// Protocol-shaped snapshots keyed by limit id.
    #[serde(default, alias = "rateLimitsByLimitId")]
    rate_limits_by_limit_id: Option<BTreeMap<String, ProtocolRateLimitSnapshot>>,
    /// Primary Codex rate-limit details.
    #[serde(default, alias = "rateLimit")]
    rate_limit: Option<RateLimitDetails>,
    /// Additional metered-feature limits.
    #[serde(default, alias = "additionalRateLimits")]
    additional_rate_limits: Option<Vec<AdditionalRateLimit>>,
}

/// Protocol-shaped rate-limit snapshot.
#[derive(Clone, Debug, Deserialize)]
struct ProtocolRateLimitSnapshot {
    /// Backend limit identifier.
    #[serde(default, alias = "limitId")]
    limit_id: Option<String>,
    /// Short rolling window.
    #[serde(default)]
    primary: Option<ProtocolRateLimitWindow>,
    /// Long rolling window.
    #[serde(default)]
    secondary: Option<ProtocolRateLimitWindow>,
}

/// Protocol-shaped rate-limit window.
#[derive(Clone, Copy, Debug, Deserialize)]
struct ProtocolRateLimitWindow {
    /// Used percentage.
    #[serde(default, alias = "usedPercent")]
    used_percent: Option<f64>,
    /// Window width in minutes.
    #[serde(default, alias = "windowDurationMins")]
    window_duration_mins: Option<i64>,
    /// Reset timestamp.
    #[serde(default, alias = "resetsAt")]
    resets_at: Option<i64>,
}

/// Additional usage-limit payload.
#[derive(Debug, Deserialize)]
struct AdditionalRateLimit {
    /// Backend metered-feature identifier.
    #[serde(default, alias = "meteredFeature")]
    metered_feature: Option<String>,
    /// Human-readable limit identifier.
    #[serde(default, alias = "limitName")]
    limit_name: Option<String>,
    /// Rate-limit windows for this feature.
    #[serde(default, alias = "rateLimit")]
    rate_limit: Option<RateLimitDetails>,
}

/// Primary and secondary rate-limit windows.
#[derive(Clone, Copy, Debug, Deserialize)]
struct RateLimitDetails {
    /// Short rolling window.
    #[serde(default, alias = "primaryWindow", alias = "primary")]
    primary_window: Option<RateLimitWindowPayload>,
    /// Long rolling window.
    #[serde(default, alias = "secondaryWindow", alias = "secondary")]
    secondary_window: Option<RateLimitWindowPayload>,
}

/// Backend representation for one rate-limit window.
#[derive(Clone, Copy, Debug, Deserialize)]
struct RateLimitWindowPayload {
    /// Used percentage.
    #[serde(default, alias = "usedPercent")]
    used_percent: Option<f64>,
    /// Window width in seconds.
    #[serde(default, alias = "limitWindowSeconds")]
    limit_window_seconds: Option<i64>,
    /// Reset timestamp.
    #[serde(default, alias = "resetAt", alias = "resetsAt")]
    reset_at: Option<i64>,
}

/// Parsed window before labels are assigned.
#[derive(Clone, Copy, Debug, PartialEq)]
struct RawWindow {
    /// Percentage of the window that has been used.
    used_percent: f64,
    /// Window width in whole minutes, when reported.
    window_minutes: Option<i64>,
    /// Unix timestamp for the reset time, when reported.
    resets_at_epoch_seconds: Option<i64>,
}

/// Parse a usage JSON body into quota windows.
pub(crate) fn quota_from_usage_payload(
    raw: &str,
) -> Result<(Option<String>, Vec<QuotaWindow>), QuotaError> {
    let payload: UsagePayload =
        serde_json::from_str(raw).map_err(|_| QuotaError::InvalidResponse)?;
    let plan = payload
        .plan_type
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let windows = windows_from_payload(payload);
    if windows.is_empty() {
        Err(QuotaError::NoLimitData)
    } else {
        Ok((plan, windows))
    }
}

/// Convert a usage payload into labeled windows.
fn windows_from_payload(payload: UsagePayload) -> Vec<QuotaWindow> {
    let mut buckets: BTreeMap<String, Vec<RawWindow>> = BTreeMap::new();
    if let Some(by_id) = payload.rate_limits_by_limit_id {
        for (key, snapshot) in by_id {
            let bucket = normalize_bucket_id(snapshot.limit_id.as_deref().unwrap_or(&key));
            append_protocol_windows(buckets.entry(bucket).or_default(), &snapshot);
        }
    }
    if let Some(snapshot) = payload.rate_limits {
        let bucket = normalize_bucket_id(snapshot.limit_id.as_deref().unwrap_or("codex"));
        append_protocol_windows(buckets.entry(bucket).or_default(), &snapshot);
    }
    if let Some(details) = payload.rate_limit {
        append_backend_windows(buckets.entry("codex".to_string()).or_default(), details);
    }
    if let Some(additional_limits) = payload.additional_rate_limits {
        for limit in additional_limits {
            let Some(details) = limit.rate_limit else {
                continue;
            };
            let raw_id = [
                limit.limit_name.as_deref().unwrap_or_default(),
                limit.metered_feature.as_deref().unwrap_or_default(),
            ]
            .join(" ");
            let bucket = normalize_bucket_id(&raw_id);
            append_backend_windows(buckets.entry(bucket).or_default(), details);
        }
    }

    let mut windows = Vec::new();
    for bucket in ["codex", "spark"] {
        if let Some(raw_windows) = buckets.remove(bucket) {
            windows.extend(classify_bucket_windows(bucket, raw_windows));
        }
    }
    for (bucket, raw_windows) in buckets {
        windows.extend(classify_bucket_windows(&bucket, raw_windows));
    }
    windows
}

/// Normalize a backend limit id to `codex`, `spark`, or a lowercase token.
fn normalize_bucket_id(raw: &str) -> String {
    let lowered = raw.trim().to_ascii_lowercase();
    if lowered.contains("spark") {
        "spark".to_string()
    } else if lowered.is_empty() || lowered.contains("codex") {
        "codex".to_string()
    } else {
        lowered
    }
}

/// Append protocol windows onto a bucket.
fn append_protocol_windows(windows: &mut Vec<RawWindow>, snapshot: &ProtocolRateLimitSnapshot) {
    windows.extend(
        [snapshot.primary, snapshot.secondary]
            .into_iter()
            .filter_map(protocol_window),
    );
}

/// Append backend windows onto a bucket.
fn append_backend_windows(windows: &mut Vec<RawWindow>, details: RateLimitDetails) {
    windows.extend(
        [details.primary_window, details.secondary_window]
            .into_iter()
            .filter_map(backend_window),
    );
}

/// Convert one protocol-shaped window.
fn protocol_window(window: Option<ProtocolRateLimitWindow>) -> Option<RawWindow> {
    let window = window?;
    let used_percent = window.used_percent?;
    if !used_percent.is_finite() {
        return None;
    }
    Some(RawWindow {
        used_percent: used_percent.clamp(0.0, 100.0),
        window_minutes: window.window_duration_mins.filter(|minutes| *minutes > 0),
        resets_at_epoch_seconds: window.resets_at,
    })
}

/// Convert one backend window.
fn backend_window(window: Option<RateLimitWindowPayload>) -> Option<RawWindow> {
    let window = window?;
    let used_percent = window.used_percent?;
    if !used_percent.is_finite() {
        return None;
    }
    Some(RawWindow {
        used_percent: used_percent.clamp(0.0, 100.0),
        window_minutes: window
            .limit_window_seconds
            .filter(|seconds| *seconds > 0)
            .map(|seconds| (seconds + 59) / 60),
        resets_at_epoch_seconds: window.reset_at,
    })
}

/// Classify raw windows into 5h / weekly rows for one bucket.
fn classify_bucket_windows(bucket: &str, windows: Vec<RawWindow>) -> Vec<QuotaWindow> {
    let mut five_hour = None;
    let mut weekly = None;
    let mut unknown = Vec::new();
    for window in windows {
        match window.window_minutes {
            Some(CODEX_FIVE_HOUR_WINDOW_MINUTES) if five_hour.is_none() => {
                five_hour = Some(window);
            }
            Some(CODEX_WEEKLY_WINDOW_MINUTES) if weekly.is_none() => weekly = Some(window),
            Some(_) => {}
            None => unknown.push(window),
        }
    }
    let mut unknown = unknown.into_iter();
    if five_hour.is_none() {
        five_hour = unknown.next();
    }
    if weekly.is_none() {
        weekly = unknown.next();
    }

    let prefix = if bucket == "codex" {
        String::new()
    } else {
        let mut label = bucket.to_string();
        if let Some(first) = label.get_mut(0..1) {
            first.make_ascii_uppercase();
        }
        format!("{label} ")
    };
    [
        (five_hour, format!("{prefix}5h")),
        (weekly, format!("{prefix}Weekly")),
    ]
    .into_iter()
    .filter_map(|(window, label)| {
        window.map(|window| QuotaWindow {
            bucket: bucket.to_string(),
            label,
            used_percent: window.used_percent,
            window_minutes: window.window_minutes,
            resets_at_epoch_seconds: window.resets_at_epoch_seconds,
        })
    })
    .collect()
}

/// Build headers for one usage request.
fn usage_request_headers(access_token: &str, account_id: Option<&str>) -> Option<HeaderMap> {
    let mut headers = HeaderMap::new();
    let auth = HeaderValue::from_str(&format!("Bearer {access_token}")).ok()?;
    headers.insert(AUTHORIZATION, auth);
    headers.insert(USER_AGENT, HeaderValue::from_static(CODEX_USER_AGENT));
    headers.insert(
        "OAI-Product-Sku",
        HeaderValue::from_static(CODEX_PRODUCT_SKU),
    );
    if let Some(account_id) = account_id {
        let account = HeaderValue::from_str(account_id).ok()?;
        headers.insert("ChatGPT-Account-ID", account);
    }
    Some(headers)
}

/// Map an HTTP status onto a quota error when the body should not be parsed.
#[must_use]
pub(crate) fn quota_error_from_status(status: u16) -> Option<QuotaError> {
    match status {
        200..=299 => None,
        401 | 403 => Some(QuotaError::Unauthorized),
        _ => Some(QuotaError::RequestFailed),
    }
}

#[cfg(test)]
mod tests {
    use super::{QuotaError, quota_from_usage_payload};

    #[test]
    fn usage_payload_maps_primary_and_secondary_windows() {
        let (plan, windows) = quota_from_usage_payload(
            r#"{
                "plan_type": "pro",
                "rate_limit": {
                    "primary_window": {
                        "used_percent": 42,
                        "limit_window_seconds": 18000,
                        "reset_at": 1770000000
                    },
                    "secondary_window": {
                        "used_percent": 9,
                        "limit_window_seconds": 604800,
                        "reset_at": 1770100000
                    }
                }
            }"#,
        )
        .expect("payload");
        assert_eq!(plan.as_deref(), Some("pro"));
        assert_eq!(windows.len(), 2, "codex 5h and weekly windows");
        assert_eq!(windows[0].label, "5h");
        assert!((windows[0].used_percent - 42.0).abs() < f64::EPSILON);
        assert_eq!(windows[1].label, "Weekly");
        assert_eq!(windows[1].window_minutes, Some(10_080));
    }

    #[test]
    fn usage_payload_includes_spark_additional_limits() {
        let (_plan, windows) = quota_from_usage_payload(
            r#"{
                "rate_limit": {
                    "primary_window": {"used_percent": 42, "limit_window_seconds": 18000}
                },
                "additional_rate_limits": [
                    {
                        "metered_feature": "spark",
                        "rate_limit": {
                            "primary_window": {"used_percent": 10, "limit_window_seconds": 18000},
                            "secondary_window": {"used_percent": 3, "limit_window_seconds": 604800}
                        }
                    }
                ]
            }"#,
        )
        .expect("payload");
        let labels = windows
            .iter()
            .map(|window| window.label.as_str())
            .collect::<Vec<_>>();
        assert_eq!(labels, ["5h", "Spark 5h", "Spark Weekly"]);
    }

    #[test]
    fn usage_payload_maps_protocol_rate_limits_by_id() {
        let (_plan, windows) = quota_from_usage_payload(
            r#"{
                "rateLimitsByLimitId": {
                    "other": {"limitId": "other", "primary": {"usedPercent": 90}},
                    "codex": {
                        "limitId": "codex",
                        "primary": {"usedPercent": 7, "windowDurationMins": 300},
                        "secondary": {"usedPercent": 2, "windowDurationMins": 10080}
                    }
                }
            }"#,
        )
        .expect("payload");
        assert!(!windows.is_empty(), "codex windows are present");
        assert!((windows[0].used_percent - 7.0).abs() < f64::EPSILON);
        assert!(windows.len() >= 2, "weekly window is present");
        assert_eq!(windows[1].label, "Weekly");
        assert!(
            windows.iter().any(|window| window.bucket == "other"),
            "unknown buckets are preserved after codex/spark"
        );
    }

    #[test]
    fn usage_payload_reports_invalid_json() {
        assert_eq!(
            quota_from_usage_payload("not-json").expect_err("invalid json"),
            QuotaError::InvalidResponse
        );
    }

    #[test]
    fn usage_payload_reports_missing_windows() {
        assert_eq!(
            quota_from_usage_payload(r#"{"rate_limit":{}}"#).expect_err("missing windows"),
            QuotaError::NoLimitData
        );
    }
}
