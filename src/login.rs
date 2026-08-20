//! Explicit account login and import.

use crate::cli::Provider;
use crate::oauth::{
    CodexOAuth, DEVICE_CODE_TIMEOUT, DEVICE_VERIFICATION_URI, DevicePoll, OAuthTokenSet,
    account_id_from_access_token,
};
use crate::store::{StoredAccount, insert_account, load_store, save_store};
use eyre::{Result, WrapErr, eyre};
use serde::Deserialize;
use std::io::{self, Write};
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

/// Add one named account by device login or explicit file import.
///
/// Returns an error when the provider is unsupported, the import file is invalid,
/// device login fails, or the store cannot be written.
pub(crate) fn login_account(
    store_path: &Path,
    provider: Provider,
    name: &str,
    import: bool,
    auth_file: Option<&Path>,
    oauth: &impl CodexOAuth,
) -> Result<()> {
    match provider {
        Provider::Xai => Err(eyre!(
            "xAI login is not implemented yet; Codex accounts are supported in this release"
        )),
        Provider::Codex if import => {
            let auth_file = auth_file.ok_or_else(|| eyre!("--import requires --auth-file PATH"))?;
            let account = import_codex_account(name, auth_file)?;
            persist_new_account(store_path, account)
        }
        Provider::Codex => {
            let tokens = device_login(oauth, &mut io::stderr())?;
            persist_new_account(store_path, stored_from_tokens(name, tokens))
        }
    }
}

/// Run Codex device-code login using `oauth`.
fn device_login(oauth: &impl CodexOAuth, err: &mut impl Write) -> Result<OAuthTokenSet> {
    let session = oauth.start_device_auth()?;
    writeln!(
        err,
        "Visit {DEVICE_VERIFICATION_URI} and enter code {}",
        session.user_code
    )?;
    writeln!(err, "Waiting for authorization...")?;
    let deadline = Instant::now() + DEVICE_CODE_TIMEOUT;
    let mut interval = session.interval;
    loop {
        if Instant::now() >= deadline {
            return Err(eyre!("Codex device login timed out"));
        }
        thread::sleep(interval);
        match oauth.poll_device_auth(&session)? {
            DevicePoll::Pending => {}
            DevicePoll::SlowDown => {
                interval = interval.saturating_mul(2).min(Duration::from_secs(15));
            }
            DevicePoll::Complete {
                authorization_code,
                code_verifier,
            } => {
                return oauth.exchange_authorization_code(&authorization_code, &code_verifier);
            }
        }
    }
}

/// Import Codex tokens from a Codex CLI or Pi auth file.
fn import_codex_account(name: &str, auth_file: &Path) -> Result<StoredAccount> {
    let raw = std::fs::read_to_string(auth_file)
        .wrap_err_with(|| format!("failed to read {}", auth_file.display()))?;
    imported_account_from_json(name, &raw)
        .wrap_err_with(|| format!("failed to import Codex auth from {}", auth_file.display()))
}

/// Parse Codex CLI `auth.json` or Pi `openai-codex` credentials.
fn imported_account_from_json(name: &str, raw: &str) -> Result<StoredAccount> {
    let value: serde_json::Value =
        serde_json::from_str(raw).wrap_err("auth file is not valid JSON")?;
    if let Some(account) = pi_codex_account(name, &value)? {
        return Ok(account);
    }
    if let Some(account) = codex_cli_account(name, raw)? {
        return Ok(account);
    }
    Err(eyre!(
        "auth file does not contain Codex subscription tokens (API keys are not supported)"
    ))
}

/// Pi `openai-codex` OAuth credential subset.
#[derive(Deserialize)]
struct PiCodexAuth {
    /// Credential kind.
    #[serde(rename = "type")]
    kind: Option<String>,
    /// Access token.
    access: Option<String>,
    /// Refresh token.
    refresh: Option<String>,
    /// Expiry in Unix milliseconds.
    expires: Option<i64>,
    /// Account id.
    #[serde(rename = "accountId")]
    account_id: Option<String>,
}

/// Read Pi `openai-codex` OAuth credentials.
fn pi_codex_account(name: &str, value: &serde_json::Value) -> Result<Option<StoredAccount>> {
    let Some(entry) = value.get("openai-codex") else {
        return Ok(None);
    };
    let auth: PiCodexAuth =
        serde_json::from_value(entry.clone()).wrap_err("invalid Pi openai-codex auth entry")?;
    if auth.kind.as_deref().is_some_and(|kind| kind != "oauth") {
        return Err(eyre!("Pi openai-codex entry is not OAuth"));
    }
    let access = auth
        .access
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| eyre!("Pi openai-codex entry missing access token"))?;
    Ok(Some(StoredAccount {
        name: name.to_string(),
        provider: Provider::Codex,
        account_id: auth
            .account_id
            .or_else(|| account_id_from_access_token(access)),
        access: access.to_string(),
        refresh: auth.refresh.filter(|value| !value.trim().is_empty()),
        expires: auth.expires,
    }))
}

/// Codex CLI `auth.json` token subset.
#[derive(Debug, Deserialize)]
struct CodexCliAuthFile {
    /// Codex CLI token blob.
    tokens: Option<CodexCliTokens>,
}

/// Token subset stored in Codex CLI `auth.json`.
#[derive(Debug, Deserialize)]
struct CodexCliTokens {
    /// Bearer access token.
    access_token: String,
    /// Refresh token.
    #[serde(default)]
    refresh_token: Option<String>,
    /// Account id.
    #[serde(default)]
    account_id: Option<String>,
}

