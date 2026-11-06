import { spawn } from "node:child_process";
import { readFile } from "node:fs/promises";
import { homedir } from "node:os";
import { join } from "node:path";

import type {
  ExtensionAPI,
  ExtensionContext,
} from "@earendil-works/pi-coding-agent";

/**
 * Claude (Pro/Max) remaining-quota status bar for Pi.
 *
 * Sources, in order:
 * 1. `usage --json` from the usage CLI, when a `claude` account is logged in
 *    (`usage login --provider claude --name max`). Pin one with PI_CLAUDE_USAGE_ACCOUNT.
 * 2. Claude Code's own `~/.claude/.credentials.json`, read-only. Claude Code keeps
 *    that token fresh; this extension never refreshes or writes it.
 */

const CLAUDE_PROVIDERS = new Set(["claude-code", "anthropic"]);
const STATUS_KEY = "ab-claude-usage";
const LABEL = "claude";
const USAGE_URL = "https://api.anthropic.com/api/oauth/usage";
const REFRESH_INTERVAL_MS = 120_000;
const MIN_REFETCH_MS = 30_000;
const FETCH_TIMEOUT_MS = 8_000;
const BAR_WIDTH = 10;
const DAY_MS = 86_400_000;
const HOUR_MS = 3_600_000;
const MINUTE_MS = 60_000;
const DUAL_BAR_CHARS = [
  "⠀", "▘", "▝", "▀", "▖", "▌", "▞", "▛",
  "▗", "▚", "▐", "▜", "▄", "▙", "▟", "█",
];

export type ClaudeWindow = {
  label: string;
  usedPercent: number;
  resetAt?: number;
};

export type ClaudeUsage = {
  source: "usage-cli" | "claude-code";
  account?: string;
  plan?: string;
  windows: ClaudeWindow[];
};

type Model = Pick<NonNullable<ExtensionContext["model"]>, "provider">;

export function isClaudeModel(model: Model | undefined): boolean {
  return model !== undefined && CLAUDE_PROVIDERS.has(model.provider);
}

/** Parse `/api/oauth/usage` (Claude Code `/usage`) into windows. */
export function parseOAuthUsage(payload: unknown): ClaudeWindow[] {
  if (!isRecord(payload)) return [];
  const windows: ClaudeWindow[] = [];
  const named: Array<[string, string]> = [
    ["five_hour", "5h"],
    ["seven_day", "Weekly"],
    ["seven_day_opus", "Opus Weekly"],
    ["seven_day_sonnet", "Sonnet Weekly"],
  ];
  for (const [key, label] of named) {
    const bucket = payload[key];
    if (!isRecord(bucket)) continue;
    const used = asNumber(bucket.utilization);
    if (used === undefined) continue;
    windows.push({ label, usedPercent: clamp(used), resetAt: asTime(bucket.resets_at) });
  }
  const limits = Array.isArray(payload.limits) ? payload.limits : [];
  for (const limit of limits) {
    if (!isRecord(limit)) continue;
    const kind = limit.kind;
    let label: string | undefined;
    if (kind === "session") label = "5h";
    else if (kind === "weekly_all") label = "Weekly";
    else if (kind === "weekly_scoped") {
      const scope = isRecord(limit.scope) ? limit.scope : undefined;
      const model = scope && isRecord(scope.model) ? scope.model : undefined;
      const name = asString(model?.display_name) ?? asString(model?.id) ?? asString(scope?.surface);
      if (name) label = `${name} Weekly`;
    }
    if (!label || windows.some((window) => window.label === label)) continue;
    const used = asNumber(limit.percent);
    if (used === undefined) continue;
    windows.push({ label, usedPercent: clamp(used), resetAt: asTime(limit.resets_at) });
  }
  return windows;
}

/** Pick the claude account from `usage --json` output. */
export function parseUsageCliReport(
  payload: unknown,
  preferred?: string,
): ClaudeUsage | undefined {
  if (!isRecord(payload) || !Array.isArray(payload.accounts)) return undefined;
  const accounts = payload.accounts.filter(
    (account): account is Record<string, unknown> =>
      isRecord(account) && account.provider === "claude",
  );
  const account = preferred
    ? accounts.find((entry) => entry.name === preferred)
    : accounts.find((entry) => !entry.error) ?? accounts[0];
  if (!account || account.error) return undefined;
  const rawWindows = Array.isArray(account.windows) ? account.windows : [];
  const windows = rawWindows.filter(isRecord).flatMap((window) => {
    const used = asNumber(window.used_percent);
    const label = asString(window.label);
    if (used === undefined || !label) return [];
    const resets = asNumber(window.resets_at);
    return [{ label, usedPercent: clamp(used), resetAt: resets === undefined ? undefined : resets * 1000 }];
  });
  if (windows.length === 0) return undefined;
  return {
    source: "usage-cli",
    account: asString(account.name),
    plan: asString(account.plan),
    windows,
  };
}

