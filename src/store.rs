//! Named account store.

use crate::cli::Provider;
use eyre::{Result, WrapErr, eyre};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

/// On-disk account collection.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct AccountStore {
    /// Explicitly logged-in accounts.
    #[serde(default)]
    pub(crate) accounts: Vec<StoredAccount>,
}

/// One stored subscription account.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct StoredAccount {
    /// Local alias used by `--account` and `logout`.
    pub(crate) name: String,
    /// Provider that issued the tokens.
    pub(crate) provider: Provider,
    /// Optional account or workspace id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) account_id: Option<String>,
    /// Bearer access token.
    pub(crate) access: String,
    /// Refresh token used to rotate access.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) refresh: Option<String>,
    /// Access-token expiry in Unix milliseconds, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) expires: Option<i64>,
}

/// JSON summary that never includes secrets.
#[derive(Debug, Serialize)]
pub(crate) struct AccountSummary {
    /// Local alias.
    pub(crate) name: String,
    /// Provider identifier.
    pub(crate) provider: Provider,
    /// Optional account id.
    pub(crate) account_id: Option<String>,
    /// Access-token expiry in Unix milliseconds, when known.
    pub(crate) expires: Option<i64>,
}

impl StoredAccount {
    /// Return a redacted listing record.
    pub(crate) fn redacted_summary(&self) -> AccountSummary {
        AccountSummary {
            name: self.name.clone(),
            provider: self.provider,
            account_id: self.account_id.clone(),
            expires: self.expires,
        }
    }
}

/// Resolve the default store path under the user config directory.
#[must_use]
pub(crate) fn default_store_path() -> PathBuf {
    default_store_path_from(
        dirs::config_dir(),
        std::env::var_os("HOME").map(PathBuf::from),
    )
}

/// Resolve the default store path from environment values.
fn default_store_path_from(config_dir: Option<PathBuf>, home: Option<PathBuf>) -> PathBuf {
    config_dir
        .or_else(|| home.map(|home| home.join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"))
        .join("usage-cli")
        .join("accounts.json")
}

/// Load the account store, or an empty store when the file is missing.
///
/// Returns an error when the file exists but cannot be read or parsed.
pub(crate) fn load_store(path: &Path) -> Result<AccountStore> {
    match fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(&raw)
            .wrap_err_with(|| format!("failed to parse account store {}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(AccountStore::default()),
        Err(error) => {
            Err(error).wrap_err_with(|| format!("failed to read account store {}", path.display()))
        }
    }
}

/// Persist the account store with restrictive permissions.
///
/// Returns an error when the parent directory or file cannot be written.
pub(crate) fn save_store(path: &Path, store: &AccountStore) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .wrap_err_with(|| format!("failed to create {}", parent.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).wrap_err_with(|| {
                format!("failed to restrict permissions on {}", parent.display())
            })?;
        }
    }

    let raw = serde_json::to_string_pretty(store)?;
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .wrap_err_with(|| format!("failed to open {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .wrap_err_with(|| format!("failed to restrict permissions on {}", path.display()))?;
    }
    file.write_all(raw.as_bytes())
        .wrap_err_with(|| format!("failed to write {}", path.display()))?;
    file.write_all(b"\n")?;
    Ok(())
}

/// Insert or reject a named account.
///
/// Returns an error when `name` is empty or already present.
pub(crate) fn insert_account(store: &mut AccountStore, account: StoredAccount) -> Result<()> {
    validate_account_name(&account.name)?;
    if store
        .accounts
        .iter()
        .any(|existing| existing.name == account.name)
    {
        return Err(eyre!(
            "account {} already exists; logout first, pass --replace, or run `usage reauth {}`",
            account.name,
            account.name
        ));
    }
    store.accounts.push(account);
    Ok(())
}

/// Insert a named account, or replace the existing account with the same name.
///
/// Returns an error when `name` is empty.
pub(crate) fn upsert_account(store: &mut AccountStore, account: StoredAccount) -> Result<()> {
    validate_account_name(&account.name)?;
    if let Some(existing) = store
        .accounts
        .iter_mut()
        .find(|existing| existing.name == account.name)
    {
        *existing = account;
        return Ok(());
    }
    store.accounts.push(account);
    Ok(())
}

/// Return whether `name` is a usable local alias.
fn validate_account_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(eyre!("account name must not be empty"));
    }
    if name.chars().any(char::is_whitespace) {
        return Err(eyre!("account name must not contain whitespace"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        AccountStore, StoredAccount, default_store_path_from, insert_account, load_store,
        save_store,
    };
    use crate::cli::Provider;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn sample_account(name: &str) -> StoredAccount {
        StoredAccount {
            name: name.to_string(),
            provider: Provider::Codex,
            account_id: Some("acct-1".to_string()),
            access: "access-token".to_string(),
            refresh: Some("refresh-token".to_string()),
            expires: Some(1_700_000_000_000),
        }
    }

    #[test]
    fn default_store_path_prefers_config_dir() {
        let path = default_store_path_from(
            Some(PathBuf::from("/tmp/config")),
            Some(PathBuf::from("/tmp/home")),
        );
        assert_eq!(path, PathBuf::from("/tmp/config/usage-cli/accounts.json"));
    }

    #[test]
    fn default_store_path_falls_back_to_home_config() {
        let path = default_store_path_from(None, Some(PathBuf::from("/tmp/home")));
        assert_eq!(
            path,
            PathBuf::from("/tmp/home/.config/usage-cli/accounts.json")
        );
    }

    #[test]
    fn missing_store_is_empty() {
        let temp = TempDir::new().expect("tempdir");
        let store = load_store(&temp.path().join("missing.json")).expect("load");
        assert!(store.accounts.is_empty(), "missing file is an empty store");
    }

    #[test]
    fn round_trip_persists_accounts_without_clobbering_names() {
        let temp = TempDir::new().expect("tempdir");
        let path = temp.path().join("usage-cli").join("accounts.json");
        let mut store = AccountStore::default();
        insert_account(&mut store, sample_account("work")).expect("insert");
        save_store(&path, &store).expect("save");
        let loaded = load_store(&path).expect("load");
        assert_eq!(loaded, store);

        let duplicate = insert_account(&mut store, sample_account("work"));
        assert!(duplicate.is_err(), "duplicate names must be rejected");

        let mut replacement = sample_account("work");
        replacement.access = "rotated".to_string();
        crate::store::upsert_account(&mut store, replacement).expect("upsert");
        assert_eq!(store.accounts.len(), 1, "upsert replaces in place");
        assert_eq!(store.accounts[0].access, "rotated");
    }

    #[cfg(unix)]
    #[test]
    fn save_store_sets_private_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let temp = TempDir::new().expect("tempdir");
        let path = temp.path().join("usage-cli").join("accounts.json");
        let mut store = AccountStore::default();
        insert_account(&mut store, sample_account("work")).expect("insert");
        save_store(&path, &store).expect("save");
        let file_mode = std::fs::metadata(&path)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777;
        let dir_mode = std::fs::metadata(path.parent().expect("parent"))
            .expect("dir metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(file_mode, 0o600);
        assert_eq!(dir_mode, 0o700);
    }
}
