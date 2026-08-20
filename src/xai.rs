//! xAI device-code login and remaining-credits fetch.

use crate::cli::Provider;
use crate::oauth::OAuthTokenSet;
use crate::quota::{QuotaError, QuotaWindow, UsageHttpResponse};
use crate::store::StoredAccount;
use eyre::{Result, WrapErr, eyre};
use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue, USER_AGENT};
use serde::Deserialize;
use std::time::Duration;

/// xAI OAuth application id used by Pi / Grok CLI.
const CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";
/// OAuth scopes requested for Grok CLI access.
const SCOPE: &str = "openid profile email offline_access grok-cli:access api:access";
/// Device-code start endpoint.
const DEVICE_CODE_URL: &str = "https://auth.x.ai/oauth2/device/code";
/// Token endpoint used for polling, exchange, and refresh.
const TOKEN_URL: &str = "https://auth.x.ai/oauth2/token";
/// Remaining-credits endpoint used by Grok CLI.
const BILLING_URL: &str = "https://cli-chat-proxy.grok.com/v1/billing?format=credits";
/// Settings endpoint that carries the human plan name.
const SETTINGS_URL: &str = "https://cli-chat-proxy.grok.com/v1/settings";
/// Token-auth marker required by the CLI proxy.
const XAI_TOKEN_AUTH: &str = "xai-grok-cli";
/// User agent for this CLI.
const USAGE_USER_AGENT: &str = "usage-cli";
/// Refresh slightly before the reported expiry.
const REFRESH_SKEW_MS: i64 = 5 * 60 * 1000;
/// Default access-token lifetime when `expires_in` is omitted.
const DEFAULT_TOKEN_LIFETIME_SECONDS: i64 = 3600;
/// Connect timeout for billing and settings.
const LIMIT_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// Whole-request timeout for billing.
const BILLING_TIMEOUT: Duration = Duration::from_secs(4);
/// Whole-request timeout for settings enrichment.
const SETTINGS_TIMEOUT: Duration = Duration::from_secs(2);
/// OIDC scope prefix used by `grok login`.
const GROK_OIDC_SCOPE_PREFIX: &str = "https://auth.x.ai::";
/// Legacy session scope used by older `grok login` files.
const GROK_LEGACY_SESSION_SCOPE: &str = "https://accounts.x.ai/sign-in";

/// Device-code session returned by xAI.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct XaiDeviceSession {
    /// RFC 8628 device code.
    pub(crate) device_code: String,
    /// User-visible code.
    pub(crate) user_code: String,
    /// Browser verification URL.
    pub(crate) verification_uri: String,
    /// Polling interval.
    pub(crate) interval: Duration,
    /// Login timeout.
    pub(crate) expires_in: Duration,
}

/// Result of one xAI device-code poll.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum XaiDevicePoll {
    /// User has not finished authorizing.
    Pending,
    /// Caller should wait longer between polls.
    SlowDown,
    /// Authorization succeeded.
    Complete(OAuthTokenSet),
}

/// xAI OAuth operations used by login and refresh.
pub(crate) trait XaiOAuth {
    /// Start a device-code login.
    ///
    /// # Errors
    ///
    /// Returns an error when the device-code endpoint cannot be reached or parsed.
    fn start_device_auth(&self) -> Result<XaiDeviceSession>;
    /// Poll an in-progress device-code login.
    ///
    /// # Errors
    ///
    /// Returns an error when the poll request fails for a non-pending reason.
    fn poll_device_auth(&self, session: &XaiDeviceSession) -> Result<XaiDevicePoll>;
    /// Refresh an access token.
    ///
    /// # Errors
    ///
    /// Returns an error when refresh is rejected.
    fn refresh_access_token(&self, refresh_token: &str) -> Result<OAuthTokenSet>;
}

/// HTTP transport used to fetch xAI billing JSON.
pub(crate) trait XaiUsageTransport {
    /// Fetch the credits payload.
    ///
    /// # Errors
    ///
    /// Returns an error when the HTTP client cannot be built or the request fails
    /// before a status is available.
    fn get_billing(&self, account: &StoredAccount) -> Result<UsageHttpResponse>;
    /// Fetch optional settings used for the plan name.
    ///
    /// # Errors
    ///
    /// Returns an error when the HTTP client cannot be built or the request fails
    /// before a status is available.
    fn get_settings(&self, account: &StoredAccount) -> Result<UsageHttpResponse>;
}

/// Blocking xAI OAuth client.
pub(crate) struct LiveXaiOAuth;