/** Compact status value: dual bar (top = 5h, bottom = weekly) + weekly reset. */
export function formatStatusValue(usage: ClaudeUsage, now = Date.now()): string {
  const fiveHour = usage.windows.find((window) => window.label === "5h");
  const weekly = usage.windows.find((window) => window.label === "Weekly");
  const top = filledTwentieths(fiveHour ?? weekly);
  const bottom = filledTwentieths(weekly ?? fiveHour);
  let bar = "";
  for (let index = 0; index < BAR_WIDTH; index++) {
    const left = index * 2 + 1;
    let mask = 0;
    if (top >= left) mask |= 1;
    if (top >= left + 1) mask |= 2;
    if (bottom >= left) mask |= 4;
    if (bottom >= left + 1) mask |= 8;
    bar += DUAL_BAR_CHARS[mask];
  }
  const resets: string[] = [];
  if (fiveHour && remaining(fiveHour) <= 0 && fiveHour.resetAt) {
    resets.push(formatCountdown(fiveHour.resetAt - now));
  }
  const weeklyReset = (weekly ?? fiveHour)?.resetAt;
  if (weeklyReset) resets.push(formatCountdown(weeklyReset - now));
  return resets.length > 0 ? `${bar} ${resets.join("/")}` : bar;
}

export function isExhausted(usage: ClaudeUsage): boolean {
  return usage.windows.some((window) => remaining(window) <= 0);
}

export function formatCountdown(ms: number): string {
  const left = Math.max(0, ms);
  if (left > DAY_MS) return `${trimTenths(Math.ceil(left / (DAY_MS / 10)) / 10)}d`;
  if (left >= HOUR_MS) return `${trimTenths(Math.ceil(left / (HOUR_MS / 10)) / 10)}h`;
  if (left >= MINUTE_MS) return `${Math.floor(left / MINUTE_MS)}m`;
  return `${Math.floor(left / 1000)}s`;
}

/** Multi-line report for `/claude-usage`. */
export function formatReport(usage: ClaudeUsage, now = Date.now()): string {
  const header = [
    "Claude",
    usage.account ? `(${usage.account}${usage.plan ? `, ${usage.plan}` : ""})` : usage.plan ? `(${usage.plan})` : "",
    `via ${usage.source}`,
  ].filter(Boolean).join(" ");
  const width = Math.max(...usage.windows.map((window) => window.label.length));
  const rows = usage.windows.map((window) => {
    const left = Math.round(remaining(window));
    const filled = Math.round(left / 5);
    const bar = "█".repeat(filled) + "░".repeat(20 - filled);
    const reset = window.resetAt ? `, resets in ${formatCountdown(window.resetAt - now)}` : "";
    return `${window.label.padEnd(width)} ${bar} ${String(left).padStart(3)}% left${reset}`;
  });
  return [header, ...rows].join("\n");
}

async function queryUsageCli(): Promise<ClaudeUsage | undefined> {
  const preferred = process.env.PI_CLAUDE_USAGE_ACCOUNT?.trim() || undefined;
  const args = ["--json", ...(preferred ? ["--account", preferred] : [])];
  const stdout = await new Promise<string | undefined>((resolve) => {
    let out = "";
    let child: ReturnType<typeof spawn>;
    try {
      child = spawn("usage", args, { stdio: ["ignore", "pipe", "ignore"] });
    } catch {
      resolve(undefined);
      return;
    }
    const timer = setTimeout(() => child.kill("SIGTERM"), FETCH_TIMEOUT_MS);
    child.stdout?.setEncoding("utf8");
    child.stdout?.on("data", (chunk: string) => (out += chunk));
    child.once("error", () => {
      clearTimeout(timer);
      resolve(undefined);
    });
    child.once("close", (code) => {
      clearTimeout(timer);
      resolve(code === 0 ? out : undefined);
    });
  });
  if (!stdout) return undefined;
  try {
    return parseUsageCliReport(JSON.parse(stdout), preferred);
  } catch {
    return undefined;
  }
}

async function queryClaudeCodeCredentials(): Promise<ClaudeUsage | undefined> {
  const configDir = process.env.CLAUDE_CONFIG_DIR || join(homedir(), ".claude");
  let raw: string;
  try {
    raw = await readFile(join(configDir, ".credentials.json"), "utf8");
  } catch {
    return undefined;
  }
  const parsed: unknown = JSON.parse(raw);
  const oauth = isRecord(parsed) && isRecord(parsed.claudeAiOauth) ? parsed.claudeAiOauth : undefined;
  const token = asString(oauth?.accessToken);
  if (!token) return undefined;
  const response = await fetch(USAGE_URL, {
    headers: {
      Authorization: `Bearer ${token}`,
      Accept: "application/json",
      "anthropic-beta": "oauth-2025-04-20",
      "User-Agent": "pi-claude-usage",
    },
    signal: AbortSignal.timeout(FETCH_TIMEOUT_MS),
  });
  if (!response.ok) throw new Error(`Claude usage endpoint returned ${response.status}`);
  const windows = parseOAuthUsage(await response.json());
  if (windows.length === 0) return undefined;
  const tier = asString(oauth?.rateLimitTier) ?? "";
  const plan = asString(oauth?.subscriptionType);
  return {
    source: "claude-code",
    plan: plan && /20x/.test(tier) ? `${plan} 20x` : plan && /5x/.test(tier) ? `${plan} 5x` : plan,
    windows,
  };
}

