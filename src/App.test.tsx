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
    retentionDays: 90,
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
  invoke.mockImplementation((command: string, args?: unknown) => {
    if (command === "set_menu_bar_preferences") {
      const menuBar = (args as { menuBar: unknown }).menuBar;
      preferencesResponse = { ...preferencesResponse, menuBar };
      return Promise.resolve(preferencesResponse);
    }
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
          accounts: [{ accountKey: "acct_a", displayName: "Account A" }],
          sessions: [{
            sessionId: "sanitized-session-01",
            model: "gpt-5.6",
            firstObservedAt: "2026-07-20T10:00:03Z",
            lastObservedAt: "2026-07-20T10:00:05Z",
            counts: { inputTokens: 120, cachedInputTokens: 45, cacheWriteInputTokens: 12, outputTokens: 60, reasoningOutputTokens: 14, totalTokens: 180 },
            assignment: {
              account: null,
              source: "unassigned",
              assignedAt: "2026-07-20T10:00:03Z",
              evidenceSource: null,
              evidenceObservedAt: null,
            },
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
    if (command === "reassign_token_session") {
      return Promise.resolve();
    }
    if (command === "get_credential_deletion_status") {
      return Promise.resolve({ status: "unavailable", reason: "keychainIntegrationUnavailable" });
    }
    if (command === "set_retention_days") {
      preferencesResponse = { ...preferencesResponse, retentionDays: 30 };
      return Promise.resolve(preferencesResponse);
    }
    if (command === "cleanup_expired_history" || command === "clear_history") {
      return Promise.resolve({ quotaSnapshotsDeleted: 1, tokenEventsDeleted: 2, systemAggregatesDeleted: 3, sessionAttributionsDeleted: 1, accountMetadataDeleted: 1 });
    }
    if (command === "export_statistics") {
      return Promise.resolve({
        filename: "codex-usage-2026-07-27.json",
        destination: "~/Downloads/codex-usage-2026-07-27.json",
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

test("keeps a disappeared quota window configurable so it can be removed", async () => {
  preferencesResponse = {
    ...preferencesResponse,
    menuBar: {
      parameterIds: ["quotaWindow:retired window", "cpu"],
      displayLimit: 2,
      pinnedAccountId: null,
    },
  };
  render(<App />);

  const retired = await screen.findByRole("checkbox", { name: "retired window (不可用)" });
  expect(retired).toBeChecked();
  fireEvent.click(retired);
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("set_menu_bar_preferences", {
    menuBar: {
      parameterIds: ["cpu"],
      displayLimit: 2,
      pinnedAccountId: null,
    },
  }));
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
      accountKey: undefined,
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

test("shows unassigned ownership, filters by account, and offers an accessible correction", async () => {
  render(<App />);

  expect(await screen.findByText("未归属")).toBeVisible();
  expect(screen.getByText(/来源.*未归属/)).toBeVisible();
  fireEvent.change(screen.getByLabelText("账户"), { target: { value: "unassigned" } });
  fireEvent.click(screen.getByRole("button", { name: "查询" }));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("get_token_usage", {
    filters: {
      startAt: undefined,
      endAt: undefined,
      model: undefined,
      sessionId: undefined,
      accountKey: "unassigned",
    },
  }));

  fireEvent.change(screen.getByLabelText("修正 sanitized-session-01 的账户归属"), {
    target: { value: "acct_a" },
  });
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("reassign_token_session", {
    sessionId: "sanitized-session-01",
    accountKey: "acct_a",
  }));
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

test("offers local retention cleanup and safe export while explaining credential deletion availability", async () => {
  render(<App />);

  expect(await screen.findByRole("heading", { name: "数据与隐私" })).toBeVisible();
  expect(screen.getByLabelText("统计保留期")).toHaveValue("90");
  expect(screen.getByRole("button", { name: "清理过期历史" })).toBeEnabled();
  expect(screen.getByRole("button", { name: "清空全部历史" })).toBeEnabled();
  expect(screen.getByRole("button", { name: "导出 JSON" })).toBeEnabled();
  expect(screen.getByRole("button", { name: "导出 CSV" })).toBeEnabled();
  expect(screen.getByRole("button", { name: "删除账户凭据" })).toBeDisabled();
  expect(screen.getByText(/Keychain 多账户授权完成后才可用/)).toBeVisible();

  fireEvent.change(screen.getByLabelText("统计保留期"), { target: { value: "30" } });
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("set_retention_days", { retentionDays: 30 }));
  const historyReadsBeforeCleanup = invoke.mock.calls.filter(([command]) => command === "get_system_health_history").length;
  fireEvent.click(screen.getByRole("button", { name: "清理过期历史" }));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("cleanup_expired_history"));
  await waitFor(() => {
    expect(invoke.mock.calls.filter(([command]) => command === "get_system_health_history").length).toBeGreaterThan(historyReadsBeforeCleanup);
    expect(invoke).toHaveBeenCalledWith("get_token_usage", { filters: {} });
    expect(invoke).toHaveBeenCalledWith("get_quota_state");
  });
  expect(await screen.findByText("本地数据操作已完成。")).toBeVisible();
  fireEvent.click(screen.getByRole("button", { name: "导出 JSON" }));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("export_statistics", { format: "json" }));
  expect(await screen.findByText("导出成功：~/Downloads/codex-usage-2026-07-27.json")).toBeVisible();
});

test("explains that a persisted pause survives restart until the user resumes", async () => {
  preferencesResponse = {
    ...preferencesResponse,
    monitoringPaused: true,
  };

  render(<App />);

  expect(await screen.findByText(/重启后仍保持暂停/)).toBeVisible();
  expect(screen.getByRole("button", { name: "恢复监控" })).toBeEnabled();
  expect(screen.getByRole("button", { name: "刷新额度" })).toBeDisabled();
});

test("UI state fixtures contain no credentials, session content, or work paths", () => {
  const fixture = JSON.stringify({
    quotaResponse,
    preferencesResponse,
    tokenUsage: {
      status: "ready",
      data: { totals: { inputTokens: 1, outputTokens: 1, totalTokens: 2 }, sessions: [], models: [], accounts: [] },
    },
    systemHealth: { status: "ready", metrics: { cpuPercent: 10, memoryPressure: "normal" } },
    applicationStatus: { storageIssue: null },
    exportReceipt: { filename: "codex-usage.json", destination: "~/Downloads/codex-usage.json" },
  }).toLowerCase();
  for (const prohibited of ["access_token", "refresh_token", "id_token", "bearer ", "sk-", "eyj", "prompt", "reply", "command", "attachment", "work_path", "/users/"]) {
    expect(fixture).not.toContain(prohibited);
  }
});
