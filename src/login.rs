//! Explicit account login and import.

use crate::cli::Provider;
use crate::oauth::{
    CodexOAuth, DEVICE_CODE_TIMEOUT, DEVICE_VERIFICATION_URI, DevicePoll, OAuthTokenSet,
    account_id_from_access_token,
};
use crate::store::{StoredAccount, insert_account, load_store, save_store, upsert_account};
use crate::xai::{
    XaiDevicePoll, XaiOAuth, imported_xai_account_from_json,
    stored_from_tokens as stored_xai_from_tokens,
};
use eyre::{Result, WrapErr, eyre};
use serde::{Deserialize, Serialize};
use std::io::{self, Write};
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

/// Machine-readable login progress for `--json`.
#[derive(Debug, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum LoginEvent {
    /// Device-code details the user must enter in a browser.
    DeviceCode {
        /// Browser verification URL.
        verification_uri: String,
        /// Short user code.
        user_code: String,
    },
    /// Tokens were stored for `name`.
    Complete {
        /// Local account alias.
        name: String,
        /// Provider identifier.
        provider: String,
    },
}

/// How login progress is printed.
#[derive(Clone, Copy, Debug)]
struct LoginReporter {
    /// Emit NDJSON on stdout instead of human stderr.
    json: bool,
}

impl LoginReporter {
    /// Show the device-code URL and user code.
    fn device_code(self, verification_uri: &str, user_code: &str) -> Result<()> {
        if self.json {
            write_json_event(&LoginEvent::DeviceCode {
                verification_uri: verification_uri.to_string(),
                user_code: user_code.to_string(),
            })
        } else {
            let mut err = io::stderr().lock();
            writeln!(err, "Visit {verification_uri} and enter code {user_code}")?;
            writeln!(err, "Waiting for authorization...")?;
            Ok(())
        }
    }

    /// Show that tokens were stored.
    fn complete(self, name: &str, provider: Provider) -> Result<()> {
        if self.json {
            write_json_event(&LoginEvent::Complete {
                name: name.to_string(),
                provider: provider.as_str().to_string(),
            })
        } else {
            let mut err = io::stderr().lock();
            writeln!(err, "Logged in {name}")?;
            Ok(())
        }
    }
}

/// Write one JSON event line and flush.
fn write_json_event(event: &LoginEvent) -> Result<()> {
    let mut stdout = io::stdout().lock();
    serde_json::to_writer(&mut stdout, event).wrap_err("failed to write login JSON event")?;
    writeln!(stdout)?;
    stdout.flush()?;
    Ok(())
}

/// Add one named account by device login or explicit file import.
///
/// Returns an error when the provider is unsupported, the import file is invalid,
/// device login fails, or the store cannot be written.
#[allow(
    clippy::too_many_arguments,
    reason = "login options are expanded at the CLI boundary"
)]
pub(crate) fn login_account(
    store_path: &Path,
    provider: Provider,
    name: &str,
    import: bool,
    auth_file: Option<&Path>,
    replace: bool,
    json: bool,
    oauth: &impl CodexOAuth,
    xai: &impl XaiOAuth,
) -> Result<()> {
    let reporter = LoginReporter { json };
    match provider {
        Provider::Codex if import => {
            let auth_file = auth_file.ok_or_else(|| eyre!("--import requires --auth-file PATH"))?;
            persist_account(
                store_path,
                import_codex_account(name, auth_file)?,
                replace,
                reporter,
            )
        }
        Provider::Codex => {
            let tokens = device_login(oauth, reporter)?;
            persist_account(
                store_path,
                stored_from_tokens(name, tokens),
                replace,
                reporter,
            )
        }
        Provider::Xai if import => {
            let auth_file = auth_file.ok_or_else(|| eyre!("--import requires --auth-file PATH"))?;
            persist_account(
                store_path,
                import_xai_account(name, auth_file)?,
                replace,
                reporter,
            )
        }
        Provider::Xai => {
            let tokens = xai_device_login(xai, reporter)?;
            persist_account(
                store_path,
                stored_xai_from_tokens(name, tokens),
                replace,
                reporter,
            )
        }
    }
}

/// Repeat device login for an existing named account.
///
/// Returns an error when the name is missing or device login fails.
pub(crate) fn reauth_account(
    store_path: &Path,
    name: &str,
    json: bool,
    oauth: &impl CodexOAuth,
    xai: &impl XaiOAuth,
) -> Result<()> {
    let store = load_store(store_path)?;
    let provider = store
        .accounts
        .iter()
        .find(|account| account.name == name)
        .map(|account| account.provider)
        .ok_or_else(|| {
            eyre!("no account named {name}; run `usage login --provider <provider> --name {name}`")
        })?;
    login_account(
        store_path, provider, name, false, None, true, json, oauth, xai,
    )
}