impl XaiOAuth for LiveXaiOAuth {
    fn start_device_auth(&self) -> Result<XaiDeviceSession> {
        let client = http_client(Duration::from_secs(15))?;
        let response = client
            .post(DEVICE_CODE_URL)
            .form(&[
                ("client_id", CLIENT_ID),
                ("scope", SCOPE),
                ("referrer", "usage-cli"),
            ])
            .send()
            .wrap_err("failed to start xAI device login")?;
        if !response.status().is_success() {
            return Err(eyre!(
                "xAI device login start failed ({})",
                response.status()
            ));
        }
        let payload: DeviceStartPayload = response
            .json()
            .wrap_err("invalid xAI device login start payload")?;
        payload.into_session()
    }

    fn poll_device_auth(&self, session: &XaiDeviceSession) -> Result<XaiDevicePoll> {
        let client = http_client(Duration::from_secs(15))?;
        let response = client
            .post(TOKEN_URL)
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("client_id", CLIENT_ID),
                ("device_code", session.device_code.as_str()),
            ])
            .send()
            .wrap_err("failed to poll xAI device login")?;
        if response.status().is_success() {
            let payload: TokenPayload = response
                .json()
                .wrap_err("invalid xAI device login token payload")?;
            let tokens = token_set_from_payload(payload, now_unix_ms()?, None)?;
            return Ok(XaiDevicePoll::Complete(tokens));
        }
        let body = response.text().unwrap_or_default();
        if body.contains("authorization_pending") {
            return Ok(XaiDevicePoll::Pending);
        }
        if body.contains("slow_down") {
            return Ok(XaiDevicePoll::SlowDown);
        }
        if body.contains("expired_token") {
            return Err(eyre!("xAI device code expired"));
        }
        if body.contains("access_denied") || body.contains("authorization_denied") {
            return Err(eyre!("xAI device authorization was denied"));
        }
        Err(eyre!("xAI device login poll failed: {body}"))
    }

    fn refresh_access_token(&self, refresh_token: &str) -> Result<OAuthTokenSet> {
        let client = http_client(Duration::from_secs(15))?;
        let response = client
            .post(TOKEN_URL)
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", CLIENT_ID),
                ("refresh_token", refresh_token),
            ])
            .send()
            .wrap_err("xAI token refresh failed")?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            return Err(eyre!("xAI token refresh failed ({status}): {body}"));
        }
        let payload: TokenPayload = response.json().wrap_err("invalid xAI token payload")?;
        token_set_from_payload(payload, now_unix_ms()?, Some(refresh_token))
    }
}

/// Live blocking billing transport.
pub(crate) struct LiveXaiUsageTransport;

impl XaiUsageTransport for LiveXaiUsageTransport {
    fn get_billing(&self, account: &StoredAccount) -> Result<UsageHttpResponse> {
        proxy_get(BILLING_URL, BILLING_TIMEOUT, account)
    }

    fn get_settings(&self, account: &StoredAccount) -> Result<UsageHttpResponse> {
        proxy_get(SETTINGS_URL, SETTINGS_TIMEOUT, account)
    }
}

/// GET one CLI-proxy JSON endpoint.
fn proxy_get(url: &str, timeout: Duration, account: &StoredAccount) -> Result<UsageHttpResponse> {
    let client = Client::builder()
        .connect_timeout(LIMIT_CONNECT_TIMEOUT)
        .timeout(timeout)
        .build()
        .wrap_err("failed to build xAI HTTP client")?;
    let headers =
        proxy_headers(&account.access).ok_or_else(|| eyre!("invalid xAI access token header"))?;
    let response = client
        .get(url)
        .headers(headers)
        .send()
        .wrap_err("xAI usage request failed")?;
    let status = response.status().as_u16();
    let body = response
        .text()
        .wrap_err("failed to read xAI usage response")?;
    Ok(UsageHttpResponse { status, body })
}

/// Build headers for the Grok CLI proxy.
fn proxy_headers(access_token: &str) -> Option<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {access_token}")).ok()?,
    );
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    headers.insert(USER_AGENT, HeaderValue::from_static(USAGE_USER_AGENT));
    headers.insert("x-xai-token-auth", HeaderValue::from_static(XAI_TOKEN_AUTH));
    Some(headers)
}

