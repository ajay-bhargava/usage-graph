import { chmodSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { getAgentDir, readStoredCredential } from "@earendil-works/pi-coding-agent";
import type { StoredAccount } from "./usage.ts";

export const CODEX_PROVIDER_ID = "openai-codex";

export type PiCodexOAuth = {
  type: "oauth";
  access: string;
  refresh: string;
  expires: number;
  accountId?: string;
};

export function defaultPiAuthPath(): string {
  return join(getAgentDir(), "auth.json");
}

export function storedAccountToPiAuth(account: StoredAccount): PiCodexOAuth {
  const credential: PiCodexOAuth = {
    type: "oauth",
    access: account.access,
    refresh: account.refresh ?? "",
    expires: account.expires ?? 0,
  };
  if (account.account_id) credential.accountId = account.account_id;
  return credential;
}

export function piAuthToStoredAccount(
  name: string,
  credential: PiCodexOAuth,
): StoredAccount {
  const account: StoredAccount = {
    name,
    provider: "codex",
    access: credential.access,
  };
  if (credential.refresh) account.refresh = credential.refresh;
  if (Number.isFinite(credential.expires) && credential.expires > 0) {
    account.expires = credential.expires;
  }
  if (credential.accountId) account.account_id = credential.accountId;
  return account;
}

export function readPiCodexAuth(
  authPath = defaultPiAuthPath(),
): PiCodexOAuth | undefined {
  const credential = readStoredCredential(CODEX_PROVIDER_ID, authPath);
  if (!credential || credential.type !== "oauth") return undefined;
  if (
    typeof credential.access !== "string" ||
    typeof credential.refresh !== "string" ||
    typeof credential.expires !== "number"
  ) {
    return undefined;
  }
  const oauth: PiCodexOAuth = {
    type: "oauth",
    access: credential.access,
    refresh: credential.refresh,
    expires: credential.expires,
  };
  if (typeof credential.accountId === "string" && credential.accountId) {
    oauth.accountId = credential.accountId;
  }
  return oauth;
}

export function writePiCodexAuth(
  credential: PiCodexOAuth,
  authPath = defaultPiAuthPath(),
): void {
  mkdirSync(dirname(authPath), { recursive: true, mode: 0o700 });
  let data: Record<string, unknown> = {};
  try {
    const parsed: unknown = JSON.parse(readFileSync(authPath, "utf8"));
    if (typeof parsed === "object" && parsed !== null && !Array.isArray(parsed)) {
      data = parsed as Record<string, unknown>;
    }
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
  }
  data[CODEX_PROVIDER_ID] = credential;
  writeFileSync(authPath, `${JSON.stringify(data, null, 2)}\n`, {
    encoding: "utf8",
    mode: 0o600,
  });
  chmodSync(authPath, 0o600);
}

export function matchingStoreAccountName(
  accounts: readonly StoredAccount[],
  piAuth: PiCodexOAuth | undefined,
): string | undefined {
  if (!piAuth) return undefined;
  return accounts.find((account) => account.access === piAuth.access)?.name;
}
