#![deny(missing_docs)]
#![deny(rustdoc::missing_crate_level_docs)]
#![deny(clippy::pedantic)]
#![deny(clippy::missing_docs_in_private_items)]
#![deny(clippy::missing_errors_doc)]
#![deny(clippy::missing_panics_doc)]
#![deny(clippy::missing_assert_message)]
#![deny(clippy::missing_asserts_for_indexing)]
#![deny(clippy::unwrap_used)]

//! Static remaining-usage reports for explicitly logged-in Codex and xAI accounts.

mod cli;
mod login;
mod oauth;
mod quota;
mod render;
mod report;
mod store;
mod xai;

use clap::Parser;
use cli::{Cli, Command};
use eyre::{Result, WrapErr, eyre};
use login::login_account;
use oauth::LiveCodexOAuth;
use quota::LiveCodexUsageTransport;
use report::print_usage_report;
use std::ffi::OsString;
use std::io::Write;
use std::path::Path;
use store::{StoredAccount, default_store_path, load_store, save_store};
use xai::{LiveXaiOAuth, LiveXaiUsageTransport};

/// Execute the CLI with process arguments.
///
/// # Errors
///
/// Returns an error when argument parsing, store IO, login, or reporting fails.
pub fn run<I>(args: I) -> Result<()>
where
    I: IntoIterator<Item = OsString>,
{
    match Cli::try_parse_from(args) {
        Ok(cli) => execute(cli),
        Err(error) => {
            error.print()?;
            if error.exit_code() == 0 {
                Ok(())
            } else {
                Err(eyre!("{error}"))
            }
        }
    }
}

/// Dispatch one parsed CLI invocation.
fn execute(cli: Cli) -> Result<()> {
    let store_path = cli.store.clone().unwrap_or_else(default_store_path);
    match cli.command {
        Some(Command::Login {
            provider,
            name,
            import,
            auth_file,
        }) => login_account(
            &store_path,
            provider,
            &name,
            import,
            auth_file.as_deref(),
            &LiveCodexOAuth,
            &LiveXaiOAuth,
        ),
        Some(Command::List) => list_accounts(&store_path, cli.json),
        Some(Command::Logout { name }) => logout_account(&store_path, &name),
        None => print_usage_report(
            &store_path,
            cli.account.as_deref(),
            cli.json,
            cli.offline,
            &LiveCodexOAuth,
            &LiveXaiOAuth,
            &LiveCodexUsageTransport,
            &LiveXaiUsageTransport,
        ),
    }
}

/// Print stored account names without secrets.
fn list_accounts(store_path: &Path, json: bool) -> Result<()> {
    let store = load_store(store_path)?;
    if json {
        let summaries = store
            .accounts
            .iter()
            .map(StoredAccount::redacted_summary)
            .collect::<Vec<_>>();
        println!("{}", serde_json::to_string_pretty(&summaries)?);
        return Ok(());
    }

    if store.accounts.is_empty() {
        println!(
            "No accounts. Run `usage login --provider codex --name <name>` or `--provider xai` to add one."
        );
        return Ok(());
    }

    println!("NAME             PROVIDER ACCOUNT");
    for account in &store.accounts {
        println!(
            "{:<16} {:<8} {}",
            account.name,
            account.provider.as_str(),
            account.account_id.as_deref().unwrap_or("-")
        );
    }
    Ok(())
}

/// Remove one named account from the store.
fn logout_account(store_path: &Path, name: &str) -> Result<()> {
    let mut store = load_store(store_path)?;
    let previous_len = store.accounts.len();
    store.accounts.retain(|account| account.name != name);
    if store.accounts.len() == previous_len {
        return Err(eyre!("no account named {name}"));
    }
    save_store(store_path, &store).wrap_err("failed to write account store")?;
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "Removed account {name}")?;
    Ok(())
}
