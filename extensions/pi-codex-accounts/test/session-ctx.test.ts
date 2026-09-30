import { describe, expect, test } from "vitest";
import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import {
  ctxIsLive,
  isStaleContextError,
  notifyLive,
  setStatusLive,
  shouldStopSessionWork,
} from "../src/session-ctx.ts";

const STALE_MESSAGE =
  "This extension ctx is stale after session replacement or reload. Do not use a captured pi or command ctx after ctx.newSession(), ctx.fork(), ctx.switchSession(), or ctx.reload().";

function staleCtx(): ExtensionContext {
  return {
    get mode() {
      throw new Error(STALE_MESSAGE);
    },
    get ui() {
      throw new Error(STALE_MESSAGE);
    },
  } as unknown as ExtensionContext;
}

function liveCtx(calls: {
  notify: unknown[];
  setStatus: unknown[];
}): ExtensionContext {
  return {
    mode: "tui",
    ui: {
      notify(message: string, level: string) {
        calls.notify.push([message, level]);
      },
      setStatus(key: string, text: string | undefined) {
        calls.setStatus.push([key, text]);
      },
    },
  } as unknown as ExtensionContext;
}

describe("stale session ctx", () => {
  test("detects Pi's session-replacement error", () => {
    expect(isStaleContextError(new Error(STALE_MESSAGE))).toBe(true);
    expect(isStaleContextError(new Error("usage timed out"))).toBe(false);
    expect(isStaleContextError("nope")).toBe(false);
  });

  test("treats a replaced ctx as dead without throwing", () => {
    expect(ctxIsLive(staleCtx())).toBe(false);
    expect(shouldStopSessionWork(staleCtx())).toBe(true);
  });

  test("stops work when the session abort fires", () => {
    const calls = { notify: [], setStatus: [] };
    const ctx = liveCtx(calls);
    const sessionWork = new AbortController();
    sessionWork.abort();
    expect(shouldStopSessionWork(ctx, sessionWork.signal)).toBe(true);
  });

  test("swallows notify and status writes on a stale ctx", () => {
    const ctx = staleCtx();
    expect(() => notifyLive(ctx, "codex → amp", "error")).not.toThrow();
    expect(() => setStatusLive(ctx, "aa-codex-account", "amp")).not.toThrow();
  });

  test("forwards live notify and status writes", () => {
    const calls = { notify: [] as unknown[], setStatus: [] as unknown[] };
    const ctx = liveCtx(calls);
    notifyLive(ctx, "codex → amp", "info");
    setStatusLive(ctx, "aa-codex-account", "amp · auto");
    expect(calls.notify).toEqual([["codex → amp", "info"]]);
    expect(calls.setStatus).toEqual([["aa-codex-account", "amp · auto"]]);
  });
});
