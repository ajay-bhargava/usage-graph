//! Codex OAuth device-code login and refresh.

use crate::store::StoredAccount;
use base64::Engine;
use eyre::{Result, WrapErr, eyre};
use reqwest::blocking::Client;
use serde::Deserialize;
use std::collections::HashMap;
use std::time::Duration;

/// Codex OAuth application id used by Codex CLI / Pi.
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
/// Token endpoint.
const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
/// Device-code start endpoint.
const DEVICE_USER_CODE_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/usercode";
/// Device-code poll endpoint.
const DEVICE_TOKEN_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/token";
/// Browser verification URL printed to the user.
pub(crate) const DEVICE_VERIFICATION_URI: &str = "https://auth.openai.com/codex/device";
/// Redirect URI attached to the device-code token exchange.
const DEVICE_REDIRECT_URI: &str = "https://auth.openai.com/deviceauth/callback";
/// Maximum time to wait for the user to complete device login.
pub(crate) const DEVICE_CODE_TIMEOUT: Duration = Duration::from_secs(45 * 60);
/// Refresh slightly before the reported expiry.
const REFRESH_SKEW_MS: i64 = 5 * 60 * 1000;
/// Claim path used by Codex access tokens.
const JWT_AUTH_CLAIM: &str = "https://api.openai.com/auth";

/// Device-code session returned by the start endpoint.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DeviceAuthSession {
    /// Server-issued device auth id.
    pub(crate) device_auth_id: String,
    /// User-visible code.
    pub(crate) user_code: String,
    /// Polling interval.
    pub(crate) interval: Duration,
}

/// Result of one device-code poll.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DevicePoll {
    /// User has not finished authorizing.
    Pending,
    /// Caller should wait longer between polls.
    SlowDown,
    /// Authorization succeeded.
    Complete {
        /// Authorization code to exchange.
        authorization_code: String,
        /// PKCE verifier supplied by the device-auth service.
        code_verifier: String,
    },
}

/// Tokens returned by the OAuth token endpoint.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OAuthTokenSet {
    /// Bearer access token.
    pub(crate) access: String,
    /// Refresh token.
    pub(crate) refresh: String,
    /// Expiry in Unix milliseconds.
    pub(crate) expires: i64,
    /// Account id extracted from the access token when present.
    pub(crate) account_id: Option<String>,
}

/// Codex OAuth operations used by login and refresh.
pub(crate) trait CodexOAuth {
    /// Start a device-code login.
    ///
    /// Returns an error when the device-code endpoint cannot be reached or parsed.
    fn start_device_auth(&self) -> Result<DeviceAuthSession>;
    /// Poll an in-progress device-code login.
    ///
    /// Returns an error when the poll request fails for a non-pending reason.
    fn poll_device_auth(&self, session: &DeviceAuthSession) -> Result<DevicePoll>;
    /// Exchange a device authorization code for tokens.
    ///
    /// Returns an error when the token endpoint rejects the code.
    fn exchange_authorization_code(
        &self,
        authorization_code: &str,
        code_verifier: &str,
    ) -> Result<OAuthTokenSet>;
    /// Refresh an access token.
    ///
    /// Returns an error when refresh is rejected.
    fn refresh_access_token(&self, refresh_token: &str) -> Result<OAuthTokenSet>;
}

/// Blocking Codex OAuth client.
pub(crate) struct LiveCodexOAuth;

impl CodexOAuth for LiveCodexOAuth {
    fn start_device_auth(&self) -> Result<DeviceAuthSession> {
        let client = http_client()?;
        let response = client
            .post(DEVICE_USER_CODE_URL)
            .json(&serde_json::json!({ "client_id": CLIENT_ID }))
            .send()
            .wrap_err("failed to start Codex device login")?;
        if !response.status().is_success() {
            return Err(eyre!(
                "Codex device login start failed ({})",
                response.status()
            ));
        }
        let payload: DeviceStartPayload = response
            .json()
            .wrap_err("invalid Codex device login start payload")?;
        payload.into_session()
    }

    fn poll_device_auth(&self, session: &DeviceAuthSession) -> Result<DevicePoll> {
        let client = http_client()?;
        let response = client
            .post(DEVICE_TOKEN_URL)
            .json(&serde_json::json!({
                "device_auth_id": session.device_auth_id,
                "user_code": session.user_code,
            }))
            .send()
            .wrap_err("failed to poll Codex device login")?;
        if response.status().is_success() {
            let payload: DeviceCompletePayload = response
                .json()
                .wrap_err("invalid Codex device login poll payload")?;
            return payload.into_poll();
        }
        if matches!(response.status().as_u16(), 403 | 404) {
            return Ok(DevicePoll::Pending);
        }
        let body = response.text().unwrap_or_default();
        if body.contains("slow_down") {
            return Ok(DevicePoll::SlowDown);
        }
        if body.contains("deviceauth_authorization_pending") {
            return Ok(DevicePoll::Pending);
        }
        Err(eyre!("Codex device login poll failed: {body}"))
    }