/// Device start payload.
#[derive(Debug, Deserialize)]
struct DeviceStartPayload {
    /// RFC 8628 device code.
    device_code: Option<String>,
    /// User code.
    user_code: Option<String>,
    /// Browser URL.
    verification_uri: Option<String>,
    /// URL that already includes the user code.
    verification_uri_complete: Option<String>,
    /// Poll interval in seconds.
    interval: Option<u64>,
    /// Lifetime in seconds.
    expires_in: Option<u64>,
}

impl DeviceStartPayload {
    /// Convert a successful start payload into a session.
    fn into_session(self) -> Result<XaiDeviceSession> {
        let device_code = required_text(self.device_code, "device_code")?;
        let user_code = required_text(self.user_code, "user_code")?;
        let verification_uri = https_uri(
            self.verification_uri_complete
                .or(self.verification_uri)
                .as_deref(),
        )?;
        Ok(XaiDeviceSession {
            device_code,
            user_code,
            verification_uri,
            interval: Duration::from_secs(self.interval.filter(|value| *value > 0).unwrap_or(5)),
            expires_in: Duration::from_secs(
                self.expires_in.filter(|value| *value > 0).unwrap_or(900),
            ),
        })
    }
}

/// Token endpoint payload.
#[derive(Debug, Deserialize)]
struct TokenPayload {
    /// Access token.
    access_token: Option<String>,
    /// Refresh token.
    refresh_token: Option<String>,
    /// Lifetime in seconds.
    expires_in: Option<u64>,
}

/// Build a token set from a token-endpoint payload.
fn token_set_from_payload(
    payload: TokenPayload,
    now_ms: i64,
    previous_refresh: Option<&str>,
) -> Result<OAuthTokenSet> {
    let access = required_text(payload.access_token, "access_token")?;
    let refresh = payload
        .refresh_token
        .filter(|value| !value.is_empty())
        .or_else(|| previous_refresh.map(ToOwned::to_owned))
        .ok_or_else(|| eyre!("xAI token response missing refresh_token"))?;
    let expires_in = i64::try_from(
        payload
            .expires_in
            .unwrap_or(u64::try_from(DEFAULT_TOKEN_LIFETIME_SECONDS).unwrap_or(3600)),
    )
    .unwrap_or(DEFAULT_TOKEN_LIFETIME_SECONDS);
    let expires = now_ms.saturating_add(expires_in.saturating_mul(1000) - REFRESH_SKEW_MS);
    Ok(OAuthTokenSet {
        access,
        refresh,
        expires,
        account_id: None,
    })
}

/// Credits payload from `/v1/billing?format=credits`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreditsResponse {
    /// Nested config object.
    #[serde(default)]
    config: Option<CreditsConfig>,
    /// Top-level plan name.
    #[serde(default, alias = "subscription_tier")]
    subscription_tier: Option<String>,
}

/// Credits config object.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreditsConfig {
    /// Used percentage of the included pool.
    #[serde(default, alias = "credit_usage_percent")]
    credit_usage_percent: Option<f64>,
    /// Current billing window.
    #[serde(default, alias = "current_period")]
    current_period: Option<CurrentPeriod>,
    /// Period end when `current_period` is absent.
    #[serde(default, alias = "billing_period_end")]
    billing_period_end: Option<String>,
    /// On-demand cap.
    #[serde(default, alias = "on_demand_cap")]
    on_demand_cap: Option<CreditsAmount>,
    /// On-demand used.
    #[serde(default, alias = "on_demand_used")]
    on_demand_used: Option<CreditsAmount>,
    /// Plan name on the config object.
    #[serde(default, alias = "subscription_tier")]
    subscription_tier: Option<String>,
}

/// Current credits window.
#[derive(Debug, Deserialize)]
struct CurrentPeriod {
    /// Inclusive/exclusive start timestamp.
    #[serde(default)]
    start: Option<String>,
    /// Reset timestamp.
    #[serde(default)]
    end: Option<String>,
}

/// `{ "val": <number> }` amount wrapper.
#[derive(Debug, Deserialize)]
struct CreditsAmount {
    /// Numeric amount.
    #[serde(default)]
    val: Option<f64>,
}

/// Settings payload used for the plan overlay.
#[derive(Debug, Deserialize)]
struct SettingsResponse {
    /// Human plan name.
    #[serde(default, alias = "subscriptionTierDisplay")]
    subscription_tier_display: Option<String>,
}

