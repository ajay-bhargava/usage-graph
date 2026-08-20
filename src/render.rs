//! Static quota table rendering.

use crate::quota::{AccountQuota, QuotaWindow};
use serde::Serialize;
use std::env;
use std::fmt::Write as _;
use std::io::IsTerminal;

/// Horizontal cells used by quota bars.
const LIMIT_BAR_WIDTH: usize = 20;

/// Border style for the rendered table.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BorderStyle {
    /// ASCII-safe borders.
    Ascii,
    /// Unicode box-drawing borders.
    Unicode,
}

/// JSON document emitted by `--json`.
#[derive(Debug, Serialize)]
pub(crate) struct UsageReportJson {
    /// Per-account remaining usage.
    pub(crate) accounts: Vec<AccountQuotaJson>,
}

/// JSON record for one account.
#[derive(Debug, Serialize)]
pub(crate) struct AccountQuotaJson {
    /// Local alias.
    pub(crate) name: String,
    /// Provider identifier.
    pub(crate) provider: String,
    /// Optional plan name.
    pub(crate) plan: Option<String>,
    /// Displayable windows.
    pub(crate) windows: Vec<QuotaWindowJson>,
    /// Fetch or parse error.
    pub(crate) error: Option<String>,
}

/// JSON record for one quota window.
#[derive(Debug, Serialize)]
pub(crate) struct QuotaWindowJson {
    /// Bucket id.
    pub(crate) bucket: String,
    /// Row label.
    pub(crate) label: String,
    /// Used percentage.
    pub(crate) used_percent: f64,
    /// Remaining percentage.
    pub(crate) left_percent: f64,
    /// Reset timestamp, when known.
    pub(crate) resets_at: Option<i64>,
    /// Locally computed countdown, when known.
    pub(crate) reset_in: Option<String>,
}

/// Render a human-readable remaining-usage report.
#[must_use]
pub(crate) fn render_report(
    accounts: &[AccountQuota],
    borders: BorderStyle,
    now_epoch_seconds: i64,
) -> String {
    let mut output = String::from("Subscription Remaining\n");
    for account in accounts {
        output.push('\n');
        output.push_str(&render_account(account, borders, now_epoch_seconds));
        output.push('\n');
    }
    output
}

/// Render one account block.
fn render_account(account: &AccountQuota, borders: BorderStyle, now_epoch_seconds: i64) -> String {
    let title = match &account.plan {
        Some(plan) => format!("{}  ({}, {plan})", account.name, account.provider.as_str()),
        None => format!("{}  ({})", account.name, account.provider.as_str()),
    };
    let rows = account_rows(account, borders, now_epoch_seconds);
    format!("{title}\n{}", render_table(&rows, borders))
}

/// Build table rows for one account.
fn account_rows(
    account: &AccountQuota,
    borders: BorderStyle,
    now_epoch_seconds: i64,
) -> Vec<String> {
    if let Some(error) = account.error {
        return vec![format!("unavailable ({})", error.as_str())];
    }
    if account.windows.is_empty() {
        return vec!["unavailable (no limit data)".to_string()];
    }
    let label_width = account
        .windows
        .iter()
        .map(|window| window.label.chars().count())
        .max()
        .unwrap_or(2)
        .max(2);
    account
        .windows
        .iter()
        .map(|window| render_window_row(window, label_width, borders, now_epoch_seconds))
        .collect()
}

/// Render one quota window row.
fn render_window_row(
    window: &QuotaWindow,
    label_width: usize,
    borders: BorderStyle,
    now_epoch_seconds: i64,
) -> String {
    let left_percent = rounded_limit_percent(100.0 - window.used_percent);
    let reset_suffix = limit_reset_countdown(window, now_epoch_seconds)
        .map(|reset| format!(", {reset}"))
        .unwrap_or_default();
    format!(
        "{:<label_width$} [{}] {left_percent:>3}% left{reset_suffix}",
        window.label,
        format_limit_bar(100.0 - window.used_percent, borders),
        label_width = label_width
    )
}

