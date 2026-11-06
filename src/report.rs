//! Fetch and print remaining usage for stored accounts.

use crate::claude::{
    ClaudeOAuth, ClaudeUsageTransport, profile_from_payload,
    quota_from_usage_payload as quota_from_claude_payload,
};
use crate::cli::Provider;
use crate::oauth::CodexOAuth;
use crate::quota::{
    AccountQuota, CodexUsageTransport, QuotaError, quota_error_from_status,
    quota_from_usage_payload,
};
use crate::render::{detect_border_style, render_report, report_json};
use crate::store::{StoredAccount, load_store, save_store};
use crate::xai::{
    XaiOAuth, XaiUsageTransport, plan_from_settings_payload, quota_from_billing_payload,
};
use eyre::{Result, WrapErr, eyre};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Print remaining usage for stored accounts.
///
/// Returns an error when the store cannot be read, no accounts are present, or JSON
/// serialization fails. Per-account fetch failures are rendered as `unavailable`.
#[allow(
    clippy::too_many_arguments,
    reason = "oauth and transport are paired per provider"
)]
pub(crate) fn print_usage_report(
    store_path: &Path,
    account_filter: Option<&str>,
    json: bool,
    offline: bool,
    oauth: &impl CodexOAuth,
    xai_oauth: &impl XaiOAuth,
    claude_oauth: &impl ClaudeOAuth,
    transport: &impl CodexUsageTransport,
    xai_transport: &impl XaiUsageTransport,
    claude_transport: &impl ClaudeUsageTransport,
) -> Result<()> {
    let mut store = load_store(store_path)?;
    if store.accounts.is_empty() {
        return Err(eyre!(
            "No accounts. Run `usage login --provider codex --name <name>` or `--provider claude|xai` to add one."
        ));
    }
    let now_ms = unix_ms()?;
    let now_secs = now_ms / 1000;
    let mut quotas = Vec::new();
    let mut store_changed = false;
    for account in &mut store.accounts {
        if account_filter.is_some_and(|name| name != account.name) {
            continue;
        }
        let before = account.clone();
        let quota = quota_for_account(
            account,
            offline,
            now_ms,
            &Clients {
                oauth,
                xai_oauth,
                claude_oauth,
                transport,
                xai_transport,
                claude_transport,
            },
        );
        if *account != before {
            store_changed = true;
        }
        quotas.push(quota);
    }
    if let Some(name) = account_filter
        && quotas.is_empty()
    {
        return Err(eyre!("no account named {name}"));
    }
    if store_changed {
        save_store(store_path, &store).wrap_err("failed to persist refreshed tokens")?;
    }
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report_json(&quotas, now_secs))?
        );
    } else {
        print!(
            "{}",
            render_report(&quotas, detect_border_style(), now_secs)
        );
    }
    Ok(())
}

/// Provider clients used while building a report.
struct Clients<'a, CO, XO, AO, CT, XT, AT> {
    /// Codex OAuth.
    oauth: &'a CO,
    /// xAI OAuth.
    xai_oauth: &'a XO,
    /// Claude OAuth.
    claude_oauth: &'a AO,
    /// Codex usage transport.
    transport: &'a CT,
    /// xAI usage transport.
    xai_transport: &'a XT,
    /// Claude usage transport.
    claude_transport: &'a AT,
}

/// Fetch remaining usage for one stored account.
fn quota_for_account<CO, XO, AO, CT, XT, AT>(
    account: &mut StoredAccount,
    offline: bool,
    now_ms: i64,
    clients: &Clients<'_, CO, XO, AO, CT, XT, AT>,
) -> AccountQuota
where
    CO: CodexOAuth,
    XO: XaiOAuth,
    AO: ClaudeOAuth,
    CT: CodexUsageTransport,
    XT: XaiUsageTransport,
    AT: ClaudeUsageTransport,
{
    let Clients {
        oauth,
        xai_oauth,
        claude_oauth,
        transport,
        xai_transport,
        claude_transport,
    } = *clients;
    if offline {
        return unavailable(account, QuotaError::Offline);
    }
    match account.provider {
        Provider::Codex => {
            let _ = try_refresh(account, now_ms, |refresh| {
                oauth.refresh_access_token(refresh)
            });
            fetch_codex_quota(account, transport)
        }
        Provider::Xai => {
            let _ = try_refresh(account, now_ms, |refresh| {
                xai_oauth.refresh_access_token(refresh)
            });
            fetch_xai_quota(account, xai_transport)
        }
        Provider::Claude => {
            let _ = try_refresh(account, now_ms, |refresh| {
                claude_oauth.refresh_access_token(refresh)
            });
            fetch_claude_quota(account, claude_transport)
        }
    }
}