/// Parse remaining-credits JSON into quota windows.
pub(crate) fn quota_from_billing_payload(
    raw: &str,
) -> Result<(Option<String>, Vec<QuotaWindow>), QuotaError> {
    let payload: CreditsResponse =
        serde_json::from_str(raw).map_err(|_| QuotaError::InvalidResponse)?;
    let Some(config) = payload.config else {
        return Err(QuotaError::NoLimitData);
    };
    let resets_at = config
        .current_period
        .as_ref()
        .and_then(|period| period.end.as_deref())
        .or(config.billing_period_end.as_deref())
        .and_then(parse_iso8601_epoch_seconds);
    let window_minutes = window_minutes_from_period(
        config
            .current_period
            .as_ref()
            .and_then(|period| period.start.as_deref()),
        config
            .current_period
            .as_ref()
            .and_then(|period| period.end.as_deref())
            .or(config.billing_period_end.as_deref()),
    );
    let used_percent = if let Some(percent) = config
        .credit_usage_percent
        .filter(|value| value.is_finite())
    {
        percent.clamp(0.0, 100.0)
    } else if let (Some(cap), Some(used)) = (
        config.on_demand_cap.as_ref().and_then(|amount| amount.val),
        config.on_demand_used.as_ref().and_then(|amount| amount.val),
    ) {
        if cap > 0.0 && cap.is_finite() && used.is_finite() {
            (used / cap * 100.0).clamp(0.0, 100.0)
        } else if resets_at.is_some() {
            0.0
        } else {
            return Err(QuotaError::NoLimitData);
        }
    } else if resets_at.is_some() {
        0.0
    } else {
        return Err(QuotaError::NoLimitData);
    };
    let plan = first_nonempty([
        config.subscription_tier.as_deref(),
        payload.subscription_tier.as_deref(),
    ]);
    let label = window_label(window_minutes);
    Ok((
        plan,
        vec![QuotaWindow {
            bucket: "xai".to_string(),
            label,
            used_percent,
            window_minutes,
            resets_at_epoch_seconds: resets_at,
        }],
    ))
}