/// Read Codex CLI `tokens` credentials.
fn codex_cli_account(name: &str, raw: &str) -> Result<Option<StoredAccount>> {
    let auth: CodexCliAuthFile =
        serde_json::from_str(raw).wrap_err("invalid Codex CLI auth.json")?;
    let Some(tokens) = auth.tokens else {
        return Ok(None);
    };
    let access = tokens.access_token.trim();
    if access.is_empty() {
        return Err(eyre!("Codex CLI auth.json has an empty access token"));
    }
    Ok(Some(StoredAccount {
        name: name.to_string(),
        provider: Provider::Codex,
        account_id: tokens
            .account_id
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .or_else(|| account_id_from_access_token(access)),
        access: access.to_string(),
        refresh: tokens
            .refresh_token
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
        expires: None,
    }))
}

/// Convert OAuth tokens into a stored Codex account.
fn stored_from_tokens(name: &str, tokens: OAuthTokenSet) -> StoredAccount {
    StoredAccount {
        name: name.to_string(),
        provider: Provider::Codex,
        account_id: tokens.account_id,
        access: tokens.access,
        refresh: Some(tokens.refresh),
        expires: Some(tokens.expires),
    }
}

/// Insert a new account and persist the store.
fn persist_new_account(store_path: &Path, account: StoredAccount) -> Result<()> {
    let mut store = load_store(store_path)?;
    let name = account.name.clone();
    insert_account(&mut store, account)?;
    save_store(store_path, &store)?;
    eprintln!("Logged in {name}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{imported_account_from_json, login_account};
    use crate::cli::Provider;
    use crate::oauth::{CodexOAuth, DeviceAuthSession, DevicePoll, OAuthTokenSet};
    use crate::store::load_store;
    use eyre::{Result, eyre};
    use std::time::Duration;
    use tempfile::TempDir;

    struct CompleteOAuth;

    impl CodexOAuth for CompleteOAuth {
        fn start_device_auth(&self) -> Result<DeviceAuthSession> {
            Ok(DeviceAuthSession {
                device_auth_id: "device".to_string(),
                user_code: "CODE".to_string(),
                interval: Duration::from_millis(1),
            })
        }

        fn poll_device_auth(&self, _session: &DeviceAuthSession) -> Result<DevicePoll> {
            Ok(DevicePoll::Complete {
                authorization_code: "auth-code".to_string(),
                code_verifier: "verifier".to_string(),
            })
        }

        fn exchange_authorization_code(
            &self,
            authorization_code: &str,
            code_verifier: &str,
        ) -> Result<OAuthTokenSet> {
            assert_eq!(authorization_code, "auth-code");
            assert_eq!(code_verifier, "verifier");
            Ok(OAuthTokenSet {
                access: "access".to_string(),
                refresh: "refresh".to_string(),
                expires: 1_234,
                account_id: Some("acct".to_string()),
            })
        }

        fn refresh_access_token(&self, _refresh_token: &str) -> Result<OAuthTokenSet> {
            Err(eyre!("refresh unused"))
        }
    }

    #[test]
    fn import_reads_codex_cli_tokens() {
        let account = imported_account_from_json(
            "work",
            r#"{"tokens":{"access_token":" access ","refresh_token":" refresh ","account_id":" acct "}}"#,
        )
        .expect("import");
        assert_eq!(account.name, "work");
        assert_eq!(account.access, "access");
        assert_eq!(account.refresh.as_deref(), Some("refresh"));
        assert_eq!(account.account_id.as_deref(), Some("acct"));
    }

    #[test]
    fn import_reads_pi_oauth_entry() {
        let account = imported_account_from_json(
            "pi",
            r#"{"openai-codex":{"type":"oauth","access":"pi-access","refresh":"pi-refresh","accountId":"pi-acct","expires":9}}"#,
        )
        .expect("import");
        assert_eq!(account.access, "pi-access");
        assert_eq!(account.account_id.as_deref(), Some("pi-acct"));
        assert_eq!(account.expires, Some(9));
    }

    #[test]
    fn import_rejects_api_keys() {
        let error = imported_account_from_json("work", r#"{"OPENAI_API_KEY":"sk-test"}"#)
            .expect_err("api key");
        assert!(
            error.to_string().contains("API keys are not supported"),
            "{error}"
        );
    }

    #[test]
    fn login_import_persists_named_account() {
        let temp = TempDir::new().expect("tempdir");
        let store_path = temp.path().join("accounts.json");
        let auth_path = temp.path().join("auth.json");
        std::fs::write(
            &auth_path,
            r#"{"tokens":{"access_token":"access","account_id":"acct"}}"#,
        )
        .expect("write auth");
        login_account(
            &store_path,
            Provider::Codex,
            "work",
            true,
            Some(&auth_path),
            &CompleteOAuth,
        )
        .expect("login");
        let store = load_store(&store_path).expect("load");
        assert_eq!(store.accounts.len(), 1, "one imported account");
        assert_eq!(store.accounts[0].name, "work");
    }

    #[test]
    fn device_login_persists_exchanged_tokens() {
        let temp = TempDir::new().expect("tempdir");
        let store_path = temp.path().join("accounts.json");
        login_account(
            &store_path,
            Provider::Codex,
            "work",
            false,
            None,
            &CompleteOAuth,
        )
        .expect("login");
        let store = load_store(&store_path).expect("load");
        assert_eq!(store.accounts.len(), 1, "one device-login account");
        assert_eq!(store.accounts[0].access, "access");
        assert_eq!(store.accounts[0].refresh.as_deref(), Some("refresh"));
    }

    #[test]
    fn xai_login_is_rejected() {
        let temp = TempDir::new().expect("tempdir");
        let error = login_account(
            &temp.path().join("accounts.json"),
            Provider::Xai,
            "grok",
            false,
            None,
            &CompleteOAuth,
        )
        .expect_err("xai");
        assert!(error.to_string().contains("not implemented"), "{error}");
    }
}
