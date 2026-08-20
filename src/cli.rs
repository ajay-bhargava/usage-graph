//! Command-line interface.

use clap::{Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Static remaining-usage reports for logged-in subscription accounts.
#[derive(Debug, Parser)]
#[command(
    name = "usage",
    version,
    about = "Print remaining Codex and xAI subscription usage for logged-in accounts"
)]
pub(crate) struct Cli {
    /// Emit structured JSON instead of tables.
    #[arg(long, short = 'j', global = true)]
    pub(crate) json: bool,
    /// Limit the report to one named account.
    #[arg(long, global = true, value_name = "NAME")]
    pub(crate) account: Option<String>,
    /// Skip network fetches and print unavailable quota.
    #[arg(long, short = 'O', global = true)]
    pub(crate) offline: bool,
    /// Override the account store path.
    #[arg(long, global = true, value_name = "PATH")]
    pub(crate) store: Option<PathBuf>,
    /// Subcommand to execute. Defaults to printing remaining usage.
    #[command(subcommand)]
    pub(crate) command: Option<Command>,
}

/// Account-management subcommands.
#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Add an account with an explicit login or import.
    Login {
        /// Subscription provider to authenticate.
        #[arg(long, value_enum)]
        provider: Provider,
        /// Local name for this account.
        #[arg(long)]
        name: String,
        /// Import tokens from an auth file instead of device login.
        #[arg(long)]
        import: bool,
        /// Auth file to import. Required with `--import`.
        #[arg(long, value_name = "PATH", requires = "import")]
        auth_file: Option<PathBuf>,
    },
    /// List stored accounts without secrets.
    List,
    /// Remove a stored account.
    Logout {
        /// Local account name to delete.
        name: String,
    },
}

/// Supported subscription providers.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Provider {
    /// Codex subscription via `ChatGPT` OAuth.
    Codex,
    /// xAI Grok subscription.
    Xai,
}

impl Provider {
    /// Return the stable CLI identifier.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Xai => "xai",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Cli;
    use clap::Parser;

    #[test]
    fn default_command_is_the_usage_report() {
        let cli = Cli::parse_from(["usage"]);
        assert!(cli.command.is_none(), "report is the default command");
        assert!(!cli.json, "json defaults to false");
        assert!(!cli.offline, "offline defaults to false");
    }

    #[test]
    fn login_requires_provider_and_name() {
        let cli = Cli::parse_from(["usage", "login", "--provider", "codex", "--name", "work"]);
        match cli.command {
            Some(super::Command::Login { name, import, .. }) => {
                assert_eq!(name, "work");
                assert!(!import, "login is device-code unless --import");
            }
            other => panic!("expected login command, got {other:?}"),
        }
    }
}
