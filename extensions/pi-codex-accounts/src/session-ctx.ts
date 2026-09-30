import type { ExtensionContext } from "@earendil-works/pi-coding-agent";

const STALE_CTX_MESSAGE = "extension ctx is stale after session replacement";

export function isStaleContextError(error: unknown): boolean {
  return error instanceof Error && error.message.includes(STALE_CTX_MESSAGE);
}

export function ctxIsLive(ctx: ExtensionContext): boolean {
  try {
    void ctx.mode;
    return true;
  } catch (error) {
    if (isStaleContextError(error)) return false;
    throw error;
  }
}

export function shouldStopSessionWork(
  ctx: ExtensionContext,
  signal?: AbortSignal,
): boolean {
  return Boolean(signal?.aborted) || !ctxIsLive(ctx);
}

export function notifyLive(
  ctx: ExtensionContext,
  message: string,
  level: "info" | "warning" | "error",
): void {
  if (!ctxIsLive(ctx)) return;
  try {
    ctx.ui.notify(message, level);
  } catch (error) {
    if (!isStaleContextError(error)) throw error;
  }
}

export function setStatusLive(
  ctx: ExtensionContext,
  key: string,
  text: string | undefined,
): void {
  if (!ctxIsLive(ctx)) return;
  try {
    ctx.ui.setStatus(key, text);
  } catch (error) {
    if (!isStaleContextError(error)) throw error;
  }
}
