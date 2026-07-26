export type HealthMetrics = {
  cpuPercent: number;
  memoryUsedBytes: number;
  memoryTotalBytes: number;
  memoryPressure: "normal" | "warning" | "critical";
  diskAvailableBytes: number;
  diskTotalBytes: number;
  networkDownBytesPerSecond: number;
  networkUpBytesPerSecond: number;
  batteryPercent: number | null;
  batteryCharging: boolean | null;
  uptimeSeconds: number;
};
export type HealthState =
  | { status: "loading" }
  | { status: "ready"; updatedAt: string; metrics: HealthMetrics }
  | {
      status: "stale";
      updatedAt: string;
      metrics: HealthMetrics;
      reason: "paused" | "outdated";
    }
  | {
      status: "error";
      updatedAt: string;
      message: string;
      lastMetrics: HealthMetrics | null;
    };

export type LifecyclePreferences = {
  monitoringPaused: boolean;
  locale: "zh-CN" | "en";
  theme: "system" | "light" | "dark";
  showInDock: boolean;
  launchAtLogin: boolean;
};

export type HealthPoint = {
  observedAt: string;
  metrics: HealthMetrics;
};

export type ApplicationStatus = {
  storageIssue: { detail: string } | null;
};

export type QuotaWindow = {
  name: string;
  remainingPercent: number;
  resetsAt: string | null;
  windowDurationMinutes: number | null;
};

export type QuotaSnapshot = {
  account: { displayName: string; planType: string };
  windows: QuotaWindow[];
  updatedAt: string;
};

export type QuotaState =
  | { status: "loading" }
  | { status: "ready"; snapshot: QuotaSnapshot }
  | { status: "error"; message: string; lastSnapshot: QuotaSnapshot | null };

export type TokenCounts = {
  inputTokens: number;
  cachedInputTokens: number;
  cacheWriteInputTokens: number;
  outputTokens: number;
  reasoningOutputTokens: number;
  totalTokens: number;
};

export type TokenUsageFilters = {
  startAt?: string;
  endAt?: string;
  model?: string;
  sessionId?: string;
};

export type TokenUsageData = {
  totals: TokenCounts;
  models: Array<{ model: string; counts: TokenCounts }>;
  sessions: Array<{
    sessionId: string;
    model: string;
    firstObservedAt: string;
    lastObservedAt: string;
    counts: TokenCounts;
  }>;
  updatedAt: string;
};

export type TokenUsageState =
  | { status: "loading" }
  | { status: "ready"; data: TokenUsageData }
  | { status: "stale"; data: TokenUsageData; reason: "paused" | "outdated" }
  | { status: "error"; message: string; lastData: TokenUsageData | null };
