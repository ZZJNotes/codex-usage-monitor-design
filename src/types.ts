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
  retentionDays: number;
  locale: "zh-CN" | "en";
  theme: "system" | "light" | "dark";
  showInDock: boolean;
  launchAtLogin: boolean;
  menuBar: MenuBarPreferences;
  notifications: NotificationPolicy;
};

export type NotificationPolicy = {
  enabled: boolean;
  quotaThresholds: number[];
  diskAvailablePercentThreshold: number;
  consecutiveRefreshFailures: number;
};

export type NotificationStatus = {
  activeConditions: Array<{
    key: string;
    kind: "quota" | "authentication" | "refreshExpired" | "disk" | "memoryPressure";
    label: string;
    accountId: string | null;
  }>;
  lastNotification: {
    sentAt: string;
    title: string;
    body: string;
  } | null;
  deliveryError: string | null;
};

export type MenuBarPreferences = {
  parameterIds: MenuBarParameterId[];
  displayLimit: number;
  pinnedAccountId: string | null;
};

export type MenuBarParameterId =
  | "cpu"
  | "memoryPressure"
  | "diskAvailable"
  | "networkDown"
  | "battery"
  | "uptime"
  | `quotaWindow:${string}`;

export type HistoryCleanupResult = {
  quotaSnapshotsDeleted: number;
  tokenEventsDeleted: number;
  systemAggregatesDeleted: number;
  sessionAttributionsDeleted: number;
  accountMetadataDeleted: number;
};

export type ExportReceipt = {
  filename: string;
  destination: string;
};

export type CredentialDeletionStatus = {
  status: "unavailable";
  reason: "keychainIntegrationUnavailable";
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
  account: { id: string; displayName: string; planType: string };
  windows: QuotaWindow[];
  updatedAt: string;
};

export type QuotaState =
  | { status: "loading" }
  | { status: "ready"; snapshot: QuotaSnapshot; nextRefreshAt: string }
  | {
      status: "stale";
      reason: "transport" | "service" | "invalidResponse";
      snapshot: QuotaSnapshot;
      failedAt: string;
      retryAt: string;
    }
  | {
      status: "error";
      reason: "paused" | "storage" | "reauthorization" | "transport" | "service" | "invalidResponse" | "unavailable";
      lastSnapshot: QuotaSnapshot | null;
      failedAt: string;
      retryAt: string | null;
    }
  | {
      status: "cooldown";
      snapshot: QuotaSnapshot | null;
      retryAt: string;
    };

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
  accountKey?: string;
};

export type TokenAccount = { accountKey: string; displayName: string };

export type SessionAttribution = {
  account: TokenAccount | null;
  source: "activeAccount" | "unassigned" | "manual";
  assignedAt: string;
  evidenceSource: string | null;
  evidenceObservedAt: string | null;
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
    assignment: SessionAttribution;
  }>;
  accounts: TokenAccount[];
  updatedAt: string;
};

export type TokenUsageState =
  | { status: "loading" }
  | { status: "ready"; data: TokenUsageData }
  | { status: "stale"; data: TokenUsageData | null; reason: "paused" | "outdated" }
  | { status: "error"; message: string; lastData: TokenUsageData | null };
