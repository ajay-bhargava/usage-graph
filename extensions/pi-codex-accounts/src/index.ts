import { join } from "node:path";
import {
  getAgentDir,
  type ExtensionAPI,
  type ExtensionCommandContext,
  type ExtensionContext,
} from "@earendil-works/pi-coding-agent";
import type { AutocompleteItem } from "@earendil-works/pi-tui";
import {
  CODEX_PROVIDER_ID,
  matchingStoreAccountName,
  piAuthToStoredAccount,
  readPiCodexAuth,
  storedAccountToPiAuth,
  writePiCodexAuth,
} from "./auth.ts";
import { AUTO_VALUE, reauthWithDialog, selectCodexAccount } from "./dialogs.ts";
import {
  findAccount,
  formatAccountStatus,
  isCodexAccount,
  isUnauthorized,
  pickAutoAccount,
  statusChip,
  unauthorizedCodexAccounts,
  weeklyLeft,
  type QuotaAccount,
} from "./picker.ts";
import {
  loadPinState,
  pinAccount,
  rememberLast,
  savePinState,
  useAuto,
  type PinState,
} from "./pin-state.ts";
import {
  ctxIsLive,
  isStaleContextError,
  notifyLive,
  setStatusLive,
  shouldStopSessionWork,
} from "./session-ctx.ts";
import {
  defaultUsageStorePath,
  fetchUsageReport,
  findStoredAccount,
  loadAccountStore,
  saveAccountStore,
  upsertStoredAccount,
  type StoredAccount,
} from "./usage.ts";

const STATUS_KEY = "aa-codex-account";
const USAGE_TIMEOUT_MS = 20_000;

function pinPath(): string {
  return join(getAgentDir(), "codex-accounts.json");
}

function canDialog(ctx: ExtensionContext): boolean {
  return ctxIsLive(ctx) && ctx.hasUI && ctx.mode === "tui";
}

function isOpenAICodexModel(
  model: ExtensionContext["model"] | undefined,
): boolean {
  return model?.provider === CODEX_PROVIDER_ID;
}

function loadState(): PinState {
  return loadPinState(pinPath());
}

function saveState(state: PinState): void {
  savePinState(pinPath(), state);
}

function setAccountStatus(
  ctx: ExtensionContext,
  name: string | undefined,
  mode: PinState["mode"],
): void {
  setStatusLive(ctx, STATUS_KEY, statusChip(name, mode));
}

function copybackActiveAccount(state: PinState): PinState {
  const store = loadAccountStore();
  const piAuth = readPiCodexAuth();
  const name =
    matchingStoreAccountName(store.accounts, piAuth) ??
    state.last ??
    state.pinned;
  if (!name || !piAuth) return state;
  const nextStore = upsertStoredAccount(
    store,
    piAuthToStoredAccount(name, piAuth),
  );
  saveAccountStore(nextStore, defaultUsageStorePath());
  return rememberLast(state, name);
}

async function swapToAccount(
  ctx: ExtensionContext,
  name: string,
  state: PinState,
  options: {
    notify: boolean;
    previousAccess?: string | undefined;
    signal?: AbortSignal;
  },
): Promise<PinState> {
  const store = loadAccountStore();
  const account = findStoredAccount(store, name);
  if (!account) {
    notifyLive(ctx, `usage-cli has no account named ${name}`, "error");
    return state;
  }
  const credential = storedAccountToPiAuth(account);
  if (credential.access !== options.previousAccess) {
    writePiCodexAuth(credential);
  }
  const next = rememberLast(state, name);
  saveState(next);
  setAccountStatus(ctx, name, next.mode);
  if (options.notify) {
    const report = await fetchUsageReport({
      timeoutMs: USAGE_TIMEOUT_MS,
      ...(options.signal ? { signal: options.signal } : {}),
    }).catch(() => undefined);
    if (shouldStopSessionWork(ctx, options.signal)) return next;
    const quota = report ? findAccount(report.accounts, name) : undefined;
    const detail = quota ? formatAccountStatus(quota) : name;
    const mode = next.mode === "pinned" ? "pinned" : "auto";
    notifyLive(ctx, `codex → ${name} (${mode}, ${detail})`, "info");
  }
  return next;
}