/// Fetch Claude 5h / weekly utilization windows.
fn fetch_claude_quota(
    account: &StoredAccount,
    transport: &impl ClaudeUsageTransport,
) -> AccountQuota {
    let profile = transport
        .get_profile(account)
        .ok()
        .filter(|response| response.status == 200)
        .map(|response| profile_from_payload(&response.body))
        .unwrap_or_default();
    let mut quota = match transport.get_usage(account) {
        Ok(response) => {
            if let Some(error) = quota_error_from_status(response.status) {
                unavailable(account, error)
            } else {
                match quota_from_claude_payload(&response.body) {
                    Ok(windows) => available(account, profile.plan.clone(), windows),
                    Err(error) => unavailable(account, error),
                }
            }
        }
        Err(_) => unavailable(account, QuotaError::RequestFailed),
    };
    if quota.login.is_none() {
        quota.login = profile.email;
    }
    if quota.plan.is_none() {
        quota.plan = profile.plan;
    }
    quota
}

/// Fetch Codex remaining-usage windows.
fn fetch_codex_quota(
    account: &StoredAccount,
    transport: &impl CodexUsageTransport,
) -> AccountQuota {
    match transport.get_usage(account) {
        Ok(response) => {
            if let Some(error) = quota_error_from_status(response.status) {
                return unavailable(account, error);
            }
            match quota_from_usage_payload(&response.body) {
                Ok((plan, windows)) => available(account, plan, windows),
                Err(error) => unavailable(account, error),
            }
        }
        Err(_) => unavailable(account, QuotaError::RequestFailed),
    }
}

/// Fetch xAI remaining-credits windows.
fn fetch_xai_quota(account: &StoredAccount, transport: &impl XaiUsageTransport) -> AccountQuota {
    match transport.get_billing(account) {
        Ok(response) => {
            if let Some(error) = quota_error_from_status(response.status) {
                return unavailable(account, error);
            }
            match quota_from_billing_payload(&response.body) {
                Ok((mut plan, windows)) => {
                    if plan.is_none()
                        && let Ok(settings) = transport.get_settings(account)
                        && settings.status == 200
                    {
                        plan = plan_from_settings_payload(&settings.body);
                    }
                    available(account, plan, windows)
                }
                Err(error) => unavailable(account, error),
            }
        }
        Err(_) => unavailable(account, QuotaError::RequestFailed),
    }
}

/// Refresh tokens in place without requiring the full store.
fn try_refresh(
    account: &mut StoredAccount,
    now_ms: i64,
    refresh: impl FnOnce(&str) -> Result<crate::oauth::OAuthTokenSet>,
) -> Result<()> {
    if !crate::oauth::access_token_needs_refresh(account, now_ms) {
        return Ok(());
    }
    let Some(refresh_token) = account.refresh.clone() else {
        return Ok(());
    };
    let tokens = refresh(&refresh_token)?;
    account.access = tokens.access;
    account.refresh = Some(tokens.refresh);
    if tokens.account_id.is_some() {
        account.account_id = tokens.account_id;
    }
    account.expires = Some(tokens.expires);
    Ok(())
}

/// Build a successful quota record.
fn available(
    account: &StoredAccount,
    plan: Option<String>,
    windows: Vec<crate::quota::QuotaWindow>,
) -> AccountQuota {
    AccountQuota {
        name: account.name.clone(),
        provider: account.provider,
        login: crate::oauth::login_label_from_access_token(&account.access),
        plan,
        windows,
        error: None,
    }
}

/// Build an unavailable quota record.
fn unavailable(account: &StoredAccount, error: QuotaError) -> AccountQuota {
    AccountQuota {
        name: account.name.clone(),
        provider: account.provider,
        login: crate::oauth::login_label_from_access_token(&account.access),
        plan: None,
        windows: Vec::new(),
        error: Some(error),
    }
}