    fn exchange_authorization_code(
        &self,
        authorization_code: &str,
        code_verifier: &str,
    ) -> Result<OAuthTokenSet> {
        exchange_form(&[
            ("grant_type", "authorization_code"),
            ("client_id", CLIENT_ID),
            ("code", authorization_code),
            ("code_verifier", code_verifier),
            ("redirect_uri", DEVICE_REDIRECT_URI),
        ])
    }

    fn refresh_access_token(&self, refresh_token: &str) -> Result<OAuthTokenSet> {
        exchange_form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", CLIENT_ID),
        ])
    }
}

/// Device start payload.
#[derive(Debug, Deserialize)]
struct DeviceStartPayload {
    /// Device auth id.
    device_auth_id: Option<String>,
    /// User code.
    user_code: Option<String>,
    /// Poll interval in seconds, string or number.
    interval: Option<serde_json::Value>,
}

impl DeviceStartPayload {
    /// Convert a successful start payload into a session.
    fn into_session(self) -> Result<DeviceAuthSession> {
        let device_auth_id = self
            .device_auth_id
            .filter(|value| !value.is_empty())
            .ok_or_else(|| eyre!("device login start missing device_auth_id"))?;
        let user_code = self
            .user_code
            .filter(|value| !value.is_empty())
            .ok_or_else(|| eyre!("device login start missing user_code"))?;
        let interval_secs = match self.interval {
            Some(serde_json::Value::Number(number)) => number.as_u64(),
            Some(serde_json::Value::String(value)) => value.trim().parse::<u64>().ok(),
            _ => None,
        }
        .unwrap_or(5);
        Ok(DeviceAuthSession {
            device_auth_id,
            user_code,
            interval: Duration::from_secs(interval_secs.max(1)),
        })
    }
}

/// Successful device poll payload.
#[derive(Debug, Deserialize)]
struct DeviceCompletePayload {
    /// Authorization code.
    authorization_code: Option<String>,
    /// PKCE verifier.
    code_verifier: Option<String>,
}