export async function queryClaudeUsage(): Promise<ClaudeUsage | undefined> {
  return (await queryUsageCli()) ?? (await queryClaudeCodeCredentials());
}

export default function claudeUsage(pi: ExtensionAPI) {
  let cache: { at: number; usage: ClaudeUsage } | undefined;
  let failures = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let inFlight: Promise<void> | undefined;
  let activeModel: Model | undefined;

  const clear = (ctx: ExtensionContext) => {
    if (timer) clearTimeout(timer);
    timer = undefined;
    try {
      ctx.ui.setStatus(STATUS_KEY, undefined);
    } catch {
      // stale context
    }
  };

  const render = (ctx: ExtensionContext) => {
    const theme = ctx.ui.theme;
    const label = theme.fg("accent", LABEL);
    if (!cache) {
      ctx.ui.setStatus(STATUS_KEY, `${label} ${theme.fg(failures > 0 ? "muted" : "dim", failures > 0 ? "n/a" : "…")}`);
      return;
    }
    const value = formatStatusValue(cache.usage);
    const [bar, countdown] = value.split(" ", 2);
    const background = isExhausted(cache.usage) ? "toolErrorBg" : "userMessageBg";
    const barText = theme.bg(background, theme.fg("dim", bar ?? ""));
    ctx.ui.setStatus(STATUS_KEY, countdown ? `${label} ${barText} ${theme.fg("dim", countdown)}` : `${label} ${barText}`);
  };

  const schedule = (ctx: ExtensionContext) => {
    if (timer) clearTimeout(timer);
    timer = setTimeout(() => void refresh(ctx, true), REFRESH_INTERVAL_MS);
    (timer as { unref?: () => void }).unref?.();
  };

  const refresh = async (ctx: ExtensionContext, force: boolean): Promise<void> => {
    if (!isClaudeModel(activeModel)) return clear(ctx);
    if (!force && cache && Date.now() - cache.at < MIN_REFETCH_MS) {
      render(ctx);
      return;
    }
    if (inFlight) return inFlight;
    inFlight = (async () => {
      try {
        render(ctx);
        const usage = await queryClaudeUsage().catch(() => undefined);
        if (!isClaudeModel(activeModel)) return clear(ctx);
        if (usage) {
          cache = { at: Date.now(), usage };
          failures = 0;
        } else {
          failures += 1;
          if (failures >= 3) cache = undefined;
        }
        render(ctx);
        schedule(ctx);
      } catch {
        // stale context after reload/shutdown
      } finally {
        inFlight = undefined;
      }
    })();
    return inFlight;
  };

  pi.registerCommand("claude-usage", {
    description: "Show remaining Claude subscription quota",
    handler: async (_args, ctx) => {
      const usage = await queryClaudeUsage().catch(() => undefined);
      if (!usage) {
        ctx.ui.notify(
          "Claude usage unavailable. Run `usage login --provider claude --name max`, or sign in to Claude Code.",
          "warning",
        );
        return;
      }
      cache = { at: Date.now(), usage };
      if (isClaudeModel(ctx.model)) render(ctx);
      ctx.ui.notify(formatReport(usage), "info");
    },
  });

  pi.on("session_start", (_event, ctx) => {
    activeModel = ctx.model;
    void refresh(ctx, false);
  });
  pi.on("model_select", (event, ctx) => {
    activeModel = event.model;
    void refresh(ctx, false);
  });
  pi.on("agent_end", (_event, ctx) => {
    activeModel = ctx.model;
    void refresh(ctx, false);
  });
  pi.on("session_shutdown", (_event, ctx) => clear(ctx));
}

function filledTwentieths(window: ClaudeWindow | undefined): number {
  if (!window) return 0;
  const left = remaining(window);
  if (left <= 0) return 0;
  return Math.max(1, Math.round(left / 5));
}

function remaining(window: ClaudeWindow): number {
  return 100 - clamp(window.usedPercent);
}

function clamp(value: number): number {
  return Math.min(100, Math.max(0, value));
}

function trimTenths(value: number): string {
  return Number.isInteger(value) ? String(value) : value.toFixed(1);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function asString(value: unknown): string | undefined {
  return typeof value === "string" && value.trim() ? value.trim() : undefined;
}

function asNumber(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

function asTime(value: unknown): number | undefined {
  if (typeof value !== "string") return undefined;
  const ms = Date.parse(value);
  return Number.isFinite(ms) ? ms : undefined;
}
