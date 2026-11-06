//! Claude (Pro / Max) subscription login and remaining-usage fetch.

use crate::cli::Provider;
use crate::oauth::OAuthTokenSet;
use crate::quota::{QuotaError, QuotaWindow, UsageHttpResponse};
use crate::store::StoredAccount;
use base64::Engine;
use eyre::{Result, WrapErr, eyre};
use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue, USER_AGENT};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::time::Duration;

/// Claude Code OAuth application id (same public client used by Claude Code and Pi).
const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
/// Browser authorization endpoint.
const AUTHORIZE_URL: &str = "https://claude.ai/oauth/authorize";
/// Token endpoint used for code exchange and refresh.
const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
/// Redirect that shows a copyable `code#state` value after sign-in.
const COPY_CODE_REDIRECT_URI: &str = "https://platform.claude.com/oauth/code/callback";
/// Scopes requested for subscription access.
const SCOPES: &str = "org:create_api_key user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";
/// Remaining-usage endpoint used by Claude Code `/usage`.
const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
/// Profile endpoint that carries the email and plan.
const PROFILE_URL: &str = "https://api.anthropic.com/api/oauth/profile";
/// Beta header required for OAuth-authenticated API calls.
const OAUTH_BETA: &str = "oauth-2025-04-20";
/// User agent for this CLI.
const USAGE_USER_AGENT: &str = "usage-cli";
/// Refresh slightly before the reported expiry.
const REFRESH_SKEW_MS: i64 = 5 * 60 * 1000;
/// Connect timeout for usage and profile.
const LIMIT_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// Whole-request timeout for usage.
const USAGE_TIMEOUT: Duration = Duration::from_secs(4);
/// Whole-request timeout for profile enrichment.
const PROFILE_TIMEOUT: Duration = Duration::from_secs(2);

/// Browser authorization session for the copy-code flow.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ClaudeAuthSession {
    /// URL the user opens in a browser.
    pub(crate) url: String,
    /// PKCE verifier, also used as `state`.
    pub(crate) verifier: String,
}

/// Claude OAuth operations used by login and refresh.
pub(crate) trait ClaudeOAuth {
    /// Start a browser copy-code login.
    ///
    /// # Errors
    ///
    /// Returns an error when secure randomness is unavailable.
    fn start_auth(&self) -> Result<ClaudeAuthSession>;
    /// Exchange the pasted `code#state` value for tokens.
    ///
    /// # Errors
    ///
    /// Returns an error when the state mismatches or the token endpoint rejects the code.
    fn exchange_code(&self, session: &ClaudeAuthSession, pasted: &str) -> Result<OAuthTokenSet>;
    /// Refresh an access token.
    ///
    /// # Errors
    ///
    /// Returns an error when refresh is rejected.
    fn refresh_access_token(&self, refresh_token: &str) -> Result<OAuthTokenSet>;
}

/// HTTP transport used to fetch Claude usage JSON.
pub(crate) trait ClaudeUsageTransport {
    /// Fetch the usage payload.
    ///
    /// # Errors
    ///
    /// Returns an error when the request fails before a status is available.
    fn get_usage(&self, account: &StoredAccount) -> Result<UsageHttpResponse>;
    /// Fetch the profile payload used for email and plan.
    ///
    /// # Errors
    ///
    /// Returns an error when the request fails before a status is available.
    fn get_profile(&self, account: &StoredAccount) -> Result<UsageHttpResponse>;
}

/// Blocking Claude OAuth client.
pub(crate) struct LiveClaudeOAuth;

impl ClaudeOAuth for LiveClaudeOAuth {
    fn start_auth(&self) -> Result<ClaudeAuthSession> {
        let mut bytes = [0_u8; 32];
        getrandom::fill(&mut bytes).map_err(|error| eyre!("secure randomness failed: {error}"))?;
        Ok(auth_session_from_verifier(&base64_url(&bytes)))
    }

