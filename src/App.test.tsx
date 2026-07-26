import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({ invoke }));

import { App } from "./App";

let quotaResponse: unknown;

afterEach(cleanup);

beforeEach(() => {
  quotaResponse = {
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
  };
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
      return Promise.resolve(quotaResponse);
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

test("gives storage-specific recovery instead of blaming Codex authentication", async () => {
  quotaResponse = { status: "error", reason: "storage", lastSnapshot: null };

  render(<App />);

  expect(await screen.findByText(/无法保存快照/)).toBeVisible();
  expect(screen.queryByText(/确认 Codex 已使用 ChatGPT 账户登录/)).not.toBeInTheDocument();
});

test("shows a textual loading state before rendering understandable system metrics", async () => {
  render(<App />);

  expect(screen.getByText("正在读取系统状态…")).toBeVisible();
  expect(await screen.findByText("处理器")).toBeVisible();
  expect(screen.getByText("12.5%")).toBeVisible();
  expect(screen.getByText("正常")).toBeVisible();
  expect(screen.getByText("Codex 额度")).toBeVisible();
  expect(screen.getByText(/这不是多账户管理/)).toBeVisible();
  expect(screen.getByText("85% 剩余")).toBeVisible();
  expect(screen.getByRole("button", { name: "暂停监控" })).toBeEnabled();
});

test("keeps the last quota visible and explains transport staleness without showing zero", async () => {
  quotaResponse = {
    status: "stale",
    reason: "transport",
    snapshot: {
      account: { id: "account-1", displayName: "user@example.com", planType: "plus" },
      windows: [{
        name: "codex · primary",
        remainingPercent: 73,
        resetsAt: null,
        windowDurationMinutes: 300,
      }],
      updatedAt: "2026-07-26T12:00:00Z",
    },
    failedAt: "2026-07-26T12:10:00Z",
    retryAt: "2026-07-26T12:10:30Z",
  };

  render(<App />);

  expect(await screen.findByText("额度数据已过期")).toBeVisible();
  expect(screen.getByText(/网络不可用/)).toBeVisible();
  expect(screen.getByText("73% 剩余")).toBeVisible();
  expect(screen.queryByText("0% 剩余")).not.toBeInTheDocument();
});

test("explains that authentication failure needs account reauthorization", async () => {
  quotaResponse = {
    status: "error",
    reason: "reauthorization",
    lastSnapshot: null,
    failedAt: "2026-07-26T12:10:00Z",
    retryAt: null,
  };

  render(<App />);

  expect(await screen.findByText("需要重新授权")).toBeVisible();
  expect(screen.getByText(/在账户管理中重新授权此账户/)).toBeVisible();
});

test("shows manual refresh cooldown as disabled feedback", async () => {
  quotaResponse = {
    status: "cooldown",
    snapshot: null,
    retryAt: "2099-07-26T12:00:30Z",
  };

  render(<App />);

  expect(await screen.findByText("刷新冷却中")).toBeVisible();
  expect(screen.getByRole("button", { name: /后可刷新/ })).toBeDisabled();
});
