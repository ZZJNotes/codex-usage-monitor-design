import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({ invoke }));

import { App } from "./App";

let quotaResponse: unknown;
let preferencesResponse: Record<string, unknown>;

afterEach(cleanup);

beforeEach(() => {
  quotaResponse = {
    status: "ready",
    snapshot: {
      account: { id: "user@example.com", displayName: "user@example.com", planType: "plus" },
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
  preferencesResponse = {
    monitoringPaused: false,
    locale: "zh-CN",
    theme: "system",
    showInDock: false,
    launchAtLogin: false,
    menuBar: {
      parameterIds: ["cpu", "memoryPressure", "diskAvailable"],
      displayLimit: 3,
      pinnedAccountId: null,
    },
  };
  invoke.mockReset();
  invoke.mockImplementation((command: string) => {
    if (command === "get_lifecycle_preferences") {
      return Promise.resolve(preferencesResponse);
    }
    if (command === "show_dashboard") {
      return Promise.resolve();
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
    if (command === "refresh_quota") {
      return Promise.resolve({
        status: "cooldown",
        snapshot: quotaResponse && (quotaResponse as { snapshot?: unknown }).snapshot || null,
        retryAt: "2099-07-26T12:00:30Z",
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

test("configures an ordered limited menu bar without inventing managed account metadata", async () => {
  invoke.mockImplementation((command: string, args?: unknown) => {
    if (command === "set_menu_bar_preferences") {
      const menuBar = (args as { menuBar: unknown }).menuBar;
      preferencesResponse = { ...preferencesResponse, menuBar };
      return Promise.resolve(preferencesResponse);
    }
    if (command === "get_lifecycle_preferences") return Promise.resolve(preferencesResponse);
    if (command === "show_dashboard") return Promise.resolve();
    if (command === "get_application_status") return Promise.resolve({ storageIssue: null });
    if (command === "get_system_health_history") return Promise.resolve([]);
    if (command === "get_quota_state") return Promise.resolve(quotaResponse);
    if (command === "get_token_usage") return Promise.resolve({ status: "loading" });
    if (command === "get_system_health") return Promise.resolve({ status: "loading" });
    return Promise.reject(new Error(`unexpected command ${command}`));
  });
  render(<App />);

  expect(await screen.findByRole("group", { name: "菜单栏参数" })).toBeVisible();
  expect(screen.queryByRole("option", { name: /user@example.com/ })).not.toBeInTheDocument();
  expect(screen.getByText(/不会把当前账户伪装成托管账户/)).toBeVisible();
  fireEvent.click(screen.getByRole("checkbox", { name: "codex · primary" }));
  await waitFor(() => expect(screen.getByRole("checkbox", { name: "codex · primary" })).toBeChecked());
  fireEvent.click(screen.getByRole("button", { name: "上移 codex · primary" }));
  await waitFor(() => expect(screen.getByLabelText("第 3 位")).toHaveTextContent("3"));
  fireEvent.change(screen.getByLabelText("最多显示数量"), { target: { value: "2" } });
  await waitFor(() => expect(screen.getByLabelText("最多显示数量")).toHaveValue("2"));
  await waitFor(() => expect(invoke).toHaveBeenLastCalledWith("set_menu_bar_preferences", {
    menuBar: {
      parameterIds: ["cpu", "memoryPressure", "quotaWindow:codex · primary", "diskAvailable"],
      displayLimit: 2,
      pinnedAccountId: null,
    },
  }));
  expect(screen.getByText(/键盘操作/)).toBeVisible();
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
  expect(await screen.findByRole("heading", { name: "处理器" })).toBeVisible();
  expect(screen.getByText("12.5%")).toBeVisible();
  expect(screen.getByText("正常")).toBeVisible();
  expect(screen.getByText("Codex 额度")).toBeVisible();
  expect(screen.getByText(/这不是多账户管理/)).toBeVisible();
  expect(screen.getByText("85% 剩余")).toBeVisible();
  expect(screen.getByRole("button", { name: "暂停监控" })).toBeEnabled();
});

test("shows exact token semantics and accessible time model session query controls", async () => {
  render(<App />);

  expect(await screen.findByRole("heading", { name: "Token 使用" })).toBeVisible();
  expect(screen.getByText(/Token 统计已更新/)).toBeVisible();
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

test("manual refresh announces cooldown and English settings retain accessible names", async () => {
  preferencesResponse = {
    ...preferencesResponse,
    locale: "en",
  };
  render(<App />);

  const refresh = await screen.findByRole("button", { name: "Refresh quota" });
  fireEvent.click(refresh);
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("refresh_quota"));
  expect(await screen.findByText("Refresh cooling down")).toBeVisible();
  expect(screen.getByRole("group", { name: "Menu bar parameters" })).toBeVisible();
  expect(screen.getByLabelText("Pinned account")).toBeEnabled();
  expect(screen.getByText(/VoiceOver reads order controls/)).toBeVisible();
});
