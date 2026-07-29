import { useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { monitorApi } from "../api";
import { translator } from "../i18n";
import type { HealthMetrics } from "../types";
import { usePopoverData } from "./usePopoverData";

function formatPercent(value: number) {
  return `${Math.round(value)}%`;
}

type SystemMetric = {
  label: string;
  value: string;
  percent?: number | null;
  ok?: boolean;
};

function MetricGauge({ label, value, percent, ok = true }: SystemMetric) {
  const normalized = percent == null ? 0 : Math.min(Math.max(percent, 0), 100);
  const circumference = 2 * Math.PI * 28;
  const tone = ok ? "" : " system-gauge--warn";

  return (
    <div className={`system-gauge${tone}`} role="img" aria-label={`${label}: ${value}`}>
      <svg viewBox="0 0 72 72" aria-hidden="true">
        <circle className="system-gauge__track" cx="36" cy="36" r="28" />
        <circle
          className="system-gauge__value"
          cx="36"
          cy="36"
          r="28"
          style={{ strokeDasharray: circumference, strokeDashoffset: circumference * (1 - normalized / 100) }}
        />
      </svg>
      <div className="system-gauge__content">
        <span className="system-gauge__value-text">{value}</span>
        <span className="system-gauge__label">{label}</span>
      </div>
    </div>
  );
}

function MetricDetail({ label, value, ok = true }: SystemMetric) {
  return (
    <div className={`system-detail${ok ? "" : " system-detail--warn"}`}>
      <span>{label}</span>
      <strong>{value}</strong>
    </div>
  );
}

function healthMetricsRows(m: HealthMetrics) {
  const memPct = m.memoryTotalBytes > 0 ? (m.memoryUsedBytes / m.memoryTotalBytes) * 100 : 0;
  const dskPct = m.diskTotalBytes > 0 ? ((m.diskTotalBytes - m.diskAvailableBytes) / m.diskTotalBytes) * 100 : 0;
  return [
    { label: "CPU", value: formatPercent(m.cpuPercent), percent: m.cpuPercent, ok: m.cpuPercent < 80 },
    { label: "MEM", value: formatPercent(memPct), percent: memPct, ok: m.memoryPressure === "normal" },
    { label: "DSK", value: formatPercent(dskPct), percent: dskPct, ok: dskPct < 90 },
    { label: "BAT", value: m.batteryPercent != null ? formatPercent(m.batteryPercent) : "—", percent: m.batteryPercent, ok: m.batteryPercent == null || m.batteryPercent > 20 },
    { label: "NET", value: `↓ ${(m.networkDownBytesPerSecond / 1_000_000).toFixed(1)} MB/s`, ok: true },
    { label: "UP", value: `${Math.floor(m.uptimeSeconds / 3600)}h ${Math.floor((m.uptimeSeconds % 3600) / 60)}m`, ok: true },
  ];
}

function timeAgo(iso: string): string {
  const diff = Date.now() - new Date(iso).getTime();
  const sec = Math.floor(diff / 1000);
  if (sec < 60) return `${sec}秒前`;
  const min = Math.floor(sec / 60);
  if (min < 60) return `${min}分钟前`;
  const h = Math.floor(min / 60);
  if (h < 24) return `${h}小时前`;
  return `${Math.floor(h / 24)}天前`;
}

export function TrayPopover() {
  const { data, error } = usePopoverData();

  const handlePause = useCallback(async () => {
    if (!data) return;
    await monitorApi.setPaused(!data.preferences.monitoringPaused);
  }, [data]);

  const handleOpenDashboard = useCallback(async () => {
    await invoke("show_dashboard");
  }, []);

  const handleQuit = useCallback(async () => {
    await invoke("quit_app");
  }, []);

  if (!data) {
    return (
      <div className="popover-shell">
        <div className="popover-status">
          <span className="terminal-spinner" aria-hidden="true" />
          <span>Loading…</span>
        </div>
      </div>
    );
  }

  const t = translator(data.preferences.locale);
  const { health, quota, preferences } = data;
  const isPaused = preferences.monitoringPaused;
  const metrics = health.metrics;
  const snapshot = quota.snapshot;
  const rows = metrics ? healthMetricsRows(metrics) : [];

  return (
    <div className="popover-shell">
      {/* Toolbar */}
      <div className="popover-toolbar">
        <div className="brand-mark" aria-hidden="true"><span /><span /><span /></div>
        <span className="popover-title">CODEX MONITOR</span>
        <span className={`popover-status-badge ${isPaused ? "popover-status-badge--paused" : ""}`}>
          <span aria-hidden="true" />
          {isPaused ? t("paused") : t("active")}
        </span>
      </div>

      {/* Account & Quota */}
      {snapshot && (
        <div className="popover-group">
          <div className="popover-account-header">
            <div className="popover-account-brand" aria-hidden="true"><span /><span /><span /></div>
            <span className="popover-account-name">{snapshot.account.displayName}</span>
            <span className="popover-plan-badge">{snapshot.account.planType}</span>
          </div>
          <div className="popover-quota-cards">
            {snapshot.windows.map((window) => (
              <div className="popover-quota-card" key={window.name}>
                <div className="popover-quota-card__row">
                  <span className="popover-quota-card__label">{window.name}</span>
                  <span className={`popover-quota-card__pct ${window.remainingPercent < 30 ? "popover-quota-card__pct--low" : ""}`}>
                    {formatPercent(window.remainingPercent)}
                  </span>
                </div>
                <div className="progress-line">
                  <div
                    className={`progress-line__fill ${window.remainingPercent < 30 ? "progress-line__fill--warn" : ""}`}
                    style={{ width: `${window.remainingPercent}%` }}
                  />
                  <div
                    className={`progress-line__dot ${window.remainingPercent < 30 ? "progress-line__dot--warn" : ""}`}
                    style={{ left: `${window.remainingPercent}%` }}
                  />
                </div>
              </div>
            ))}
          </div>
          <span className="popover-updated">{t("updated")} {timeAgo(snapshot.updatedAt)}</span>
        </div>
      )}

      {/* System Metrics */}
      {rows.length > 0 && (
        <div className="popover-group popover-system-metrics">
          <div className="system-gauges">
            {rows.slice(0, 4).map((row) => <MetricGauge key={row.label} {...row} />)}
          </div>
          <div className="system-details">
            {rows.slice(4).map((row) => <MetricDetail key={row.label} {...row} />)}
          </div>
        </div>
      )}

      {error && <div className="popover-error">{error}</div>}

      {/* Actions */}
      <div className="popover-actions">
        <button className="popover-action-btn" type="button" onClick={() => monitorApi.refreshQuota().catch(() => {})}>
          ↻ {t("quotaRefresh")}
        </button>
        <button className="popover-action-btn" type="button" onClick={handlePause}>
          {isPaused ? "▶" : "⏸"} {isPaused ? t("resume") : t("pause")}
        </button>
        <button className="popover-action-btn" type="button" onClick={handleOpenDashboard}>
          ◆ {preferences.locale === "zh-CN" ? "打开仪表盘" : "Open dashboard"}
        </button>
      </div>
      <div className="popover-actions-divider" />
      <div className="popover-actions">
        <button className="popover-action-btn popover-action-btn--quit" type="button" onClick={handleQuit}>
          ⊗ {preferences.locale === "zh-CN" ? "退出" : "Quit"}
        </button>
      </div>
    </div>
  );
}