    fn exchange_code(&self, session: &ClaudeAuthSession, pasted: &str) -> Result<OAuthTokenSet> {
        let (code, state) = parse_pasted_code(pasted)?;
        if state
            .as_deref()
            .is_some_and(|state| state != session.verifier)
        {
            return Err(eyre!("Claude OAuth state mismatch; restart login"));
        }
        let state = state.unwrap_or_else(|| session.verifier.clone());
        token_request(
            &serde_json::json!({
                "grant_type": "authorization_code",
                "client_id": CLIENT_ID,
                "code": code,
                "state": state,
                "redirect_uri": COPY_CODE_REDIRECT_URI,
                "code_verifier": session.verifier,
            }),
            None,
        )
    }

    fn refresh_access_token(&self, refresh_token: &str) -> Result<OAuthTokenSet> {
        token_request(
            &serde_json::json!({
                "grant_type": "refresh_token",
                "client_id": CLIENT_ID,
                "refresh_token": refresh_token,
            }),
            Some(refresh_token),
        )
    }
}

/// Live blocking usage transport.
pub(crate) struct LiveClaudeUsageTransport;

impl ClaudeUsageTransport for LiveClaudeUsageTransport {
    fn get_usage(&self, account: &StoredAccount) -> Result<UsageHttpResponse> {
        api_get(USAGE_URL, USAGE_TIMEOUT, account)
    }

    fn get_profile(&self, account: &StoredAccount) -> Result<UsageHttpResponse> {
        api_get(PROFILE_URL, PROFILE_TIMEOUT, account)
    }
}

/// Build the authorize URL for a PKCE verifier.
fn auth_session_from_verifier(verifier: &str) -> ClaudeAuthSession {
    let challenge = base64_url(&Sha256::digest(verifier.as_bytes()));
    let query = [
        ("code", "true"),
        ("client_id", CLIENT_ID),
        ("response_type", "code"),
        ("redirect_uri", COPY_CODE_REDIRECT_URI),
        ("scope", SCOPES),
        ("code_challenge", challenge.as_str()),
        ("code_challenge_method", "S256"),
        ("state", verifier),
    ]
    .iter()
    .map(|(key, value)| format!("{key}={}", percent_encode(value)))
    .collect::<Vec<_>>()
    .join("&");
    ClaudeAuthSession {
        url: format!("{AUTHORIZE_URL}?{query}"),
        verifier: verifier.to_string(),
    }
}

/// Split a pasted `code#state`, redirect URL, or bare code.
fn parse_pasted_code(raw: &str) -> Result<(String, Option<String>)> {
    let value = raw.trim();
    if value.is_empty() {
        return Err(eyre!("missing Claude authorization code"));
    }
    if let Some((_, query)) = value.split_once('?') {
        let mut code = None;
        let mut state = None;
        for pair in query.split(['&', '#']) {
            match pair.split_once('=') {
                Some(("code", v)) => code = Some(v.to_string()),
                Some(("state", v)) => state = Some(v.to_string()),
                _ => {}
            }
        }
        let code = code.ok_or_else(|| eyre!("redirect URL has no code parameter"))?;
        return Ok((code, state));
    }
    if let Some((code, state)) = value.split_once('#') {
        return Ok((code.to_string(), Some(state.to_string())));
    }
    Ok((value.to_string(), None))
}

/// Percent-encode a query value (RFC 3986 unreserved set).
fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

/// Unpadded URL-safe base64.
fn base64_url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
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

/// POST the token endpoint as JSON.
fn token_request(
    body: &serde_json::Value,
    previous_refresh: Option<&str>,
) -> Result<OAuthTokenSet> {
    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .wrap_err("failed to build HTTP client")?;
    let response = client
        .post(TOKEN_URL)
        .header(ACCEPT, "application/json")
        .json(body)
        .send()
        .wrap_err("Claude token request failed")?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().unwrap_or_default();
        return Err(eyre!("Claude token request failed ({status}): {body}"));
    }
    let payload: TokenPayload = response.json().wrap_err("invalid Claude token payload")?;
    token_set_from_payload(payload, now_unix_ms()?, previous_refresh)
}

