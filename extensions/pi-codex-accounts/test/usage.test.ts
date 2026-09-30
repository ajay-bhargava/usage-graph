import { describe, expect, test } from "vitest";
import { defaultUsageStorePath, parseLoginEvent, upsertStoredAccount } from "../src/usage.ts";

describe("usage helpers", () => {
  test("matches the Rust CLI config directory on macOS and Linux", () => {
    expect(defaultUsageStorePath("darwin", "/Users/test", { XDG_CONFIG_HOME: "/ignored" }))
      .toBe("/Users/test/Library/Application Support/usage-cli/accounts.json");
    expect(defaultUsageStorePath("linux", "/home/test", {}))
      .toBe("/home/test/.config/usage-cli/accounts.json");
    expect(defaultUsageStorePath("linux", "/home/test", { XDG_CONFIG_HOME: "/custom" }))
      .toBe("/custom/usage-cli/accounts.json");
    expect(defaultUsageStorePath("linux", "/home/test", { XDG_CONFIG_HOME: "relative" }))
      .toBe("/home/test/.config/usage-cli/accounts.json");
  });
  test("parses device-code and complete events", () => {
    expect(
      parseLoginEvent(
        `{"event":"device_code","verification_uri":"https://auth.openai.com/codex/device","user_code":"ABCD-1234"}`,
      ),
    ).toEqual({
      event: "device_code",
      verification_uri: "https://auth.openai.com/codex/device",
      user_code: "ABCD-1234",
    });
    expect(
      parseLoginEvent(`{"event":"complete","name":"amp","provider":"codex"}`),
    ).toEqual({
      event: "complete",
      name: "amp",
      provider: "codex",
    });
    expect(parseLoginEvent("")).toBeUndefined();
  });

  test("upserts a named account in place", () => {
    const store = upsertStoredAccount(
      {
        accounts: [
          { name: "amp", provider: "codex", access: "old" },
          { name: "work", provider: "codex", access: "work" },
        ],
      },
      { name: "amp", provider: "codex", access: "new", refresh: "r" },
    );
    expect(store.accounts).toEqual([
      { name: "work", provider: "codex", access: "work" },
      { name: "amp", provider: "codex", access: "new", refresh: "r" },
    ]);
  });
});
