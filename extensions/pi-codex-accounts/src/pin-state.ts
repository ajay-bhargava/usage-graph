import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";

export type PinMode = "auto" | "pinned";

export type PinState = {
  mode: PinMode;
  last?: string;
  pinned?: string;
};

export function defaultPinState(): PinState {
  return { mode: "auto" };
}

export function parsePinState(raw: string): PinState {
  const parsed: unknown = JSON.parse(raw);
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
    throw new Error("codex-accounts state is not an object");
  }
  const record = parsed as Record<string, unknown>;
  const mode = record.mode === "pinned" ? "pinned" : "auto";
  const state: PinState = { mode };
  if (typeof record.last === "string" && record.last.length > 0) {
    state.last = record.last;
  }
  if (
    mode === "pinned" &&
    typeof record.pinned === "string" &&
    record.pinned.length > 0
  ) {
    state.pinned = record.pinned;
  } else if (mode === "pinned" && state.last) {
    state.pinned = state.last;
  }
  return state;
}

export function loadPinState(path: string): PinState {
  try {
    return parsePinState(readFileSync(path, "utf8"));
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") {
      return defaultPinState();
    }
    throw error;
  }
}

export function savePinState(path: string, state: PinState): void {
  mkdirSync(dirname(path), { recursive: true, mode: 0o700 });
  writeFileSync(path, `${JSON.stringify(state, null, 2)}\n`, {
    encoding: "utf8",
    mode: 0o600,
  });
}

export function pinAccount(state: PinState, name: string): PinState {
  return { mode: "pinned", pinned: name, last: name };
}

export function useAuto(state: PinState): PinState {
  const next: PinState = { mode: "auto" };
  if (state.last) next.last = state.last;
  return next;
}

export function rememberLast(state: PinState, name: string): PinState {
  if (state.mode === "pinned") {
    return { mode: "pinned", pinned: state.pinned ?? name, last: name };
  }
  return { mode: "auto", last: name };
}
