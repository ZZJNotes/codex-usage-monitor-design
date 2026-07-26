import { useCallback, useEffect, useMemo, useState } from "react";
import { monitorApi } from "./api";
import { translator } from "./i18n";
import type {
  ApplicationStatus,
  HealthMetrics,
  HealthPoint,
  HealthState,
  LifecyclePreferences,
} from "./types";
import "./app.css";

const defaultPreferences: LifecyclePreferences = {
  monitoringPaused: false,
  locale: "zh-CN",
  theme: "system",
  showInDock: false,
  launchAtLogin: false,
};

function formatBytes(value: number, locale: string, suffix = "") {
  if (!Number.isFinite(value) || value < 0) return "—";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let size = value;
  let unit = 0;
  while (size >= 1000 && unit < units.length - 1) {
    size /= 1000;
    unit += 1;
  }
  return `${new Intl.NumberFormat(locale, { maximumFractionDigits: size >= 100 ? 0 : 1 }).format(size)} ${units[unit]}${suffix}`;
}

function formatDuration(seconds: number, locale: string) {
  const days = Math.floor(seconds / 86_400);
  const hours = Math.floor((seconds % 86_400) / 3_600);
  const minutes = Math.floor((seconds % 3_600) / 60);
  const unit = (value: number, name: "day" | "hour" | "minute") =>
    new Intl.NumberFormat(locale, {
      style: "unit",
      unit: name,
      unitDisplay: "short",
      maximumFractionDigits: 0,
    }).format(value);
  return days > 0
    ? `${unit(days, "day")} ${unit(hours, "hour")}`
    : `${unit(hours, "hour")} ${unit(minutes, "minute")}`;
}

function formatPercent(value: number, locale: string, maximumFractionDigits: number) {
  return new Intl.NumberFormat(locale, {
    style: "percent",
    maximumFractionDigits,
    minimumFractionDigits: maximumFractionDigits,
  }).format(value / 100);
}

function MetricCard({
  title,
  value,
  help,
  tone,
  children,
}: {
  title: string;
  value: string;
  help: string;
  tone?: "normal" | "warning" | "critical";
  children?: React.ReactNode;
}) {
  return (
    <article className="metric-card">
      <div className="metric-card__header">
        <h2>{title}</h2>
        <span className={`metric-dot metric-dot--${tone ?? "normal"}`} aria-hidden="true" />
      </div>
      <p className="metric-value">{value}</p>
      <p className="metric-help">{help}</p>
      {children}
    </article>
  );
}

function MetricsGrid({
  metrics,
  locale,
  formatLocale,
}: {
  metrics: HealthMetrics;
  locale: "zh-CN" | "en";
  formatLocale: string;
}) {
  const t = translator(locale);
  const pressure = t(metrics.memoryPressure);
  const battery = metrics.batteryPercent == null
    ? t("unavailable")
    : formatPercent(metrics.batteryPercent, formatLocale, 0);
  return (
    <section className="metrics-grid" aria-label={t("title")}>
      <MetricCard title={t("cpu")} value={formatPercent(metrics.cpuPercent, formatLocale, 1)} help={t("cpuHelp")} />
      <MetricCard
        title={t("memory")}
        value={pressure}
        tone={metrics.memoryPressure}
        help={t("memoryHelp", {
          used: formatBytes(metrics.memoryUsedBytes, formatLocale),
          total: formatBytes(metrics.memoryTotalBytes, formatLocale),
        })}
      />
      <MetricCard
        title={t("disk")}
        value={formatBytes(metrics.diskAvailableBytes, formatLocale)}
        help={t("diskHelp", {
          available: formatBytes(metrics.diskAvailableBytes, formatLocale),
          total: formatBytes(metrics.diskTotalBytes, formatLocale),
        })}
      />
      <MetricCard title={t("network")} value={formatBytes(metrics.networkDownBytesPerSecond, formatLocale, "/s")} help={t("networkHelp")}>
        <div className="metric-split">
          <span>↓ {t("download")}</span>
          <strong>{formatBytes(metrics.networkDownBytesPerSecond, formatLocale, "/s")}</strong>
          <span>↑ {t("upload")}</span>
          <strong>{formatBytes(metrics.networkUpBytesPerSecond, formatLocale, "/s")}</strong>
        </div>
      </MetricCard>
      <MetricCard
        title={t("battery")}
        value={battery}
        help={metrics.batteryCharging == null ? t("batteryHelp") : metrics.batteryCharging ? t("charging") : t("discharging")}
      />
      <MetricCard title={t("uptime")} value={formatDuration(metrics.uptimeSeconds, formatLocale)} help={t("uptimeHelp")} />
    </section>
  );
}