/// Build a token set from a token-endpoint payload.
fn token_set_from_payload(
    payload: TokenPayload,
    now_ms: i64,
    previous_refresh: Option<&str>,
) -> Result<OAuthTokenSet> {
    let access = payload
        .access_token
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| eyre!("Claude token response missing access_token"))?;
    let refresh = payload
        .refresh_token
        .filter(|value| !value.trim().is_empty())
        .or_else(|| previous_refresh.map(ToOwned::to_owned))
        .ok_or_else(|| eyre!("Claude token response missing refresh_token"))?;
    let expires_in = i64::try_from(payload.expires_in.unwrap_or(3600)).unwrap_or(3600);
    let expires = now_ms.saturating_add(expires_in.saturating_mul(1000) - REFRESH_SKEW_MS);
    Ok(OAuthTokenSet {
        access,
        refresh,
        expires,
        account_id: None,
    })
}

/// GET one OAuth API JSON endpoint.
fn api_get(url: &str, timeout: Duration, account: &StoredAccount) -> Result<UsageHttpResponse> {
    let client = Client::builder()
        .connect_timeout(LIMIT_CONNECT_TIMEOUT)
        .timeout(timeout)
        .build()
        .wrap_err("failed to build Claude HTTP client")?;
    let headers =
        api_headers(&account.access).ok_or_else(|| eyre!("invalid Claude access token header"))?;
    let response = client
        .get(url)
        .headers(headers)
        .send()
        .wrap_err("Claude usage request failed")?;
    let status = response.status().as_u16();
    let body = response
        .text()
        .wrap_err("failed to read Claude usage response")?;
    Ok(UsageHttpResponse { status, body })
}

/// Build headers for Claude OAuth API calls.
fn api_headers(access_token: &str) -> Option<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {access_token}")).ok()?,
    );
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    headers.insert(USER_AGENT, HeaderValue::from_static(USAGE_USER_AGENT));
    headers.insert("anthropic-beta", HeaderValue::from_static(OAUTH_BETA));
    Some(headers)
}

/// One utilization bucket from the usage payload.
#[derive(Debug, Deserialize)]
struct UsageBucket {
    /// Used percentage (0-100).
    #[serde(default)]
    utilization: Option<f64>,
    /// RFC 3339 reset time.
    #[serde(default)]
    resets_at: Option<String>,
}

/// One entry of the newer `limits` array.
#[derive(Debug, Deserialize)]
struct UsageLimit {
    /// Limit kind such as `session`, `weekly_all`, or `weekly_scoped`.
    #[serde(default)]
    kind: Option<String>,
    /// Used percentage (0-100).
    #[serde(default)]
    percent: Option<f64>,
    /// RFC 3339 reset time.
    #[serde(default)]
    resets_at: Option<String>,
    /// Optional scope (model or surface).
    #[serde(default)]
    scope: Option<serde_json::Value>,
}

/// `/api/oauth/usage` subset.
#[derive(Debug, Deserialize)]
struct UsagePayload {
    /// Rolling 5-hour session window.
    #[serde(default)]
    five_hour: Option<UsageBucket>,
    /// Rolling 7-day window across all models.
    #[serde(default)]
    seven_day: Option<UsageBucket>,
    /// Opus-only 7-day window.
    #[serde(default)]
    seven_day_opus: Option<UsageBucket>,
    /// Sonnet-only 7-day window.
    #[serde(default)]
    seven_day_sonnet: Option<UsageBucket>,
    /// Typed limit list (newer payloads).
    #[serde(default)]
    limits: Option<Vec<UsageLimit>>,
}