/// Current Unix time in milliseconds.
fn unix_ms() -> Result<i64> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .wrap_err("system clock is before Unix epoch")?;
    i64::try_from(now.as_millis()).wrap_err("system clock overflowed")
}

#[cfg(test)]
mod tests {
    use super::print_usage_report;
    use crate::claude::{ClaudeAuthSession, ClaudeOAuth, ClaudeUsageTransport};
    use crate::cli::Provider;
    use crate::oauth::{CodexOAuth, DeviceAuthSession, DevicePoll, OAuthTokenSet};
    use crate::quota::{CodexUsageTransport, UsageHttpResponse};
    use crate::store::{AccountStore, StoredAccount, save_store};
    use crate::xai::{XaiDevicePoll, XaiDeviceSession, XaiOAuth, XaiUsageTransport};
    use eyre::{Result, eyre};
    use tempfile::TempDir;

    struct NoOAuth;

    impl CodexOAuth for NoOAuth {
        fn start_device_auth(&self) -> Result<DeviceAuthSession> {
            Err(eyre!("unused"))
        }
        fn poll_device_auth(&self, _session: &DeviceAuthSession) -> Result<DevicePoll> {
            Err(eyre!("unused"))
        }
        fn exchange_authorization_code(
            &self,
            _authorization_code: &str,
            _code_verifier: &str,
        ) -> Result<OAuthTokenSet> {
            Err(eyre!("unused"))
        }
        fn refresh_access_token(&self, _refresh_token: &str) -> Result<OAuthTokenSet> {
            Err(eyre!("unused"))
        }
    }

    struct FixtureTransport {
        /// Body returned for a 200 response.
        body: String,
    }

    impl CodexUsageTransport for FixtureTransport {
        fn get_usage(&self, _account: &StoredAccount) -> Result<UsageHttpResponse> {
            Ok(UsageHttpResponse {
                status: 200,
                body: self.body.clone(),
            })
        }
    }

    struct NoXaiOAuth;

    impl XaiOAuth for NoXaiOAuth {
        fn start_device_auth(&self) -> Result<XaiDeviceSession> {
            Err(eyre!("unused"))
        }
        fn poll_device_auth(&self, _session: &XaiDeviceSession) -> Result<XaiDevicePoll> {
            Err(eyre!("unused"))
        }
        fn refresh_access_token(&self, _refresh_token: &str) -> Result<OAuthTokenSet> {
            Err(eyre!("unused"))
        }
    }

    struct FixtureXaiTransport {
        /// Billing body.
        billing: String,
        /// Settings body.
        settings: String,
    }

    impl XaiUsageTransport for FixtureXaiTransport {
        fn get_billing(&self, _account: &StoredAccount) -> Result<UsageHttpResponse> {
            Ok(UsageHttpResponse {
                status: 200,
                body: self.billing.clone(),
            })
        }
        fn get_settings(&self, _account: &StoredAccount) -> Result<UsageHttpResponse> {
            Ok(UsageHttpResponse {
                status: 200,
                body: self.settings.clone(),
            })
        }
    }

    struct NoClaudeOAuth;

    impl ClaudeOAuth for NoClaudeOAuth {
        fn start_auth(&self) -> Result<ClaudeAuthSession> {
            Err(eyre!("unused"))
        }
        fn exchange_code(&self, _s: &ClaudeAuthSession, _p: &str) -> Result<OAuthTokenSet> {
            Err(eyre!("unused"))
        }
        fn refresh_access_token(&self, _refresh_token: &str) -> Result<OAuthTokenSet> {
            Err(eyre!("unused"))
        }
    }

    struct FixtureClaudeTransport {
        /// Usage body.
        usage: String,
    }

    impl ClaudeUsageTransport for FixtureClaudeTransport {
        fn get_usage(&self, _account: &StoredAccount) -> Result<UsageHttpResponse> {
            Ok(UsageHttpResponse {
                status: 200,
                body: self.usage.clone(),
            })
        }
        fn get_profile(&self, _account: &StoredAccount) -> Result<UsageHttpResponse> {
            Ok(UsageHttpResponse {
                status: 200,
                body: r#"{"account":{"email":"me@x.co"},"organization":{"organization_type":"claude_pro"}}"#
                    .to_string(),
            })
        }
    }

