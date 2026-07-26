import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, test, vi } from "vitest";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({ invoke }));

import { App } from "./App";

beforeEach(() => {
  invoke.mockReset();
  invoke.mockImplementation((command: string) => {
    if (command === "get_lifecycle_preferences") {
      return Promise.resolve({
        monitoringPaused: false,
        locale: "zh-CN",
        theme: "system",
        showInDock: false,
        launchAtLogin: false,
      });
    }
    if (command === "get_application_status") {
      return Promise.resolve({ storageIssue: null });
    }
    if (command === "get_system_health_history") {
      return Promise.resolve([]);
    }
    if (command === "get_quota_state") {
      return Promise.resolve({
        status: "ready",
        snapshot: {
          account: { displayName: "user@example.com", planType: "plus" },
          windows: [
            {
              name: "codex · primary",
              remainingPercent: 85,
              resetsAt: "2026-08-02T12:00:00Z",
              windowDurationMinutes: 10080,
            },
          ],
          updatedAt: "2026-07-26T12:00:00Z",
        },
      });
    }
    if (command === "get_token_usage") {
      return Promise.resolve({
        status: "ready",
        data: {
          totals: {
            inputTokens: 120,
            cachedInputTokens: 45,
            cacheWriteInputTokens: 12,
            outputTokens: 60,
            reasoningOutputTokens: 14,
            totalTokens: 180,
          },
          models: [{ model: "gpt-5.6", counts: { inputTokens: 120, cachedInputTokens: 45, cacheWriteInputTokens: 12, outputTokens: 60, reasoningOutputTokens: 14, totalTokens: 180 } }],
          sessions: [{
            sessionId: "sanitized-session-01",
            model: "gpt-5.6",
            firstObservedAt: "2026-07-20T10:00:03Z",
            lastObservedAt: "2026-07-20T10:00:05Z",
            counts: { inputTokens: 120, cachedInputTokens: 45, cacheWriteInputTokens: 12, outputTokens: 60, reasoningOutputTokens: 14, totalTokens: 180 },
          }],
          updatedAt: "2026-07-20T10:00:05Z",
        },
      });
    }
    if (command === "get_system_health") {
      return Promise.resolve({
        status: "ready",
        updatedAt: "2026-07-26T10:00:00Z",
        metrics: {
          cpuPercent: 12.5,
          memoryUsedBytes: 8_000_000_000,
          memoryTotalBytes: 16_000_000_000,
          memoryPressure: "normal",
          diskAvailableBytes: 400_000_000_000,
          diskTotalBytes: 1_000_000_000_000,
          networkDownBytesPerSecond: 2_000_000,
          networkUpBytesPerSecond: 300_000,
          batteryPercent: 88,
          batteryCharging: false,
          uptimeSeconds: 7_200,
        },
      });
    }
    return Promise.reject(new Error(`unexpected command ${command}`));
  });
});

test("shows a textual loading state before rendering understandable system metrics", async () => {
  render(<App />);

  expect(screen.getByText("正在读取系统状态…")).toBeVisible();
  expect(await screen.findByText("处理器")).toBeVisible();
  expect(screen.getByText("12.5%")).toBeVisible();
  expect(screen.getByText("正常")).toBeVisible();
  expect(screen.getByText("Codex 额度")).toBeVisible();
  expect(screen.getByText("85% 剩余")).toBeVisible();
  expect(screen.getByRole("button", { name: "暂停监控" })).toBeEnabled();
});

test("shows exact token semantics and accessible time model session query controls", async () => {
  render(<App />);

  expect(await screen.findByRole("heading", { name: "Token 使用" })).toBeVisible();
  expect(screen.getAllByText("180").length).toBeGreaterThanOrEqual(1);
  expect(screen.getByText("总量 = 输入 + 输出；缓存与推理是子集，不会重复相加。")).toBeVisible();
  expect(screen.getByLabelText("开始时间")).toBeEnabled();
  expect(screen.getByLabelText("结束时间")).toBeEnabled();
  expect(screen.getByLabelText("模型")).toBeEnabled();
  expect(screen.getByLabelText("会话")).toBeEnabled();
  expect(screen.getByRole("table", { name: "按会话统计" })).toBeVisible();
  expect(screen.getByText("sanitized-session-01")).toBeVisible();

  fireEvent.change(screen.getByLabelText("模型"), { target: { value: "gpt-5.6" } });
  fireEvent.change(screen.getByLabelText("会话"), { target: { value: "sanitized-session-01" } });
  fireEvent.click(screen.getByRole("button", { name: "查询" }));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("get_token_usage", {
    filters: {
      startAt: undefined,
      endAt: undefined,
      model: "gpt-5.6",
      sessionId: "sanitized-session-01",
    },
  }));
});
