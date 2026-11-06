import { describe, expect, it } from "vitest";
import {
  formatCountdown,
  formatStatusValue,
  isClaudeModel,
  isExhausted,
  parseOAuthUsage,
  parseUsageCliReport,
} from "../index.ts";

const NOW = Date.parse("2026-11-02T01:00:00Z");

describe("parseOAuthUsage", () => {
  it("maps named buckets and scoped limits", () => {
    const windows = parseOAuthUsage({
      five_hour: { utilization: 25, resets_at: "2026-11-02T06:00:00Z" },
      seven_day: { utilization: 10, resets_at: "2026-11-08T17:00:00Z" },
      seven_day_opus: null,
      limits: [
        { kind: "session", percent: 25 },
        { kind: "weekly_scoped", percent: 0, scope: { model: { display_name: "Fable" } } },
      ],
    });
    expect(windows.map((w) => w.label)).toEqual(["5h", "Weekly", "Fable Weekly"]);
    expect(windows[0]?.resetAt).toBe(Date.parse("2026-11-02T06:00:00Z"));
  });

  it("degrades to empty on junk", () => {
    expect(parseOAuthUsage("nope")).toEqual([]);
  });
});

describe("parseUsageCliReport", () => {
  const report = {
    accounts: [
      { name: "work", provider: "codex", windows: [] },
      {
        name: "max",
        provider: "claude",
        plan: "max 20x",
        windows: [{ label: "5h", used_percent: 40, resets_at: 1_793_599_199 }],
        error: null,
      },
    ],
  };

  it("selects the claude account", () => {
    const usage = parseUsageCliReport(report);
    expect(usage?.account).toBe("max");
    expect(usage?.windows[0]?.resetAt).toBe(1_793_599_199_000);
  });

  it("returns undefined when the preferred account is missing", () => {
    expect(parseUsageCliReport(report, "other")).toBeUndefined();
  });
});

describe("formatStatusValue", () => {
  it("draws a dual bar with the weekly countdown", () => {
    const value = formatStatusValue(
      {
        source: "claude-code",
        windows: [
          { label: "5h", usedPercent: 50, resetAt: NOW + 3 * 3_600_000 },
          { label: "Weekly", usedPercent: 0, resetAt: NOW + 6.5 * 86_400_000 },
        ],
      },
      NOW,
    );
    expect(value).toBe("█████▄▄▄▄▄ 6.5d");
  });

  it("adds the 5h reset when exhausted", () => {
    const usage = {
      source: "claude-code" as const,
      windows: [
        { label: "5h", usedPercent: 100, resetAt: NOW + 2 * 3_600_000 },
        { label: "Weekly", usedPercent: 0, resetAt: NOW + 2 * 86_400_000 },
      ],
    };
    expect(formatStatusValue(usage, NOW)).toBe("▄▄▄▄▄▄▄▄▄▄ 2h/2d");
    expect(isExhausted(usage)).toBe(true);
  });
});

it("formats countdowns and detects providers", () => {
  expect(formatCountdown(90 * 60_000)).toBe("1.5h");
  expect(formatCountdown(30_000)).toBe("30s");
  expect(isClaudeModel({ provider: "claude-code" })).toBe(true);
  expect(isClaudeModel({ provider: "openai-codex" })).toBe(false);
});