async function applyCodexAccount(
  ctx: ExtensionContext,
  options: {
    notifySwitch: boolean;
    allowDialog: boolean;
    signal?: AbortSignal;
  },
): Promise<void> {
  if (shouldStopSessionWork(ctx, options.signal)) return;
  if (!isOpenAICodexModel(ctx.model)) {
    setStatusLive(ctx, STATUS_KEY, undefined);
    return;
  }

  let state = copybackActiveAccount(loadState());
  saveState(state);
  const previousAccess = readPiCodexAuth()?.access;
  let report;
  try {
    report = await fetchUsageReport({
      timeoutMs: USAGE_TIMEOUT_MS,
      ...(options.signal ? { signal: options.signal } : {}),
    });
  } catch (error) {
    if (shouldStopSessionWork(ctx, options.signal)) return;
    throw error;
  }
  if (shouldStopSessionWork(ctx, options.signal)) return;
  const codexAccounts = report.accounts.filter(isCodexAccount);

  if (state.mode === "pinned" && state.pinned) {
    const account = findAccount(codexAccounts, state.pinned);
    if (!account) {
      notifyLive(
        ctx,
        `pinned Codex account ${state.pinned} is missing`,
        "error",
      );
      setAccountStatus(ctx, state.pinned, "pinned");
      return;
    }
    if (account.error) {
      if (options.allowDialog && canDialog(ctx) && isUnauthorized(account)) {
        const ok = await reauthWithDialog(ctx, state.pinned);
        if (shouldStopSessionWork(ctx, options.signal)) return;
        if (!ok) {
          notifyLive(ctx, `${state.pinned} needs reauth`, "warning");
          setAccountStatus(ctx, state.pinned, "pinned");
          return;
        }
      } else {
        notifyLive(
          ctx,
          `${state.pinned} needs reauth. Run /codex-account reauth ${state.pinned}`,
          "warning",
        );
        setAccountStatus(ctx, state.pinned, "pinned");
        return;
      }
    } else if ((weeklyLeft(account) ?? 0) <= 0) {
      notifyLive(
        ctx,
        `${state.pinned} weekly window is empty · keeping pin`,
        "warning",
      );
    }
    await swapToAccount(ctx, state.pinned, state, {
      notify: options.notifySwitch,
      previousAccess,
      ...(options.signal ? { signal: options.signal } : {}),
    });
    return;
  }

  const picked = pickAutoAccount(codexAccounts, state.last);
  if (picked) {
    const stored = findStoredAccount(loadAccountStore(), picked.name);
    const switched = stored?.access !== previousAccess;
    await swapToAccount(ctx, picked.name, state, {
      notify: options.notifySwitch && switched,
      previousAccess,
      ...(options.signal ? { signal: options.signal } : {}),
    });
    return;
  }

  const unauthorized = unauthorizedCodexAccounts(codexAccounts);
  if (
    unauthorized.length > 0 &&
    unauthorized.length === codexAccounts.length &&
    options.allowDialog &&
    canDialog(ctx) &&
    ctxIsLive(ctx)
  ) {
    const choice = await selectCodexAccount(
      ctx,
      unauthorized,
      state.last,
      "auto",
    );
    if (shouldStopSessionWork(ctx, options.signal)) return;
    if (choice && choice !== AUTO_VALUE) {
      const ok = await reauthWithDialog(ctx, choice);
      if (shouldStopSessionWork(ctx, options.signal)) return;
      if (ok) {
        await swapToAccount(ctx, choice, useAuto(state), {
          notify: true,
          previousAccess,
          ...(options.signal ? { signal: options.signal } : {}),
        });
      }
    }
    return;
  }

  setAccountStatus(ctx, state.last, "auto");
  notifyLive(
    ctx,
    "all Codex weekly windows empty · keeping current account",
    "warning",
  );
}

function accountNames(): string[] {
  return loadAccountStore()
    .accounts.filter((account) => account.provider === "codex")
    .map((account) => account.name);
}