/// Parse a usage JSON body into quota windows.
pub(crate) fn quota_from_usage_payload(raw: &str) -> Result<Vec<QuotaWindow>, QuotaError> {
    let payload: UsagePayload =
        serde_json::from_str(raw).map_err(|_| QuotaError::InvalidResponse)?;
    let mut windows = Vec::new();
    let named = [
        (payload.five_hour, "claude", "5h", Some(5 * 60)),
        (payload.seven_day, "claude", "Weekly", Some(7 * 24 * 60)),
        (
            payload.seven_day_opus,
            "opus",
            "Opus Weekly",
            Some(7 * 24 * 60),
        ),
        (
            payload.seven_day_sonnet,
            "sonnet",
            "Sonnet Weekly",
            Some(7 * 24 * 60),
        ),
    ];
    for (bucket, id, label, minutes) in named {
        if let Some(window) = bucket.and_then(|bucket| {
            window_from(
                id,
                label,
                bucket.utilization,
                bucket.resets_at.as_deref(),
                minutes,
            )
        }) {
            windows.push(window);
        }
    }
    for limit in payload.limits.unwrap_or_default() {
        let kind = limit.kind.as_deref().unwrap_or_default();
        let (id, label) = match kind {
            "session" => ("claude".to_string(), "5h".to_string()),
            "weekly_all" => ("claude".to_string(), "Weekly".to_string()),
            "weekly_scoped" => {
                let Some(name) = scope_name(limit.scope.as_ref()) else {
                    continue;
                };
                (name.to_ascii_lowercase(), format!("{name} Weekly"))
            }
            _ => continue,
        };
        if windows.iter().any(|window| window.label == label) {
            continue;
        }
        let minutes = if kind == "session" {
            Some(5 * 60)
        } else {
            Some(7 * 24 * 60)
        };
        if let Some(window) = window_from(
            &id,
            &label,
            limit.percent,
            limit.resets_at.as_deref(),
            minutes,
        ) {
            windows.push(window);
        }
    }
    if windows.is_empty() {
        Err(QuotaError::NoLimitData)
    } else {
        Ok(windows)
    }
}

/// Human name for a scoped limit (`scope.model.display_name` or `scope.surface`).
fn scope_name(scope: Option<&serde_json::Value>) -> Option<String> {
    let scope = scope?;
    let model = scope
        .get("model")
        .and_then(|model| model.get("display_name").or_else(|| model.get("id")))
        .and_then(serde_json::Value::as_str);
    let surface = scope.get("surface").and_then(serde_json::Value::as_str);
    model
        .or(surface)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

/// Build one quota window when utilization is present.
fn window_from(
    bucket: &str,
    label: &str,
    used: Option<f64>,
    resets_at: Option<&str>,
    window_minutes: Option<i64>,
) -> Option<QuotaWindow> {
    let used = used.filter(|value| value.is_finite())?;
    Some(QuotaWindow {
        bucket: bucket.to_string(),
        label: label.to_string(),
        used_percent: used.clamp(0.0, 100.0),
        window_minutes,
        resets_at_epoch_seconds: resets_at.and_then(|raw| {
            chrono::DateTime::parse_from_rfc3339(raw.trim())
                .ok()
                .map(|value| value.timestamp())
        }),
    })
}

/// Email and plan read from `/api/oauth/profile`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ClaudeProfile {
    /// Account email.
    pub(crate) email: Option<String>,
    /// Plan such as `max 20x` or `pro`.
    pub(crate) plan: Option<String>,
}

/// Parse email and plan from a profile payload.
#[must_use]
pub(crate) fn profile_from_payload(raw: &str) -> ClaudeProfile {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return ClaudeProfile::default();
    };
    let text = |pointer: &str| {
        value
            .pointer(pointer)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    };
    let tier = text("/organization/rate_limit_tier");
    let org_type = text("/organization/organization_type");
    let plan = match (org_type.as_deref(), tier.as_deref()) {
        (Some("claude_max"), Some(tier)) if tier.contains("20x") => Some("max 20x".to_string()),
        (Some("claude_max"), Some(tier)) if tier.contains("5x") => Some("max 5x".to_string()),
        (Some("claude_max"), _) => Some("max".to_string()),
        (Some("claude_pro"), _) => Some("pro".to_string()),
        (Some(other), _) => Some(other.trim_start_matches("claude_").replace('_', " ")),
        (None, _) => None,
    };
    ClaudeProfile {
        email: text("/account/email"),
        plan,
    }
}