/// Run Codex device-code login using `oauth`.
fn device_login(oauth: &impl CodexOAuth, reporter: LoginReporter) -> Result<OAuthTokenSet> {
    let session = oauth.start_device_auth()?;
    reporter.device_code(DEVICE_VERIFICATION_URI, &session.user_code)?;
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

/// Run xAI device-code login using `oauth`.
fn xai_device_login(oauth: &impl XaiOAuth, reporter: LoginReporter) -> Result<OAuthTokenSet> {
    let session = oauth.start_device_auth()?;
    reporter.device_code(&session.verification_uri, &session.user_code)?;
    let deadline = Instant::now() + session.expires_in;
    let mut interval = session.interval;
    loop {
        if Instant::now() >= deadline {
            return Err(eyre!("xAI device login timed out"));
        }
        thread::sleep(interval);
        match oauth.poll_device_auth(&session)? {
            XaiDevicePoll::Pending => {}
            XaiDevicePoll::SlowDown => {
                interval = interval.saturating_mul(2).min(Duration::from_secs(15));
            }
            XaiDevicePoll::Complete(tokens) => return Ok(tokens),
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

/// Import xAI tokens from a Grok CLI or Pi auth file.
fn import_xai_account(name: &str, auth_file: &Path) -> Result<StoredAccount> {
    let raw = std::fs::read_to_string(auth_file)
        .wrap_err_with(|| format!("failed to read {}", auth_file.display()))?;
    imported_xai_account_from_json(name, &raw)
        .wrap_err_with(|| format!("failed to import xAI auth from {}", auth_file.display()))
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

/// Insert or replace a named account and persist the store.
fn persist_account(
    store_path: &Path,
    account: StoredAccount,
    replace: bool,
    reporter: LoginReporter,
) -> Result<()> {
    let mut store = load_store(store_path)?;
    let name = account.name.clone();
    let provider = account.provider;
    if replace {
        upsert_account(&mut store, account)?;
    } else {
        insert_account(&mut store, account)?;
    }
    save_store(store_path, &store).wrap_err("failed to write account store")?;
    reporter.complete(&name, provider)
}

#[cfg(test)]
mod tests {
    use super::{imported_account_from_json, login_account, reauth_account};
    use crate::cli::Provider;
    use crate::oauth::{CodexOAuth, DeviceAuthSession, DevicePoll, OAuthTokenSet};
    use crate::store::load_store;
    use crate::xai::{XaiDevicePoll, XaiDeviceSession, XaiOAuth};
    use eyre::{Result, eyre};
    use std::path::Path;
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

    struct RotatingOAuth;

    impl CodexOAuth for RotatingOAuth {
        fn start_device_auth(&self) -> Result<DeviceAuthSession> {
            CompleteOAuth.start_device_auth()
        }

        fn poll_device_auth(&self, session: &DeviceAuthSession) -> Result<DevicePoll> {
            CompleteOAuth.poll_device_auth(session)
        }

        fn exchange_authorization_code(
            &self,
            authorization_code: &str,
            code_verifier: &str,
        ) -> Result<OAuthTokenSet> {
            let mut tokens =
                CompleteOAuth.exchange_authorization_code(authorization_code, code_verifier)?;
            tokens.access = "rotated-access".to_string();
            tokens.refresh = "rotated-refresh".to_string();
            Ok(tokens)
        }

        fn refresh_access_token(&self, _refresh_token: &str) -> Result<OAuthTokenSet> {
            Err(eyre!("refresh unused"))
        }
    }

    struct CompleteXaiOAuth;

    impl XaiOAuth for CompleteXaiOAuth {
        fn start_device_auth(&self) -> Result<XaiDeviceSession> {
            Ok(XaiDeviceSession {
                device_code: "device".to_string(),
                user_code: "CODE".to_string(),
                verification_uri: "https://auth.x.ai/device".to_string(),
                interval: Duration::from_millis(1),
                expires_in: Duration::from_secs(60),
            })
        }

        fn poll_device_auth(&self, _session: &XaiDeviceSession) -> Result<XaiDevicePoll> {
            Ok(XaiDevicePoll::Complete(OAuthTokenSet {
                access: "xai-access".to_string(),
                refresh: "xai-refresh".to_string(),
                expires: 1_234,
                account_id: None,
            }))
        }

        fn refresh_access_token(&self, _refresh_token: &str) -> Result<OAuthTokenSet> {
            Err(eyre!("refresh unused"))
        }
    }

    fn login(
        store_path: &Path,
        provider: Provider,
        name: &str,
        import: bool,
        auth_file: Option<&Path>,
    ) -> Result<()> {
        login_account(
            store_path,
            provider,
            name,
            import,
            auth_file,
            false,
            false,
            &CompleteOAuth,
            &CompleteXaiOAuth,
        )
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
        login(&store_path, Provider::Codex, "work", true, Some(&auth_path)).expect("login");
        let store = load_store(&store_path).expect("load");
        assert_eq!(store.accounts.len(), 1, "one imported account");
        assert_eq!(store.accounts[0].name, "work");
    }

    #[test]
    fn device_login_persists_exchanged_tokens() {
        let temp = TempDir::new().expect("tempdir");
        let store_path = temp.path().join("accounts.json");
        login(&store_path, Provider::Codex, "work", false, None).expect("login");
        let store = load_store(&store_path).expect("load");
        assert_eq!(store.accounts.len(), 1, "one device-login account");
        assert_eq!(store.accounts[0].access, "access");
        assert_eq!(store.accounts[0].refresh.as_deref(), Some("refresh"));
    }

    #[test]
    fn xai_device_login_persists_tokens() {
        let temp = TempDir::new().expect("tempdir");
        let store_path = temp.path().join("accounts.json");
        login(&store_path, Provider::Xai, "grok", false, None).expect("login");
        let store = load_store(&store_path).expect("load");
        assert_eq!(store.accounts.len(), 1, "one xAI account");
        assert_eq!(store.accounts[0].provider, Provider::Xai);
        assert_eq!(store.accounts[0].access, "xai-access");
    }

    #[test]
    fn xai_import_persists_named_account() {
        let temp = TempDir::new().expect("tempdir");
        let store_path = temp.path().join("accounts.json");
        let auth_path = temp.path().join("auth.json");
        std::fs::write(
            &auth_path,
            r#"{"xai":{"type":"oauth","access":"pi-xai","refresh":"r"}}"#,
        )
        .expect("write auth");
        login(&store_path, Provider::Xai, "grok", true, Some(&auth_path)).expect("login");
        let store = load_store(&store_path).expect("load");
        assert_eq!(store.accounts.len(), 1, "one imported xAI account");
        assert_eq!(store.accounts[0].access, "pi-xai");
    }

    #[test]
    fn login_replace_overwrites_existing_name() {
        let temp = TempDir::new().expect("tempdir");
        let store_path = temp.path().join("accounts.json");
        login(&store_path, Provider::Codex, "work", false, None).expect("login");
        login_account(
            &store_path,
            Provider::Codex,
            "work",
            false,
            None,
            true,
            false,
            &RotatingOAuth,
            &CompleteXaiOAuth,
        )
        .expect("replace");
        let store = load_store(&store_path).expect("load");
        assert_eq!(store.accounts.len(), 1, "name is replaced in place");
        assert_eq!(store.accounts[0].access, "rotated-access");
        assert_eq!(
            store.accounts[0].refresh.as_deref(),
            Some("rotated-refresh")
        );
    }

    #[test]
    fn reauth_replaces_tokens_for_existing_name() {
        let temp = TempDir::new().expect("tempdir");
        let store_path = temp.path().join("accounts.json");
        login(&store_path, Provider::Codex, "amp", false, None).expect("login");
        reauth_account(&store_path, "amp", false, &RotatingOAuth, &CompleteXaiOAuth)
            .expect("reauth");
        let store = load_store(&store_path).expect("load");
        assert_eq!(store.accounts.len(), 1, "reauth keeps one account");
        assert_eq!(store.accounts[0].name, "amp");
        assert_eq!(store.accounts[0].access, "rotated-access");
    }

    #[test]
    fn reauth_rejects_unknown_name() {
        let temp = TempDir::new().expect("tempdir");
        let store_path = temp.path().join("accounts.json");
        let error = reauth_account(
            &store_path,
            "missing",
            false,
            &CompleteOAuth,
            &CompleteXaiOAuth,
        )
        .expect_err("missing");
        assert!(
            error.to_string().contains("no account named missing"),
            "{error}"
        );
    }
}