/// Parse a plan name from settings JSON. Missing or invalid bodies yield `None`.
#[must_use]
pub(crate) fn plan_from_settings_payload(raw: &str) -> Option<String> {
    let payload: SettingsResponse = serde_json::from_str(raw).ok()?;
    payload
        .subscription_tier_display
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Choose a row label from the period length.
fn window_label(window_minutes: Option<i64>) -> String {
    match window_minutes {
        Some(minutes) if minutes <= 8 * 24 * 60 => "Weekly".to_string(),
        Some(minutes) if minutes <= 40 * 24 * 60 => "Monthly".to_string(),
        _ => "Credits".to_string(),
    }
}

/// Convert a period start/end pair into whole minutes.
fn window_minutes_from_period(start: Option<&str>, end: Option<&str>) -> Option<i64> {
    let start = parse_iso8601_epoch_seconds(start?)?;
    let end = parse_iso8601_epoch_seconds(end?)?;
    let delta = end.saturating_sub(start);
    (delta > 0).then_some((delta + 59) / 60)
}

/// Parse an RFC 3339 timestamp to Unix seconds.
fn parse_iso8601_epoch_seconds(raw: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(raw.trim())
        .ok()
        .map(|value| value.timestamp())
}

/// Import xAI tokens from a Pi auth file or Grok CLI `auth.json`.
///
/// # Errors
///
/// Returns an error when the file is not valid JSON or contains no xAI OAuth tokens.
pub(crate) fn imported_xai_account_from_json(name: &str, raw: &str) -> Result<StoredAccount> {
    let value: serde_json::Value =
        serde_json::from_str(raw).wrap_err("auth file is not valid JSON")?;
    if let Some(account) = pi_xai_account(name, &value)? {
        return Ok(account);
    }
    if let Some(account) = grok_cli_account(name, &value) {
        return Ok(account);
    }
    Err(eyre!(
        "auth file does not contain xAI OAuth tokens (API keys are not supported)"
    ))
}

/// Read Pi `xai` OAuth credentials.
fn pi_xai_account(name: &str, value: &serde_json::Value) -> Result<Option<StoredAccount>> {
    let Some(entry) = value.get("xai") else {
        return Ok(None);
    };
    let auth: PiXaiAuth =
        serde_json::from_value(entry.clone()).wrap_err("invalid Pi xai auth entry")?;
    if auth.kind.as_deref().is_some_and(|kind| kind != "oauth") {
        return Err(eyre!("Pi xai entry is not OAuth"));
    }
    let access = required_text(auth.access, "access")?;
    Ok(Some(StoredAccount {
        name: name.to_string(),
        provider: Provider::Xai,
        account_id: None,
        access,
        refresh: auth.refresh.filter(|value| !value.trim().is_empty()),
        expires: auth.expires,
    }))
}

/// Pi `xai` OAuth credential subset.
#[derive(Deserialize)]
struct PiXaiAuth {
    /// Credential kind.
    #[serde(rename = "type")]
    kind: Option<String>,
    /// Access token.
    access: Option<String>,
    /// Refresh token.
    refresh: Option<String>,
    /// Expiry in Unix milliseconds.
    expires: Option<i64>,
}

/// Read Grok CLI `auth.json` OIDC entries.
fn grok_cli_account(name: &str, value: &serde_json::Value) -> Option<StoredAccount> {
    let root = value.as_object()?;
    let mut oidc = None;
    let mut legacy = None;
    for (scope, entry) in root {
        let Some(map) = entry.as_object() else {
            continue;
        };
        let Some(key) = map
            .get("key")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let candidate = GrokCliEntry {
            key: key.to_string(),
            refresh_token: string_field(map, "refresh_token"),
            expires_at: map.get("expires_at").cloned(),
            user_id: string_field(map, "user_id"),
            email: string_field(map, "email"),
        };
        if scope.starts_with(GROK_OIDC_SCOPE_PREFIX) {
            oidc = Some(candidate);
        } else if scope == GROK_LEGACY_SESSION_SCOPE || scope.contains("/sign-in") {
            legacy = Some(candidate);
        }
    }
    let entry = oidc.or(legacy)?;
    Some(StoredAccount {
        name: name.to_string(),
        provider: Provider::Xai,
        account_id: entry.user_id.or(entry.email),
        access: entry.key,
        refresh: entry.refresh_token,
        expires: parse_expires_at(entry.expires_at),
    })
}

/// Selected Grok CLI auth entry.
struct GrokCliEntry {
    /// Bearer token.
    key: String,
    /// Refresh token.
    refresh_token: Option<String>,
    /// Raw expiry field.
    expires_at: Option<serde_json::Value>,
    /// User id.
    user_id: Option<String>,
    /// Email.
    email: Option<String>,
}

/// Read a trimmed string field from a JSON object.
fn string_field(map: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<String> {
    map.get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

/// Convert a Grok CLI expiry field to Unix milliseconds.
fn parse_expires_at(value: Option<serde_json::Value>) -> Option<i64> {
    match value? {
        serde_json::Value::String(raw) => {
            parse_iso8601_epoch_seconds(&raw).map(|seconds| seconds.saturating_mul(1000))
        }
        serde_json::Value::Number(number) => {
            let n = number.as_i64()?;
            Some(if n > 10_000_000_000 {
                n
            } else {
                n.saturating_mul(1000)
            })
        }
        _ => None,
    }
}

/// Convert a stored xAI token set into an account.
#[must_use]
pub(crate) fn stored_from_tokens(name: &str, tokens: OAuthTokenSet) -> StoredAccount {
    StoredAccount {
        name: name.to_string(),
        provider: Provider::Xai,
        account_id: tokens.account_id,
        access: tokens.access,
        refresh: Some(tokens.refresh),
        expires: Some(tokens.expires),
    }
}

/// Require a non-empty string.
fn required_text(value: Option<String>, field: &str) -> Result<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| eyre!("xAI response missing {field}"))
}

/// Return the first non-empty trimmed string.
fn first_nonempty<'a>(values: impl IntoIterator<Item = Option<&'a str>>) -> Option<String> {
    values.into_iter().flatten().find_map(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

/// Require an `https://` verification URI.
fn https_uri(raw: Option<&str>) -> Result<String> {
    let raw = raw
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| eyre!("xAI device login missing verification_uri"))?;
    if !raw.starts_with("https://") {
        return Err(eyre!("untrusted xAI verification URI"));
    }
    Ok(raw.to_string())
}

/// Build a blocking HTTP client.
fn http_client(timeout: Duration) -> Result<Client> {
    Client::builder()
        .timeout(timeout)
        .build()
        .wrap_err("failed to build HTTP client")
}

/// Current Unix time in milliseconds.
fn now_unix_ms() -> Result<i64> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .wrap_err("system clock is before Unix epoch")?;
    i64::try_from(now.as_millis()).wrap_err("system clock overflowed")
}

#[cfg(test)]
mod tests {
    use super::{
        DeviceStartPayload, imported_xai_account_from_json, plan_from_settings_payload,
        quota_from_billing_payload,
    };
    use crate::cli::Provider;
    use crate::quota::QuotaError;