/// Import Claude tokens from Claude Code `.credentials.json` or Pi `anthropic` OAuth.
///
/// Only the access token is stored: Claude refresh tokens rotate, and refreshing a
/// copied token would sign the source app out. Run `usage reauth` once it expires.
///
/// # Errors
///
/// Returns an error when the file contains no Claude OAuth access token.
pub(crate) fn imported_claude_account_from_json(name: &str, raw: &str) -> Result<StoredAccount> {
    let value: serde_json::Value =
        serde_json::from_str(raw).wrap_err("auth file is not valid JSON")?;
    let (access, expires) = if let Some(entry) = value.get("claudeAiOauth") {
        (
            entry.get("accessToken").and_then(serde_json::Value::as_str),
            entry.get("expiresAt").and_then(serde_json::Value::as_i64),
        )
    } else if let Some(entry) = value.get("anthropic") {
        if entry
            .get("type")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|kind| kind != "oauth")
        {
            return Err(eyre!(
                "Pi anthropic entry is not OAuth (API keys are not supported)"
            ));
        }
        (
            entry.get("access").and_then(serde_json::Value::as_str),
            entry.get("expires").and_then(serde_json::Value::as_i64),
        )
    } else {
        (None, None)
    };
    let access = access
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            eyre!(
                "auth file does not contain Claude subscription tokens (API keys are not supported)"
            )
        })?;
    Ok(StoredAccount {
        name: name.to_string(),
        provider: Provider::Claude,
        account_id: None,
        access: access.to_string(),
        refresh: None,
        expires,
    })
}

