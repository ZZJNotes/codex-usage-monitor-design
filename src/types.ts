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
