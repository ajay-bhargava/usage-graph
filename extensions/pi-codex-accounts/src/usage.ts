import { spawn } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import { createInterface } from "node:readline";
import type { UsageReport } from "./picker.ts";

export type StoredAccount = {
  name: string;
  provider: string;
  account_id?: string;
  access: string;
  refresh?: string;
  expires?: number;
};

export type AccountStore = {
  accounts: StoredAccount[];
};

export type DeviceCodeEvent = {
  event: "device_code";
  verification_uri: string;
  user_code: string;
};

export type CompleteEvent = {
  event: "complete";
  name: string;
  provider: string;
};

export type LoginEvent = DeviceCodeEvent | CompleteEvent;

export function defaultUsageStorePath(
  platform = process.platform,
  home = homedir(),
  env = process.env,
): string {
  // Match Rust dirs::config_dir(), including macOS ignoring XDG_CONFIG_HOME.
  const configDir = platform === "darwin"
    ? join(home, "Library", "Application Support")
    : platform === "win32"
      ? env.APPDATA ?? join(home, ".config")
      : env.XDG_CONFIG_HOME?.startsWith("/")
        ? env.XDG_CONFIG_HOME
        : join(home, ".config");
  return join(configDir, "usage-cli", "accounts.json");
}

export function loadAccountStore(path = defaultUsageStorePath()): AccountStore {
  try {
    const parsed: unknown = JSON.parse(readFileSync(path, "utf8"));
    if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
      throw new Error("usage account store is not an object");
    }
    const accounts = (parsed as { accounts?: unknown }).accounts;
    if (!Array.isArray(accounts)) return { accounts: [] };
    return { accounts: accounts.filter(isStoredAccount) };
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") {
      return { accounts: [] };
    }
    throw error;
  }
}

export function saveAccountStore(
  store: AccountStore,
  path = defaultUsageStorePath(),
): void {
  mkdirSync(dirname(path), { recursive: true, mode: 0o700 });
  writeFileSync(path, `${JSON.stringify(store, null, 2)}\n`, {
    encoding: "utf8",
    mode: 0o600,
  });
}

export function upsertStoredAccount(
  store: AccountStore,
  account: StoredAccount,
): AccountStore {
  const accounts = store.accounts.filter((entry) => entry.name !== account.name);
  accounts.push(account);
  return { accounts };
}

export function findStoredAccount(
  store: AccountStore,
  name: string,
): StoredAccount | undefined {
  return store.accounts.find((account) => account.name === name);
}

export async function fetchUsageReport(options?: {
  timeoutMs?: number;
  signal?: AbortSignal;
}): Promise<UsageReport> {
  const { stdout } = await runUsage(["--json"], options);
  const parsed: unknown = JSON.parse(stdout);
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
    throw new Error("usage --json did not return an object");
  }
  const accounts = (parsed as { accounts?: unknown }).accounts;
  if (!Array.isArray(accounts)) return { accounts: [] };
  return { accounts: accounts as UsageReport["accounts"] };
}

export async function reauthAccount(
  name: string,
  onEvent: (event: LoginEvent) => void,
  options?: { signal?: AbortSignal },
): Promise<void> {
  await runUsageLines(["reauth", name, "--json"], onEvent, options);
}

export function parseLoginEvent(raw: string): LoginEvent | undefined {
  const trimmed = raw.trim();
  if (!trimmed) return undefined;
  const parsed: unknown = JSON.parse(trimmed);
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
    return undefined;
  }
  const record = parsed as Record<string, unknown>;
  if (
    record.event === "device_code" &&
    typeof record.verification_uri === "string" &&
    typeof record.user_code === "string"
  ) {
    return {
      event: "device_code",
      verification_uri: record.verification_uri,
      user_code: record.user_code,
    };
  }
  if (
    record.event === "complete" &&
    typeof record.name === "string" &&
    typeof record.provider === "string"
  ) {
    return {
      event: "complete",
      name: record.name,
      provider: record.provider,
    };
  }
  return undefined;
}

function isStoredAccount(value: unknown): value is StoredAccount {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return false;
  }
  const record = value as Record<string, unknown>;
  return typeof record.name === "string" && typeof record.access === "string";
}

function runUsage(
  args: string[],
  options?: { timeoutMs?: number; signal?: AbortSignal },
): Promise<{ stdout: string; stderr: string }> {
  return new Promise((resolve, reject) => {
    const child = spawn("usage", args, { stdio: ["ignore", "pipe", "pipe"] });
    let stdout = "";
    let stderr = "";
    const timeout = options?.timeoutMs
      ? setTimeout(() => {
          child.kill("SIGTERM");
          reject(new Error(`usage ${args.join(" ")} timed out`));
        }, options.timeoutMs)
      : undefined;
    const onAbort = () => child.kill("SIGTERM");
    options?.signal?.addEventListener("abort", onAbort, { once: true });
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk: string) => {
      stdout += chunk;
    });
    child.stderr.on("data", (chunk: string) => {
      stderr += chunk;
    });
    child.once("error", (error) => {
      if (timeout) clearTimeout(timeout);
      options?.signal?.removeEventListener("abort", onAbort);
      reject(error);
    });
    child.once("close", (code) => {
      if (timeout) clearTimeout(timeout);
      options?.signal?.removeEventListener("abort", onAbort);
      if (code === 0) {
        resolve({ stdout, stderr });
        return;
      }
      reject(
        new Error(
          `usage ${args.join(" ")} exited ${code ?? "unknown"}: ${stderr.trim() || stdout.trim()}`,
        ),
      );
    });
  });
}

function runUsageLines(
  args: string[],
  onEvent: (event: LoginEvent) => void,
  options?: { signal?: AbortSignal },
): Promise<void> {
  return new Promise((resolve, reject) => {
    const child = spawn("usage", args, { stdio: ["ignore", "pipe", "pipe"] });
    let stderr = "";
    const onAbort = () => child.kill("SIGTERM");
    options?.signal?.addEventListener("abort", onAbort, { once: true });
    child.stderr.setEncoding("utf8");
    child.stderr.on("data", (chunk: string) => {
      stderr += chunk;
    });
    const lines = createInterface({ input: child.stdout });
    lines.on("line", (line) => {
      try {
        const event = parseLoginEvent(line);
        if (event) onEvent(event);
      } catch {
        // Ignore non-JSON chatter on stdout.
      }
    });
    child.once("error", (error) => {
      options?.signal?.removeEventListener("abort", onAbort);
      reject(error);
    });
    child.once("close", (code) => {
      options?.signal?.removeEventListener("abort", onAbort);
      if (code === 0) {
        resolve();
        return;
      }
      reject(
        new Error(
          `usage ${args.join(" ")} exited ${code ?? "unknown"}: ${stderr.trim()}`,
        ),
      );
    });
  });
}