    fn empty_claude() -> FixtureClaudeTransport {
        FixtureClaudeTransport {
            usage: String::new(),
        }
    }

    #[test]
    fn json_report_includes_claude_windows() {
        let temp = TempDir::new().expect("tempdir");
        let path = temp.path().join("accounts.json");
        save_store(
            &path,
            &AccountStore {
                accounts: vec![StoredAccount {
                    name: "max".to_string(),
                    provider: Provider::Claude,
                    account_id: None,
                    access: "access".to_string(),
                    refresh: None,
                    expires: None,
                }],
            },
        )
        .expect("save");
        print_usage_report(
            &path,
            None,
            true,
            false,
            &NoOAuth,
            &NoXaiOAuth,
            &NoClaudeOAuth,
            &FixtureTransport {
                body: String::new(),
            },
            &empty_xai(),
            &FixtureClaudeTransport {
                usage:
                    r#"{"five_hour":{"utilization":20,"resets_at":"2026-11-02T05:59:59+00:00"}}"#
                        .to_string(),
            },
        )
        .expect("claude json report");
    }

    fn empty_xai() -> FixtureXaiTransport {
        FixtureXaiTransport {
            billing: String::new(),
            settings: String::new(),
        }
    }

    fn write_account(dir: &TempDir) -> std::path::PathBuf {
        let path = dir.path().join("accounts.json");
        save_store(
            &path,
            &AccountStore {
                accounts: vec![StoredAccount {
                    name: "work".to_string(),
                    provider: Provider::Codex,
                    account_id: Some("acct".to_string()),
                    access: "access".to_string(),
                    refresh: None,
                    expires: None,
                }],
            },
        )
        .expect("save");
        path
    }

    #[test]
    fn empty_store_errors_without_auto_import() {
        let temp = TempDir::new().expect("tempdir");
        let error = print_usage_report(
            &temp.path().join("missing.json"),
            None,
            false,
            false,
            &NoOAuth,
            &NoXaiOAuth,
            &NoClaudeOAuth,
            &FixtureTransport {
                body: String::new(),
            },
            &empty_xai(),
            &empty_claude(),
        )
        .expect_err("empty");
        assert!(error.to_string().contains("No accounts"), "{error}");
    }

    #[test]
    fn offline_report_marks_accounts_unavailable() {
        let temp = TempDir::new().expect("tempdir");
        let path = write_account(&temp);
        print_usage_report(
            &path,
            None,
            false,
            true,
            &NoOAuth,
            &NoXaiOAuth,
            &NoClaudeOAuth,
            &FixtureTransport {
                body: String::new(),
            },
            &empty_xai(),
            &empty_claude(),
        )
        .expect("offline report");
    }

    #[test]
    fn json_report_includes_parsed_windows() {
        let temp = TempDir::new().expect("tempdir");
        let path = write_account(&temp);
        print_usage_report(
            &path,
            None,
            true,
            false,
            &NoOAuth,
            &NoXaiOAuth,
            &NoClaudeOAuth,
            &FixtureTransport {
                body: r#"{"rate_limit":{"primary_window":{"used_percent":10,"limit_window_seconds":18000}}}"#
                    .to_string(),
            },
            &empty_xai(),
            &empty_claude(),
        )
        .expect("json report");
    }

    #[test]
    fn json_report_includes_xai_credits_window() {
        let temp = TempDir::new().expect("tempdir");
        let path = temp.path().join("accounts.json");
        save_store(
            &path,
            &AccountStore {
                accounts: vec![StoredAccount {
                    name: "grok".to_string(),
                    provider: Provider::Xai,
                    account_id: None,
                    access: "access".to_string(),
                    refresh: None,
                    expires: None,
                }],
            },
        )
        .expect("save");
        print_usage_report(
            &path,
            None,
            true,
            false,
            &NoOAuth,
            &NoXaiOAuth,
            &NoClaudeOAuth,
            &FixtureTransport {
                body: String::new(),
            },
            &FixtureXaiTransport {
                billing: r#"{"config":{"creditUsagePercent":10,"currentPeriod":{"start":"2026-08-20T00:00:00Z","end":"2026-08-27T00:00:00Z"}}}"#.to_string(),
                settings: r#"{"subscription_tier_display":"SuperGrok"}"#.to_string(),
            },
            &empty_claude(),
        )
        .expect("xai json report");
    }
}
