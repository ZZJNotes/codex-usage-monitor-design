import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";

import { TokenUsageSection } from "./TokenUsageSection";
import type { TokenUsageData } from "../types";

afterEach(cleanup);

const counts = {
  inputTokens: 1,
  cachedInputTokens: 0,
  cacheWriteInputTokens: 0,
  outputTokens: 1,
  reasoningOutputTokens: 0,
  totalTokens: 2,
};

function data(): TokenUsageData {
  return {
    totals: counts,
    models: [{ model: "gpt-5.6", counts }],
    accounts: [],
    sessions: Array.from({ length: 23 }, (_, index) => ({
      sessionId: `session-${String(index + 1).padStart(2, "0")}`,
      model: "gpt-5.6",
      firstObservedAt: new Date(Date.UTC(2026, 6, 1, 0, index)).toISOString(),
      lastObservedAt: new Date(Date.UTC(2026, 6, 1, 0, index)).toISOString(),
      counts,
      assignment: {
        account: null,
        source: "unassigned" as const,
        assignedAt: new Date(Date.UTC(2026, 6, 1, 0, index)).toISOString(),
        evidenceSource: null,
        evidenceObservedAt: null,
      },
    })),
    updatedAt: "2026-07-01T00:22:00.000Z",
  };
}

test("sorts sessions newest first and paginates ten rows by default", () => {
  render(<TokenUsageSection
    state={{ status: "ready", data: data() }}
    locale="zh-CN"
    formatLocale="zh-CN"
    onQuery={vi.fn()}
    onReassign={vi.fn()}
  />);

  const table = screen.getByRole("table", { name: "按会话统计" });
  const rows = within(table).getAllByRole("row").slice(1);
  expect(rows).toHaveLength(10);
  expect(rows[0]).toHaveTextContent("session-23");
  expect(rows[9]).toHaveTextContent("session-14");
  expect(screen.getByText("第 1 / 3 页")).toBeVisible();
  expect(screen.getByRole("button", { name: "上一页" })).toBeDisabled();

  fireEvent.click(screen.getByRole("button", { name: "下一页" }));
  const nextRows = within(table).getAllByRole("row").slice(1);
  expect(nextRows).toHaveLength(10);
  expect(nextRows[0]).toHaveTextContent("session-13");
  expect(screen.getByText("第 2 / 3 页")).toBeVisible();
});
