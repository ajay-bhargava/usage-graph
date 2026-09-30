export type QuotaWindow = {
  bucket: string;
  label: string;
  used_percent: number;
  left_percent: number;
  resets_at?: number | null;
  reset_in?: string | null;
};

export type QuotaAccount = {
  name: string;
  provider: string;
  plan?: string | null;
  windows: QuotaWindow[];
  error?: string | null;
};

export type UsageReport = {
  accounts: QuotaAccount[];
};

export function isCodexAccount(account: QuotaAccount): boolean {
  return account.provider === "codex";
}

export function weeklyCodexWindow(
  account: QuotaAccount,
): QuotaWindow | undefined {
  const windows = account.windows.filter((window) => window.bucket === "codex");
  return (
    windows.find((window) => window.label.toLowerCase() === "weekly") ??
    windows.find((window) => window.label.toLowerCase().includes("weekly")) ??
    windows[0]
  );
}

export function weeklyLeft(account: QuotaAccount): number | undefined {
  return weeklyCodexWindow(account)?.left_percent;
}

export function isUnauthorized(account: QuotaAccount): boolean {
  return (account.error ?? "").toLowerCase().includes("unauthorized");
}

export function isHealthyCodex(account: QuotaAccount): boolean {
  if (!isCodexAccount(account) || account.error) return false;
  const left = weeklyLeft(account);
  return left !== undefined && left > 0;
}

export function findAccount(
  accounts: readonly QuotaAccount[],
  name: string,
): QuotaAccount | undefined {
  return accounts.find((account) => account.name === name);
}

export function pickAutoAccount(
  accounts: readonly QuotaAccount[],
  last?: string,
): QuotaAccount | undefined {
  const healthy = accounts.filter(isHealthyCodex);
  if (healthy.length === 0) return undefined;
  healthy.sort((left, right) => {
    const leftRemaining = weeklyLeft(left) ?? -1;
    const rightRemaining = weeklyLeft(right) ?? -1;
    if (rightRemaining !== leftRemaining) return rightRemaining - leftRemaining;
    if (left.name === last) return -1;
    if (right.name === last) return 1;
    return left.name.localeCompare(right.name);
  });
  return healthy[0];
}

export function unauthorizedCodexAccounts(
  accounts: readonly QuotaAccount[],
): QuotaAccount[] {
  return accounts.filter(
    (account) => isCodexAccount(account) && isUnauthorized(account),
  );
}

export function formatAccountStatus(account: QuotaAccount): string {
  if (account.error) return account.error;
  const window = weeklyCodexWindow(account);
  if (!window) return "no weekly window";
  const remaining = `${Math.round(window.left_percent)}% left`;
  return window.reset_in ? `${remaining}  ·  ${window.reset_in}` : remaining;
}

export function statusChip(name: string | undefined, mode: "auto" | "pinned"): string {
  if (!name) return `codex · ${mode}`;
  return `${name} · ${mode}`;
}