/// Format one fixed-width remaining-usage bar.
fn format_limit_bar(left_percent: f64, borders: BorderStyle) -> String {
    let filled = limit_bar_filled_cells(left_percent);
    let empty = LIMIT_BAR_WIDTH.saturating_sub(filled);
    match borders {
        BorderStyle::Ascii => format!("{}{}", "#".repeat(filled), "-".repeat(empty)),
        BorderStyle::Unicode => format!("{}{}", "█".repeat(filled), "░".repeat(empty)),
    }
}

/// Return how many cells should be filled for one remaining percentage.
fn limit_bar_filled_cells(left_percent: f64) -> usize {
    let left_percent = left_percent.clamp(0.0, 100.0);
    let width = u32::try_from(LIMIT_BAR_WIDTH).map_or(20.0, f64::from);
    let filled_units = left_percent * width / 100.0;
    (0..LIMIT_BAR_WIDTH)
        .filter(|index| {
            let index = u32::try_from(*index).map_or(0.0, f64::from);
            index + 0.5 <= filled_units
        })
        .count()
}

/// Format a whole-number percentage for limit display.
fn rounded_limit_percent(value: f64) -> String {
    format!("{:.0}", value.clamp(0.0, 100.0))
}

/// Format the locally computed reset countdown for one window.
fn limit_reset_countdown(window: &QuotaWindow, now_epoch_seconds: i64) -> Option<String> {
    let resets_at = window.resets_at_epoch_seconds?;
    Some(format_reset_countdown(
        resets_at.saturating_sub(now_epoch_seconds).max(0),
    ))
}

/// Format a duration as up to two non-zero major units.
fn format_reset_countdown(total_seconds: i64) -> String {
    let mut remaining = total_seconds.max(0);
    let mut parts = Vec::with_capacity(2);
    for (unit_seconds, unit_label) in [(86_400, "d"), (3_600, "h"), (60, "m"), (1, "s")] {
        let value = remaining / unit_seconds;
        remaining %= unit_seconds;
        if value > 0 || (parts.is_empty() && unit_seconds == 1) {
            parts.push(format!("{value}{unit_label}"));
        }
        if parts.len() == 2 {
            break;
        }
    }
    parts.join(" ")
}

/// Render a one-column ASCII or Unicode table.
fn render_table(rows: &[String], borders: BorderStyle) -> String {
    let width = rows
        .iter()
        .map(|row| row.chars().count())
        .max()
        .unwrap_or(0);
    let (horizontal, vertical, top_left, top_right, bottom_left, bottom_right) = match borders {
        BorderStyle::Ascii => ('-', '|', '+', '+', '+', '+'),
        BorderStyle::Unicode => ('─', '│', '┌', '┐', '└', '┘'),
    };
    let rule = |left: char, right: char| {
        format!("{left}{}{right}", horizontal.to_string().repeat(width + 2))
    };
    let mut output = String::new();
    let _ = writeln!(&mut output, "{}", rule(top_left, top_right));
    for row in rows {
        let padding = width.saturating_sub(row.chars().count());
        let _ = writeln!(
            &mut output,
            "{vertical} {row}{} {vertical}",
            " ".repeat(padding)
        );
    }
    let _ = write!(&mut output, "{}", rule(bottom_left, bottom_right));
    output
}

/// Convert account quotas into JSON.
#[must_use]
pub(crate) fn report_json(accounts: &[AccountQuota], now_epoch_seconds: i64) -> UsageReportJson {
    UsageReportJson {
        accounts: accounts
            .iter()
            .map(|account| AccountQuotaJson {
                name: account.name.clone(),
                provider: account.provider.as_str().to_string(),
                plan: account.plan.clone(),
                windows: account
                    .windows
                    .iter()
                    .map(|window| QuotaWindowJson {
                        bucket: window.bucket.clone(),
                        label: window.label.clone(),
                        used_percent: window.used_percent,
                        left_percent: (100.0 - window.used_percent).clamp(0.0, 100.0),
                        resets_at: window.resets_at_epoch_seconds,
                        reset_in: limit_reset_countdown(window, now_epoch_seconds),
                    })
                    .collect(),
                error: account.error.map(|error| error.as_str().to_string()),
            })
            .collect(),
    }
}