/// Convert OAuth tokens into a stored Claude account.
#[must_use]
pub(crate) fn stored_from_tokens(name: &str, tokens: OAuthTokenSet) -> StoredAccount {
    StoredAccount {
        name: name.to_string(),
        provider: Provider::Claude,
        account_id: None,
        access: tokens.access,
        refresh: Some(tokens.refresh),
        expires: Some(tokens.expires),
    }
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
        TokenPayload, auth_session_from_verifier, imported_claude_account_from_json,
        parse_pasted_code, profile_from_payload, quota_from_usage_payload, token_set_from_payload,
    };
    use crate::cli::Provider;
    use crate::quota::QuotaError;

    const LIVE_SHAPE: &str = r#"{
        "five_hour":{"utilization":12.5,"resets_at":"2026-11-02T05:59:59.876878+00:00"},
        "seven_day":{"utilization":40.0,"resets_at":"2026-11-08T16:59:59.876930+00:00"},
        "seven_day_opus":null,
        "seven_day_sonnet":{"utilization":3,"resets_at":null},
        "extra_usage":{"is_enabled":false},
        "limits":[
            {"kind":"session","percent":12,"resets_at":"2026-11-02T05:59:59+00:00"},
            {"kind":"weekly_all","percent":40,"resets_at":"2026-11-08T16:59:59+00:00"},
            {"kind":"weekly_scoped","percent":7,"resets_at":"2026-11-08T17:00:00+00:00",
             "scope":{"model":{"id":null,"display_name":"Fable"},"surface":null}}
        ]
    }"#;

    #[test]
    fn usage_payload_maps_session_weekly_and_scoped_limits() {
        let windows = quota_from_usage_payload(LIVE_SHAPE).expect("payload");
        let labels = windows.iter().map(|w| w.label.as_str()).collect::<Vec<_>>();
        assert_eq!(labels, ["5h", "Weekly", "Sonnet Weekly", "Fable Weekly"]);
        assert!((windows[0].used_percent - 12.5).abs() < f64::EPSILON);
        assert_eq!(windows[0].resets_at_epoch_seconds, Some(1_793_599_199));
        assert_eq!(windows[3].bucket, "fable");
        assert_eq!(windows[2].resets_at_epoch_seconds, None);
    }

    #[test]
    fn usage_payload_falls_back_to_limits_only() {
        let windows = quota_from_usage_payload(
            r#"{"limits":[{"kind":"session","percent":50},{"kind":"weekly_all","percent":10}]}"#,
        )
        .expect("payload");
        assert_eq!(windows.len(), 2, "limits-only payloads still render");
        assert_eq!(windows[1].label, "Weekly");
    }

    #[test]
    fn usage_payload_errors_degrade() {
        assert_eq!(
            quota_from_usage_payload("nope").expect_err("bad json"),
            QuotaError::InvalidResponse
        );
        assert_eq!(
            quota_from_usage_payload(r#"{"five_hour":null}"#).expect_err("empty"),
            QuotaError::NoLimitData
        );
    }

    #[test]
    fn profile_payload_reads_email_and_max_tier() {
        let profile = profile_from_payload(
            r#"{"account":{"email":"a@b.co"},"organization":{"organization_type":"claude_max","rate_limit_tier":"default_claude_max_20x"}}"#,
        );
        assert_eq!(profile.email.as_deref(), Some("a@b.co"));
        assert_eq!(profile.plan.as_deref(), Some("max 20x"));
        assert_eq!(profile_from_payload("x"), super::ClaudeProfile::default());
    }

    #[test]
    fn authorize_url_carries_pkce_and_state() {
        let session = auth_session_from_verifier("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
        assert!(
            session
                .url
                .starts_with("https://claude.ai/oauth/authorize?code=true&")
        );
        // RFC 7636 appendix B vector.
        assert!(
            session
                .url
                .contains("code_challenge=E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"),
            "{}",
            session.url
        );
        assert!(
            session
                .url
                .contains("scope=org%3Acreate_api_key%20user%3Aprofile")
        );
        assert!(
            session
                .url
                .contains("state=dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk")
        );
    }

    #[test]
    fn pasted_code_accepts_hash_url_and_bare_forms() {
        assert_eq!(
            parse_pasted_code(" abc#xyz \n").expect("hash"),
            ("abc".to_string(), Some("xyz".to_string()))
        );
        assert_eq!(
            parse_pasted_code("https://x/cb?code=abc&state=xyz").expect("url"),
            ("abc".to_string(), Some("xyz".to_string()))
        );
        assert_eq!(
            parse_pasted_code("abc").expect("bare"),
            ("abc".to_string(), None)
        );
        assert!(parse_pasted_code("  ").is_err(), "empty is rejected");
    }

    #[test]
    fn refresh_keeps_previous_refresh_when_omitted() {
        let tokens = token_set_from_payload(
            TokenPayload {
                access_token: Some("a".to_string()),
                refresh_token: None,
                expires_in: Some(3600),
            },
            0,
            Some("old"),
        )
        .expect("tokens");
        assert_eq!(tokens.refresh, "old");
        assert_eq!(tokens.expires, 3_600_000 - 300_000);
    }

    #[test]
    fn import_reads_claude_code_and_pi_entries_without_refresh() {
        let account = imported_claude_account_from_json(
            "max",
            r#"{"claudeAiOauth":{"accessToken":"cc","refreshToken":"r","expiresAt":9}}"#,
        )
        .expect("claude code");
        assert_eq!(account.provider, Provider::Claude);
        assert_eq!(account.access, "cc");
        assert_eq!(
            account.refresh, None,
            "rotating refresh token is never copied"
        );
        assert_eq!(account.expires, Some(9));

        let pi = imported_claude_account_from_json(
            "max",
            r#"{"anthropic":{"type":"oauth","access":"pa","refresh":"r","expires":1}}"#,
        )
        .expect("pi");
        assert_eq!(pi.access, "pa");
        assert!(
            imported_claude_account_from_json("x", r#"{"anthropic":{"type":"api_key","key":"k"}}"#)
                .is_err(),
            "api keys are rejected"
        );
    }
}
