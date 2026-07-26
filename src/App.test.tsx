import { render, screen } from "@testing-library/react";
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
  expect(screen.getByText(/这不是多账户管理/)).toBeVisible();
  expect(screen.getByText("85% 剩余")).toBeVisible();
  expect(screen.getByRole("button", { name: "暂停监控" })).toBeEnabled();
});