/// Detect the best border style for the current stdout stream.
#[must_use]
pub(crate) fn detect_border_style() -> BorderStyle {
    detect_border_style_for(
        std::io::stdout().is_terminal(),
        env::var("LC_ALL").ok().as_deref(),
        env::var("LC_CTYPE").ok().as_deref(),
        env::var("LANG").ok().as_deref(),
    )
}

/// Decide whether Unicode box-drawing is safe for the current environment.
fn detect_border_style_for(
    stdout_is_terminal: bool,
    lc_all: Option<&str>,
    lc_ctype: Option<&str>,
    lang: Option<&str>,
) -> BorderStyle {
    if !stdout_is_terminal {
        return BorderStyle::Ascii;
    }
    let locale = lc_all
        .filter(|value| !value.is_empty())
        .or(lc_ctype.filter(|value| !value.is_empty()))
        .or(lang.filter(|value| !value.is_empty()))
        .unwrap_or_default()
        .to_ascii_lowercase();
    if locale.contains("utf-8") || locale.contains("utf8") {
        BorderStyle::Unicode
    } else {
        BorderStyle::Ascii
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BorderStyle, format_limit_bar, format_reset_countdown, render_report, rounded_limit_percent,
    };
    use crate::cli::Provider;
    use crate::quota::{AccountQuota, QuotaError, QuotaWindow};

    #[test]
    fn limit_bars_show_usage_left_with_border_specific_glyphs() {
        assert_eq!(
            format_limit_bar(50.0, BorderStyle::Ascii),
            "##########----------"
        );
        assert_eq!(
            format_limit_bar(50.0, BorderStyle::Unicode),
            "██████████░░░░░░░░░░"
        );
    }

    #[test]
    fn reset_countdown_uses_two_non_zero_major_units() {
        assert_eq!(format_reset_countdown(0), "0s");
        assert_eq!(format_reset_countdown(59), "59s");
        assert_eq!(format_reset_countdown(62), "1m 2s");
        assert_eq!(format_reset_countdown(3_600), "1h");
        assert_eq!(
            format_reset_countdown((3 * 86_400) + (2 * 3_600) + 300),
            "3d 2h"
        );
    }

    #[test]
    fn report_renders_account_windows_and_errors() {
        let rendered = render_report(
            &[
                AccountQuota {
                    name: "work".to_string(),
                    provider: Provider::Codex,
                    plan: Some("pro".to_string()),
                    windows: vec![
                        QuotaWindow {
                            bucket: "codex".to_string(),
                            label: "5h".to_string(),
                            used_percent: 42.0,
                            window_minutes: Some(300),
                            resets_at_epoch_seconds: Some(100 + (3 * 86_400) + (2 * 3_600)),
                        },
                        QuotaWindow {
                            bucket: "codex".to_string(),
                            label: "Weekly".to_string(),
                            used_percent: 9.0,
                            window_minutes: Some(10_080),
                            resets_at_epoch_seconds: Some(145),
                        },
                    ],
                    error: None,
                },
                AccountQuota {
                    name: "broken".to_string(),
                    provider: Provider::Codex,
                    plan: None,
                    windows: Vec::new(),
                    error: Some(QuotaError::Unauthorized),
                },
            ],
            BorderStyle::Ascii,
            100,
        );
        assert!(rendered.contains("Subscription Remaining"));
        assert!(rendered.contains("work  (codex, pro)"));
        assert!(rendered.contains("5h     [############--------]  58% left, 3d 2h"));
        assert!(rendered.contains("Weekly [##################--]  91% left, 45s"));
        assert!(rendered.contains("unavailable (unauthorized)"));
        assert_eq!(rounded_limit_percent(58.2), "58");
    }
}
