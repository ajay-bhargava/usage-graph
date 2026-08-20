//! Fetch and print remaining usage for stored accounts.

use crate::cli::Provider;
use crate::oauth::CodexOAuth;
use crate::quota::{
    AccountQuota, CodexUsageTransport, QuotaError, quota_error_from_status,
    quota_from_usage_payload,
};
use crate::render::{detect_border_style, render_report, report_json};
use crate::store::{StoredAccount, load_store, save_store};
use eyre::{Result, WrapErr, eyre};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Print remaining usage for stored accounts.
///
/// Returns an error when the store cannot be read, no accounts are present, or JSON
/// serialization fails. Per-account fetch failures are rendered as `unavailable`.
pub(crate) fn print_usage_report(
    store_path: &Path,
    account_filter: Option<&str>,
    json: bool,
    offline: bool,
    oauth: &impl CodexOAuth,
    transport: &impl CodexUsageTransport,
) -> Result<()> {
    let mut store = load_store(store_path)?;
    if store.accounts.is_empty() {
        return Err(eyre!(
            "No accounts. Run `usage login --provider codex --name <name>` to add one."
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
        let quota = quota_for_account(account, offline, now_ms, oauth, transport);
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

/// Fetch remaining usage for one stored account.
fn quota_for_account(
    account: &mut StoredAccount,
    offline: bool,
    now_ms: i64,
    oauth: &impl CodexOAuth,
    transport: &impl CodexUsageTransport,
) -> AccountQuota {
    if offline {
        return unavailable(account, QuotaError::Offline);
    }
    if account.provider != Provider::Codex {
        return unavailable(account, QuotaError::UnsupportedProvider);
    }
    let _ = try_refresh(account, now_ms, oauth);
    match transport.get_usage(account) {
        Ok(response) => {
            if let Some(error) = quota_error_from_status(response.status) {
                return unavailable(account, error);
            }
            match quota_from_usage_payload(&response.body) {
                Ok((plan, windows)) => AccountQuota {
                    name: account.name.clone(),
                    provider: account.provider,
                    plan,
                    windows,
                    error: None,
                },
                Err(error) => unavailable(account, error),
            }
        }
        Err(_) => unavailable(account, QuotaError::RequestFailed),
    }
}

/// Refresh tokens in place without requiring the full store.
fn try_refresh(account: &mut StoredAccount, now_ms: i64, oauth: &impl CodexOAuth) -> Result<()> {
    if !crate::oauth::access_token_needs_refresh(account, now_ms) {
        return Ok(());
    }
    let Some(refresh) = account.refresh.clone() else {
        return Ok(());
    };
    let tokens = oauth.refresh_access_token(&refresh)?;
    account.access = tokens.access;
    account.refresh = Some(tokens.refresh);
    if tokens.account_id.is_some() {
        account.account_id = tokens.account_id;
    }
    account.expires = Some(tokens.expires);
    Ok(())
}

/// Build an unavailable quota record.
fn unavailable(account: &StoredAccount, error: QuotaError) -> AccountQuota {
    AccountQuota {
        name: account.name.clone(),
        provider: account.provider,
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
    use crate::cli::Provider;
    use crate::oauth::{CodexOAuth, DeviceAuthSession, DevicePoll, OAuthTokenSet};
    use crate::quota::{CodexUsageTransport, UsageHttpResponse};
    use crate::store::{AccountStore, StoredAccount, save_store};
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
            &FixtureTransport {
                body: String::new(),
            },
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
            &FixtureTransport {
                body: String::new(),
            },
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
            &FixtureTransport {
                body: r#"{"rate_limit":{"primary_window":{"used_percent":10,"limit_window_seconds":18000}}}"#
                    .to_string(),
            },
        )
        .expect("json report");
    }
}
