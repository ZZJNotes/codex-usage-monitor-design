import { useCallback, useEffect, useMemo, useState } from "react";
import { monitorApi } from "./api";
import { translator } from "./i18n";
import type { HealthMetrics, HealthState, LifecyclePreferences } from "./types";
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
  if (locale === "zh-CN") {
    return days > 0 ? `${days} 天 ${hours} 小时` : `${hours} 小时 ${minutes} 分`;
  }
  return days > 0 ? `${days}d ${hours}h` : `${hours}h ${minutes}m`;
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

function MetricsGrid({ metrics, locale }: { metrics: HealthMetrics; locale: "zh-CN" | "en" }) {
  const t = translator(locale);
  const pressure = t(metrics.memoryPressure);
  const battery = metrics.batteryPercent == null ? t("unavailable") : `${metrics.batteryPercent.toFixed(0)}%`;
  return (
    <section className="metrics-grid" aria-label={t("title")}>
      <MetricCard title={t("cpu")} value={`${metrics.cpuPercent.toFixed(1)}%`} help={t("cpuHelp")} />
      <MetricCard
        title={t("memory")}
        value={pressure}
        tone={metrics.memoryPressure}
        help={t("memoryHelp", {
          used: formatBytes(metrics.memoryUsedBytes, locale),
          total: formatBytes(metrics.memoryTotalBytes, locale),
        })}
      />
      <MetricCard
        title={t("disk")}
        value={formatBytes(metrics.diskAvailableBytes, locale)}
        help={t("diskHelp", {
          available: formatBytes(metrics.diskAvailableBytes, locale),
          total: formatBytes(metrics.diskTotalBytes, locale),
        })}
      />
      <MetricCard title={t("network")} value={formatBytes(metrics.networkDownBytesPerSecond, locale, "/s")} help={t("networkHelp")}>
        <div className="metric-split">
          <span>↓ {t("download")}</span>
          <strong>{formatBytes(metrics.networkDownBytesPerSecond, locale, "/s")}</strong>
          <span>↑ {t("upload")}</span>
          <strong>{formatBytes(metrics.networkUpBytesPerSecond, locale, "/s")}</strong>
        </div>
      </MetricCard>
      <MetricCard
        title={t("battery")}
        value={battery}
        help={metrics.batteryCharging == null ? t("batteryHelp") : metrics.batteryCharging ? t("charging") : t("discharging")}
      />
      <MetricCard title={t("uptime")} value={formatDuration(metrics.uptimeSeconds, locale)} help={t("uptimeHelp")} />
    </section>
  );
}

export function App() {
  const [preferences, setPreferences] = useState<LifecyclePreferences>(defaultPreferences);
  const [health, setHealth] = useState<HealthState>({ status: "loading" });
  const [preferenceError, setPreferenceError] = useState<string | null>(null);
  const t = useMemo(() => translator(preferences.locale), [preferences.locale]);

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

  useEffect(() => {
    void monitorApi
      .getPreferences()
      .then(setPreferences)
      .catch((error) => setPreferenceError(error instanceof Error ? error.message : String(error)));
    void refresh();
  }, [refresh]);

  useEffect(() => {
    document.documentElement.lang = preferences.locale;
    document.documentElement.dataset.theme = preferences.theme;
    if (preferences.monitoringPaused) return;
    const timer = window.setInterval(() => void refresh(), 2_000);
    return () => window.clearInterval(timer);
  }, [preferences.locale, preferences.monitoringPaused, preferences.theme, refresh]);

  async function updatePause() {
    try {
      setPreferences(await monitorApi.setPaused(!preferences.monitoringPaused));
      setPreferenceError(null);
    } catch (error) {
      setPreferenceError(error instanceof Error ? error.message : String(error));
    }
  }

  async function updateTheme(theme: LifecyclePreferences["theme"]) {
    try {
      setPreferences(await monitorApi.setTheme(theme));
    } catch (error) {
      setPreferenceError(error instanceof Error ? error.message : String(error));
    }
  }

  async function updateLocale(locale: LifecyclePreferences["locale"]) {
    try {
      setPreferences(await monitorApi.setLocale(locale));
    } catch (error) {
      setPreferenceError(error instanceof Error ? error.message : String(error));
    }
  }

  const shownMetrics = health.status === "ready" ? health.metrics : health.status === "error" ? health.lastMetrics : null;
  const hasMetrics = shownMetrics && shownMetrics.memoryTotalBytes > 0 && shownMetrics.diskTotalBytes > 0;
  const updatedAt = health.status === "ready" || health.status === "error" ? health.updatedAt : null;

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
          {updatedAt && <time dateTime={updatedAt}>{t("updated")} {new Intl.DateTimeFormat(preferences.locale, { hour: "2-digit", minute: "2-digit", second: "2-digit" }).format(new Date(updatedAt))}</time>}
        </div>

        {preferenceError && <div className="error-banner" role="alert">{preferenceError}</div>}
        {health.status === "loading" && !preferences.monitoringPaused && <div className="state-panel" role="status"><span className="spinner" aria-hidden="true" />{t("loading")}</div>}
        {health.status === "loading" && preferences.monitoringPaused && <div className="state-panel" role="status">{t("empty")}</div>}
        {health.status === "error" && <div className="error-banner" role="alert"><div><strong>{t("error")}</strong><span>{health.message}</span></div><button type="button" onClick={() => void refresh()}>{t("retry")}</button></div>}
        {shownMetrics && hasMetrics && <MetricsGrid metrics={shownMetrics} locale={preferences.locale} />}
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