impl DeviceCompletePayload {
    /// Convert a successful poll payload.
    fn into_poll(self) -> Result<DevicePoll> {
        let authorization_code = self
            .authorization_code
            .filter(|value| !value.is_empty())
            .ok_or_else(|| eyre!("device login poll missing authorization_code"))?;
        let code_verifier = self
            .code_verifier
            .filter(|value| !value.is_empty())
            .ok_or_else(|| eyre!("device login poll missing code_verifier"))?;
        Ok(DevicePoll::Complete {
            authorization_code,
            code_verifier,
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

/// POST the token endpoint as form data.
fn exchange_form(fields: &[(&str, &str)]) -> Result<OAuthTokenSet> {
    let client = http_client()?;
    let mut form = HashMap::new();
    for (key, value) in fields {
        form.insert(*key, *value);
    }
    let response = client
        .post(TOKEN_URL)
        .form(&form)
        .send()
        .wrap_err("Codex token request failed")?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().unwrap_or_default();
        return Err(eyre!("Codex token request failed ({status}): {body}"));
    }
    let payload: TokenPayload = response.json().wrap_err("invalid Codex token payload")?;
    token_set_from_payload(payload, now_unix_ms()?)
}

/// Build a token set from a token-endpoint payload.
fn token_set_from_payload(payload: TokenPayload, now_ms: i64) -> Result<OAuthTokenSet> {
    let access = payload
        .access_token
        .filter(|value| !value.is_empty())
        .ok_or_else(|| eyre!("Codex token response missing access_token"))?;
    let refresh = payload
        .refresh_token
        .filter(|value| !value.is_empty())
        .ok_or_else(|| eyre!("Codex token response missing refresh_token"))?;
    let expires_in = i64::try_from(payload.expires_in.unwrap_or(3600)).unwrap_or(3600);
    let expires = now_ms.saturating_add(expires_in.saturating_mul(1000) - REFRESH_SKEW_MS);
    let account_id = account_id_from_access_token(&access);
    Ok(OAuthTokenSet {
        access,
        refresh,
        expires,
        account_id,
    })
}

/// Build a blocking HTTP client.
fn http_client() -> Result<Client> {
    Client::builder()
        .timeout(Duration::from_secs(15))
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

/// Extract an account id from a JWT access token when present.
#[must_use]
pub(crate) fn account_id_from_access_token(access_token: &str) -> Option<String> {
    let payload = decode_jwt_payload(access_token)?;
    payload
        .get(JWT_AUTH_CLAIM)
        .and_then(|claim| claim.get("chatgpt_account_id"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .filter(|value| !value.is_empty())
}

/// Login identity stored in an access token, without secrets.
///
/// Codex tokens contribute the profile email. xAI tokens contribute `team` or
/// `user` from `principal_type`.
#[must_use]
pub(crate) fn login_label_from_access_token(access_token: &str) -> Option<String> {
    let payload = decode_jwt_payload(access_token)?;
    if let Some(email) = payload
        .get("https://api.openai.com/profile")
        .and_then(|claim| claim.get("email"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Some(email.to_string());
    }
    payload
        .get("principal_type")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
}

/// Decode a JWT payload without verifying the signature.
fn decode_jwt_payload(token: &str) -> Option<serde_json::Value> {
    let payload = token.split('.').nth(1)?;
    let mut padded = payload.replace('-', "+").replace('_', "/");
    while padded.len() % 4 != 0 {
        padded.push('=');
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(padded)
        .ok()?;
    serde_json::from_slice(&decoded).ok()
}

/// Return whether the stored access token should be refreshed.
#[must_use]
pub(crate) fn access_token_needs_refresh(account: &StoredAccount, now_ms: i64) -> bool {
    account.refresh.is_some()
        && account
            .expires
            .is_none_or(|expires| now_ms + REFRESH_SKEW_MS >= expires)
}

#[cfg(test)]
mod tests {
    use super::{
        DeviceStartPayload, TokenPayload, access_token_needs_refresh, account_id_from_access_token,
        login_label_from_access_token, token_set_from_payload,
    };
    use crate::cli::Provider;
    use crate::store::StoredAccount;
    use base64::Engine;

    fn jwt_with_account(account_id: &str) -> String {
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"{\"alg\":\"none\"}");
        let payload =
            format!(r#"{{"https://api.openai.com/auth":{{"chatgpt_account_id":"{account_id}"}}}}"#);
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.as_bytes());
        format!("{header}.{payload}.sig")
    }

    #[test]
    fn device_start_payload_reads_string_interval() {
        let payload = DeviceStartPayload {
            device_auth_id: Some("device-1".to_string()),
            user_code: Some("ABCD-1234".to_string()),
            interval: Some(serde_json::Value::String("7".to_string())),
        };
        let session = payload.into_session().expect("session");
        assert_eq!(session.device_auth_id, "device-1");
        assert_eq!(session.interval.as_secs(), 7);
    }

    #[test]
    fn token_payload_extracts_account_id_from_jwt() {
        let access = jwt_with_account("acct-42");
        let tokens = token_set_from_payload(
            TokenPayload {
                access_token: Some(access.clone()),
                refresh_token: Some("refresh".to_string()),
                expires_in: Some(3600),
            },
            1_000_000,
        )
        .expect("tokens");
        assert_eq!(tokens.account_id.as_deref(), Some("acct-42"));
        assert_eq!(
            account_id_from_access_token(&access).as_deref(),
            Some("acct-42")
        );
        assert!(tokens.expires < 1_000_000 + 3_600_000, "skew applied");
    }

    #[test]
    fn login_label_reads_codex_email_and_xai_principal() {
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"{\"alg\":\"none\"}");
        let codex = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(br#"{"https://api.openai.com/profile":{"email":"a@b.co"}}"#);
        let xai = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(br#"{"principal_type":"Team"}"#);
        assert_eq!(
            login_label_from_access_token(&format!("{header}.{codex}.sig")).as_deref(),
            Some("a@b.co")
        );
        assert_eq!(
            login_label_from_access_token(&format!("{header}.{xai}.sig")).as_deref(),
            Some("team")
        );
    }

    #[test]
    fn refresh_is_due_when_expiry_is_missing_or_near() {
        let account = StoredAccount {
            name: "work".to_string(),
            provider: Provider::Codex,
            account_id: None,
            access: "access".to_string(),
            refresh: Some("refresh".to_string()),
            expires: None,
        };
        assert!(access_token_needs_refresh(&account, 0), "missing expiry");
        let mut fresh = account.clone();
        fresh.expires = Some(10_000_000);
        assert!(
            !access_token_needs_refresh(&fresh, 0),
            "far-future expiry is fresh"
        );
    }
}