async function handleCommand(
  args: string,
  ctx: ExtensionCommandContext,
): Promise<void> {
  const tokens = args.trim().split(/\s+/).filter(Boolean);
  const command = tokens[0] ?? "";

  if (command === "status") {
    const state = loadState();
    const report = await fetchUsageReport({ timeoutMs: USAGE_TIMEOUT_MS });
    const name = state.mode === "pinned" ? state.pinned : state.last;
    const account = name ? findAccount(report.accounts, name) : undefined;
    const detail = account ? formatAccountStatus(account) : "no quota yet";
    notifyLive(ctx, `${name ?? "none"} · ${state.mode} · ${detail}`, "info");
    return;
  }

  if (command === "auto") {
    const state = useAuto(loadState());
    saveState(state);
    await applyCodexAccount(ctx, { notifySwitch: true, allowDialog: true });
    return;
  }

  if (command === "reauth") {
    const name = tokens[1] ?? (await pickNameForReauth(ctx));
    if (!name) return;
    const ok = await reauthWithDialog(ctx, name);
    if (!ok) {
      notifyLive(ctx, "reauth cancelled", "warning");
      return;
    }
    const next = pinAccount(loadState(), name);
    saveState(next);
    await swapToAccount(ctx, name, next, { notify: true });
    return;
  }

  if (command) {
    await pinAndSwap(ctx, command);
    return;
  }

  const report = await fetchUsageReport({ timeoutMs: USAGE_TIMEOUT_MS });
  const state = loadState();
  const choice = await selectCodexAccount(
    ctx,
    report.accounts.filter(isCodexAccount),
    state.mode === "pinned" ? state.pinned : state.last,
    state.mode,
  );
  if (!choice) return;
  if (choice === AUTO_VALUE) {
    saveState(useAuto(state));
    await applyCodexAccount(ctx, { notifySwitch: true, allowDialog: true });
    return;
  }
  const selected = findAccount(report.accounts, choice);
  if (selected?.error && isUnauthorized(selected)) {
    const ok = await reauthWithDialog(ctx, choice);
    if (!ok) {
      notifyLive(ctx, "reauth cancelled", "warning");
      return;
    }
  }
  await pinAndSwap(ctx, choice);
}

async function pickNameForReauth(
  ctx: ExtensionCommandContext,
): Promise<string | undefined> {
  const names = accountNames();
  if (names.length === 0) {
    notifyLive(ctx, "no Codex accounts in usage-cli", "error");
    return undefined;
  }
  if (names.length === 1) return names[0];
  return ctx.ui.select("Reauth which account?", names);
}

async function pinAndSwap(
  ctx: ExtensionCommandContext,
  name: string,
): Promise<void> {
  const next = pinAccount(loadState(), name);
  saveState(next);
  await swapToAccount(ctx, name, next, { notify: true });
}

function commandCompletions(prefix: string): AutocompleteItem[] {
  const values = ["auto", "status", "reauth", ...accountNames()];
  return values
    .filter((value) => value.startsWith(prefix))
    .map((value) => ({ value, label: value }));
}

export default function codexAccounts(pi: ExtensionAPI): void {
  let sessionWork: AbortController | undefined;

  function startSessionWork(): AbortSignal {
    sessionWork?.abort();
    sessionWork = new AbortController();
    return sessionWork.signal;
  }

  function currentSessionSignal(): AbortSignal {
    if (!sessionWork) sessionWork = new AbortController();
    return sessionWork.signal;
  }

  function runAccountWork(
    ctx: ExtensionContext,
    signal: AbortSignal,
    notifyLevel: "error" | "warning",
  ): Promise<void> {
    return applyCodexAccount(ctx, {
      notifySwitch: false,
      allowDialog: notifyLevel === "warning",
      signal,
    }).catch((error: unknown) => {
      if (isStaleContextError(error) || signal.aborted) return;
      notifyLive(
        ctx,
        error instanceof Error ? error.message : String(error),
        notifyLevel,
      );
    });
  }

  pi.registerCommand("codex-account", {
    description: "Pin, auto-rotate, or reauth Codex subscription accounts",
    getArgumentCompletions: (prefix: string) => commandCompletions(prefix),
    handler: async (args, ctx) => {
      try {
        await handleCommand(args, ctx);
      } catch (error) {
        if (isStaleContextError(error)) return;
        notifyLive(
          ctx,
          error instanceof Error ? error.message : String(error),
          "error",
        );
      }
    },
  });

  pi.on("session_start", (_event, ctx) => {
    const signal = startSessionWork();
    if (!isOpenAICodexModel(ctx.model)) {
      setStatusLive(ctx, STATUS_KEY, undefined);
      return;
    }
    void runAccountWork(ctx, signal, "error");
  });

  pi.on("model_select", (event, ctx) => {
    const signal = startSessionWork();
    if (!isOpenAICodexModel(event.model)) {
      setStatusLive(ctx, STATUS_KEY, undefined);
      return;
    }
    void runAccountWork(ctx, signal, "error");
  });

  pi.on("before_agent_start", (_event, ctx) => {
    if (!isOpenAICodexModel(ctx.model)) return;
    return runAccountWork(ctx, currentSessionSignal(), "warning");
  });

  pi.on("session_shutdown", () => {
    sessionWork?.abort();
    sessionWork = undefined;
    saveState(copybackActiveAccount(loadState()));
  });
}

export { findAccount, pickAutoAccount, pinAccount, statusChip, weeklyLeft };
export type { QuotaAccount, StoredAccount };
