import { describe, expect, test } from "vitest";
import {
  formatAccountStatus,
  isHealthyCodex,
  isUnauthorized,
  pickAutoAccount,
  statusChip,
  type QuotaAccount,
} from "../src/picker.ts";

function account(
  name: string,
  left: number,
  extra?: Partial<QuotaAccount>,
): QuotaAccount {
  return {
    name,
    provider: "codex",
    windows: [
      {
        bucket: "codex",
        label: "Weekly",
        used_percent: 100 - left,
        left_percent: left,
        reset_in: "2d",
      },
    ],
    ...extra,
  };
}

describe("pickAutoAccount", () => {
  test("picks the highest remaining weekly window", () => {
    const picked = pickAutoAccount([
      account("work", 0),
      account("consulting", 18),
      account("amp", 98),
      account("personal", 0),
    ]);
    expect(picked?.name).toBe("amp");
  });

  test("skips unauthorized and non-codex accounts", () => {
    const picked = pickAutoAccount([
      account("amp", 98, { error: "unauthorized" }),
      account("consulting", 18),
      {
        name: "xai-work",
        provider: "xai",
        windows: [
          {
            bucket: "xai",
            label: "Weekly",
            used_percent: 10,
            left_percent: 90,
          },
        ],
      },
    ]);
    expect(picked?.name).toBe("consulting");
  });

  test("keeps the last account on a remaining-percent tie", () => {
    const picked = pickAutoAccount(
      [account("amp", 40), account("consulting", 40)],
      "consulting",
    );
    expect(picked?.name).toBe("consulting");
  });

  test("returns undefined when every Codex window is empty", () => {
    expect(
      pickAutoAccount([account("work", 0), account("personal", 0)]),
    ).toBeUndefined();
  });
});

describe("account status", () => {
  test("formats remaining percent and reset", () => {
    expect(formatAccountStatus(account("amp", 98))).toBe("98% left  ·  2d");
  });

  test("flags unauthorized accounts", () => {
    const dead = account("personal", 0, { error: "unauthorized" });
    expect(isUnauthorized(dead)).toBe(true);
    expect(isHealthyCodex(dead)).toBe(false);
    expect(formatAccountStatus(dead)).toBe("unauthorized");
  });

  test("renders the status chip", () => {
    expect(statusChip("amp", "auto")).toBe("amp · auto");
    expect(statusChip("consulting", "pinned")).toBe("consulting · pinned");
  });
});