    #[test]
    fn device_start_prefers_complete_https_uri() {
        let session = DeviceStartPayload {
            device_code: Some("device".to_string()),
            user_code: Some("ABCD".to_string()),
            verification_uri: Some("https://auth.x.ai/device".to_string()),
            verification_uri_complete: Some("https://auth.x.ai/device?user_code=ABCD".to_string()),
            interval: Some(7),
            expires_in: Some(600),
        }
        .into_session()
        .expect("session");
        assert_eq!(
            session.verification_uri,
            "https://auth.x.ai/device?user_code=ABCD"
        );
        assert_eq!(session.interval.as_secs(), 7);
    }

    #[test]
    fn billing_payload_maps_weekly_percent() {
        let (plan, windows) = quota_from_billing_payload(
            r#"{
                "config": {
                    "creditUsagePercent": 33,
                    "currentPeriod": {
                        "start": "2026-08-20T00:00:00Z",
                        "end": "2026-08-27T00:00:00Z"
                    },
                    "subscriptionTier": "SuperGrok"
                }
            }"#,
        )
        .expect("payload");
        assert_eq!(plan.as_deref(), Some("SuperGrok"));
        assert_eq!(windows.len(), 1, "one credits window");
        assert_eq!(windows[0].label, "Weekly");
        assert!((windows[0].used_percent - 33.0).abs() < f64::EPSILON);
        assert_eq!(windows[0].window_minutes, Some(7 * 24 * 60));
    }

    #[test]
    fn billing_payload_falls_back_to_on_demand_ratio() {
        let (plan, windows) = quota_from_billing_payload(
            r#"{
                "config": {
                    "on_demand_cap": {"val": 100},
                    "on_demand_used": {"val": 25},
                    "billing_period_end": "2026-09-20T00:00:00Z"
                }
            }"#,
        )
        .expect("payload");
        assert!(plan.is_none(), "no plan on this fixture");
        assert_eq!(windows.len(), 1, "one credits window");
        assert_eq!(windows[0].label, "Credits");
        assert!((windows[0].used_percent - 25.0).abs() < f64::EPSILON);
    }

    #[test]
    fn billing_payload_zero_usage_when_only_reset_is_present() {
        let (_plan, windows) = quota_from_billing_payload(
            r#"{"config":{"currentPeriod":{"end":"2026-08-27T00:00:00Z"}}}"#,
        )
        .expect("payload");
        assert_eq!(windows.len(), 1, "one credits window");
        assert!((windows[0].used_percent - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn billing_payload_reports_missing_config() {
        assert_eq!(
            quota_from_billing_payload("{}").expect_err("missing config"),
            QuotaError::NoLimitData
        );
    }

    #[test]
    fn settings_payload_reads_plan_name() {
        assert_eq!(
            plan_from_settings_payload(r#"{"subscription_tier_display":"SuperGrok Heavy"}"#)
                .as_deref(),
            Some("SuperGrok Heavy")
        );
    }

    #[test]
    fn import_reads_pi_oauth_entry() {
        let account = imported_xai_account_from_json(
            "grok",
            r#"{"xai":{"type":"oauth","access":"xai-access","refresh":"xai-refresh","expires":9}}"#,
        )
        .expect("import");
        assert_eq!(account.provider, Provider::Xai);
        assert_eq!(account.access, "xai-access");
        assert_eq!(account.refresh.as_deref(), Some("xai-refresh"));
        assert_eq!(account.expires, Some(9));
    }

    #[test]
    fn import_reads_grok_cli_oidc_entry() {
        let account = imported_xai_account_from_json(
            "grok",
            r#"{
                "https://auth.x.ai::b1a00492-073a-47ea-816f-4c329264a828": {
                    "key": "grok-key",
                    "refresh_token": "grok-refresh",
                    "expires_at": "2026-08-27T00:00:00Z",
                    "user_id": "user-1",
                    "email": "a@b.com"
                }
            }"#,
        )
        .expect("import");
        assert_eq!(account.access, "grok-key");
        assert_eq!(account.account_id.as_deref(), Some("user-1"));
        assert_eq!(account.expires, Some(1_787_788_800_000));
    }

    #[test]
    fn import_rejects_api_keys() {
        let error =
            imported_xai_account_from_json("grok", r#"{"xai":{"type":"api_key","key":"xai-"}}"#)
                .expect_err("api key");
        assert!(error.to_string().contains("not OAuth"), "{error}");
    }
}