export function App() {
  const [preferences, setPreferences] = useState<LifecyclePreferences>(defaultPreferences);
  const [health, setHealth] = useState<HealthState>({ status: "loading" });
  const [history, setHistory] = useState<HealthPoint[]>([]);
  const [applicationStatus, setApplicationStatus] = useState<ApplicationStatus>({ storageError: null });
  const [requestError, setRequestError] = useState<string | null>(null);
  const t = useMemo(() => translator(preferences.locale), [preferences.locale]);
  const formatLocale = useMemo(
    () => window.navigator.language || preferences.locale,
    [preferences.locale],
  );

  const refresh = useCallback(async () => {
    try {
      setHealth(await monitorApi.getHealth());
    } catch (error) {
      setHealth({
        status: "error",
        updatedAt: new Date().toISOString(),
        message: error instanceof Error ? error.message : String(error),
        lastMetrics: null,
      });
    }
  }, []);

  const refreshHistory = useCallback(async () => {
    try {
      setHistory(await monitorApi.getHealthHistory());
    } catch (error) {
      setRequestError(error instanceof Error ? error.message : String(error));
    }
  }, []);

  const refreshPreferences = useCallback(async () => {
    try {
      const [nextPreferences, nextStatus] = await Promise.all([
        monitorApi.getPreferences(),
        monitorApi.getApplicationStatus(),
      ]);
      setPreferences(nextPreferences);
      setApplicationStatus(nextStatus);
    } catch (error) {
      setRequestError(error instanceof Error ? error.message : String(error));
    }
  }, []);

  useEffect(() => {
    void refreshPreferences();
    void refreshHistory();
    void refresh();
    const preferenceTimer = window.setInterval(() => void refreshPreferences(), 2_000);
    const historyTimer = window.setInterval(() => void refreshHistory(), 60_000);
    return () => {
      window.clearInterval(preferenceTimer);
      window.clearInterval(historyTimer);
    };
  }, [refresh, refreshHistory, refreshPreferences]);

  useEffect(() => {
    document.documentElement.lang = preferences.locale;
    document.documentElement.dataset.theme = preferences.theme;
    void refresh();
    if (preferences.monitoringPaused) return;
    const timer = window.setInterval(() => void refresh(), 2_000);
    return () => window.clearInterval(timer);
  }, [preferences.locale, preferences.monitoringPaused, preferences.theme, refresh]);

  async function applyPreferenceMutation(request: Promise<LifecyclePreferences>) {
    try {
      setPreferences(await request);
      setRequestError(null);
    } catch (error) {
      setRequestError(error instanceof Error ? error.message : String(error));
    }
  }
  function updatePause() {
    return applyPreferenceMutation(monitorApi.setPaused(!preferences.monitoringPaused));
  }

  function updateTheme(theme: LifecyclePreferences["theme"]) {
    return applyPreferenceMutation(monitorApi.setTheme(theme));
  }

  function updateLocale(locale: LifecyclePreferences["locale"]) {
    return applyPreferenceMutation(monitorApi.setLocale(locale));
  }

  const shownMetrics = health.status === "ready" || health.status === "stale" ? health.metrics : health.status === "error" ? health.lastMetrics : null;
  const hasMetrics = shownMetrics && shownMetrics.memoryTotalBytes > 0 && shownMetrics.diskTotalBytes > 0;
  const updatedAt = health.status === "ready" || health.status === "stale" || health.status === "error" ? health.updatedAt : null;

  return (
    <div className="app-shell">
      <aside className="sidebar" aria-label={t("appName")}>
        <div className="brand-mark" aria-hidden="true"><span /><span /><span /></div>
        <div className="sidebar-copy">
          <strong>{t("appName")}</strong>
          <span>{t("localOnly")}</span>
        </div>
      </aside>

      <main className="dashboard">
        <header className="page-header">
          <div>
            <p className="eyebrow">{t("eyebrow")}</p>
            <h1>{t("title")}</h1>
            <p className="subtitle">{t("subtitle")}</p>
          </div>
          <button className="pause-button" type="button" onClick={() => void updatePause()}>
            <span className={preferences.monitoringPaused ? "play-icon" : "pause-icon"} aria-hidden="true" />
            {preferences.monitoringPaused ? t("resume") : t("pause")}
          </button>
        </header>

        <div className="status-row" aria-live="polite">
          <span className={`status-pill ${preferences.monitoringPaused ? "status-pill--paused" : ""}`}>
            <span aria-hidden="true" />
            {preferences.monitoringPaused ? t("paused") : t("active")}
          </span>
          {updatedAt && <time dateTime={updatedAt}>{t("updated")} {new Intl.DateTimeFormat(formatLocale, { hour: "2-digit", minute: "2-digit", second: "2-digit" }).format(new Date(updatedAt))}</time>}
          {history.length > 0 && <span>{t("samples", { count: new Intl.NumberFormat(formatLocale).format(history.length) })}</span>}
        </div>

        {requestError && <div className="error-banner" role="alert">{requestError}</div>}
        {applicationStatus.storageError && <div className="error-banner" role="alert">{applicationStatus.storageError}</div>}
        {health.status === "loading" && !preferences.monitoringPaused && <div className="state-panel" role="status"><span className="spinner" aria-hidden="true" />{t("loading")}</div>}
        {health.status === "loading" && preferences.monitoringPaused && <div className="state-panel" role="status">{t("empty")}</div>}
        {health.status === "error" && <div className="error-banner" role="alert"><div><strong>{t("error")}</strong><span>{health.message}</span></div><button type="button" onClick={() => void refresh()}>{t("retry")}</button></div>}
        {health.status === "stale" && !preferences.monitoringPaused && <div className="error-banner" role="status">{t("stale")}</div>}
        {shownMetrics && hasMetrics && <MetricsGrid metrics={shownMetrics} locale={preferences.locale} formatLocale={formatLocale} />}
        {shownMetrics && !hasMetrics && <div className="state-panel" role="status">{t("empty")}</div>}

        <section className="settings-card" aria-labelledby="appearance-heading">
          <div>
            <p className="eyebrow">{t("appearance")}</p>
            <h2 id="appearance-heading">{t("language")} &amp; {t("theme")}</h2>
          </div>
          <label>{t("language")}
            <select value={preferences.locale} onChange={(event) => void updateLocale(event.target.value as LifecyclePreferences["locale"])}>
              <option value="zh-CN">{t("chinese")}</option>
              <option value="en">{t("english")}</option>
            </select>
          </label>
          <label>{t("theme")}
            <select value={preferences.theme} onChange={(event) => void updateTheme(event.target.value as LifecyclePreferences["theme"])}>
              <option value="system">{t("system")}</option>
              <option value="light">{t("light")}</option>
              <option value="dark">{t("dark")}</option>
            </select>
          </label>
        </section>
      </main>
    </div>
  );
}
