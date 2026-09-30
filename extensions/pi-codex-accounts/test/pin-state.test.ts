import { describe, expect, test } from "vitest";
import {
  parsePinState,
  pinAccount,
  rememberLast,
  useAuto,
} from "../src/pin-state.ts";

describe("pin state", () => {
  test("parses auto and pinned documents", () => {
    expect(parsePinState(`{"mode":"auto","last":"amp"}`)).toEqual({
      mode: "auto",
      last: "amp",
    });
    expect(
      parsePinState(`{"mode":"pinned","pinned":"consulting","last":"consulting"}`),
    ).toEqual({
      mode: "pinned",
      pinned: "consulting",
      last: "consulting",
    });
  });

  test("pins, unpins, and remembers the last account", () => {
    const pinned = pinAccount({ mode: "auto", last: "amp" }, "consulting");
    expect(pinned).toEqual({
      mode: "pinned",
      pinned: "consulting",
      last: "consulting",
    });
    expect(useAuto(pinned)).toEqual({
      mode: "auto",
      last: "consulting",
    });
    expect(rememberLast({ mode: "auto" }, "amp")).toEqual({
      mode: "auto",
      last: "amp",
    });
  });
});
