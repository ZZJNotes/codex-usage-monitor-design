import { useCallback, useEffect, useMemo, useState } from "react";
import { monitorApi } from "./api";
import { translator } from "./i18n";
import { TokenUsageSection } from "./token-usage/TokenUsageSection";
import { useTokenUsage } from "./token-usage/useTokenUsage";
import type {
  ApplicationStatus,
  HealthMetrics,
  HealthPoint,
  HealthState,
  LifecyclePreferences,
  QuotaSnapshot,
  QuotaState,
} from "./types";
import "./app.css";

const defaultPreferences: LifecyclePreferences = {
  monitoringPaused: false,
  locale: "zh-CN",
  theme: "system",
  showInDock: false,
  launchAtLogin: false,
};

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

type HealthView = {
  metrics: HealthMetrics | null;
  updatedAt: string | null;
  notice: "loading" | "empty" | "error" | "stale" | null;
  errorDetail: string | null;
};

function toHealthView(health: HealthState, paused: boolean): HealthView {
  switch (health.status) {
    case "loading":
      return {
        metrics: null,
        updatedAt: null,
        notice: paused ? "empty" : "loading",
        errorDetail: null,
      };
    case "ready":
      return { metrics: health.metrics, updatedAt: health.updatedAt, notice: null, errorDetail: null };
    case "stale":
      return {
        metrics: health.metrics,
        updatedAt: health.updatedAt,
        notice: paused ? null : "stale",
        errorDetail: null,
      };
    case "error":
      return {
        metrics: health.lastMetrics,
        updatedAt: health.updatedAt,
        notice: "error",
        errorDetail: health.message,
      };
  }
}

type QuotaView = {
  snapshot: QuotaSnapshot | null;
  notice: "loading" | "error" | null;
  error: "paused" | "storage" | "unavailable" | null;
};

function toQuotaView(quota: QuotaState): QuotaView {
  switch (quota.status) {
    case "loading":
      return { snapshot: null, notice: "loading", error: null };
    case "ready":
      return { snapshot: quota.snapshot, notice: null, error: null };
    case "error":
      return {
        snapshot: quota.lastSnapshot,
        notice: "error",
        error: quota.reason,
      };
  }
}

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
  const [applicationStatus, setApplicationStatus] = useState<ApplicationStatus>({ storageIssue: null });
  const [quota, setQuota] = useState<QuotaState>({ status: "loading" });
  const [quotaRefreshing, setQuotaRefreshing] = useState(false);
  const tokenUsage = useTokenUsage();
  const [requestError, setRequestError] = useState<string | null>(null);
  const t = useMemo(() => translator(preferences.locale), [preferences.locale]);
  const formatLocale = useMemo(
    () => window.navigator.language || preferences.locale,
    [preferences.locale],
  );

  const refresh = useCallback(async (force = false) => {
    try {
      setHealth(await (force ? monitorApi.refreshHealth() : monitorApi.getHealth()));
    } catch (error) {
      setHealth({
        status: "error",
        updatedAt: new Date().toISOString(),
        message: errorMessage(error),
        lastMetrics: null,
      });
    }
  }, []);

  const refreshHistory = useCallback(async () => {
    try {
      setHistory(await monitorApi.getHealthHistory());
    } catch (error) {
      setRequestError(errorMessage(error));
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
      setRequestError(errorMessage(error));
    }
  }, []);

  const readQuota = useCallback(async () => {
    try {
      setQuota(await monitorApi.getQuota());
    } catch (error) {
      setQuota({ status: "error", reason: "unavailable", lastSnapshot: null });
    }
  }, []);

  async function refreshQuota() {
    setQuotaRefreshing(true);
    try {
      setQuota(await monitorApi.refreshQuota());
    } catch (error) {
      setQuota({ status: "error", reason: "unavailable", lastSnapshot: null });
    } finally {
      setQuotaRefreshing(false);
    }
  }

  useEffect(() => {
    void monitorApi.showDashboard();
    void refreshPreferences();
    void refreshHistory();
    void refresh();
    void readQuota();
    const preferenceTimer = window.setInterval(() => void refreshPreferences(), 2_000);
    const historyTimer = window.setInterval(() => void refreshHistory(), 60_000);
    const quotaTimer = window.setInterval(() => void readQuota(), 5_000);
    return () => {
      window.clearInterval(preferenceTimer);
      window.clearInterval(historyTimer);
      window.clearInterval(quotaTimer);
    };
  }, [readQuota, refresh, refreshHistory, refreshPreferences]);

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
      setRequestError(errorMessage(error));
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

  const healthView = toHealthView(health, preferences.monitoringPaused);
  const shownMetrics = healthView.metrics;
  const hasMetrics = shownMetrics && shownMetrics.memoryTotalBytes > 0 && shownMetrics.diskTotalBytes > 0;
  const updatedAt = healthView.updatedAt;
  const quotaView = toQuotaView(quota);
  const quotaSnapshot = quotaView.snapshot;

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
        {applicationStatus.storageIssue && <div className="error-banner" role="alert"><div><strong>{t("storageError")}</strong><span>{applicationStatus.storageIssue.detail}. {t("storageRecovery")}</span></div></div>}

        <section className="quota-section" aria-labelledby="quota-heading">
          <div className="quota-heading">
            <div>
              <p className="eyebrow">{t("quotaEyebrow")}</p>
              <h2 id="quota-heading">{t("quotaTitle")}</h2>
              <p>{t("quotaSubtitle")}</p>
            </div>
            <button type="button" onClick={() => void refreshQuota()} disabled={quotaRefreshing || preferences.monitoringPaused}>
              {quotaRefreshing ? t("quotaLoading") : t("quotaRefresh")}
            </button>
          </div>
          {quotaView.notice === "loading" && <div className="quota-state" role="status">{t("quotaLoading")}</div>}
          {quotaView.notice === "error" && <div className="error-banner" role="alert"><div><strong>{t("quotaError")}</strong><span>{quotaView.error === "paused" ? t("quotaPaused") : quotaView.error === "storage" ? t("quotaStorageRecovery") : t("quotaRecovery")}</span></div></div>}
          {quotaSnapshot && <div className="account-line"><span>{t("currentAccount")}: <strong>{quotaSnapshot.account.displayName}</strong></span><span>{t("plan")}: <strong>{quotaSnapshot.account.planType}</strong></span><time dateTime={quotaSnapshot.updatedAt}>{t("updated")} {new Intl.DateTimeFormat(formatLocale, { hour: "2-digit", minute: "2-digit" }).format(new Date(quotaSnapshot.updatedAt))}</time></div>}
          {quotaSnapshot && quotaSnapshot.windows.length === 0 && <div className="quota-state" role="status">{t("quotaEmpty")}</div>}
          {quotaSnapshot && quotaSnapshot.windows.length > 0 && <div className="quota-grid">
            {quotaSnapshot.windows.map((window) => <article className="quota-card" key={window.name}>
              <h3>{window.name}</h3>
              <p className="quota-value">{formatPercent(window.remainingPercent, formatLocale, 0)} {t("remaining")}</p>
              <p>{t("resets")}: {window.resetsAt ? new Intl.DateTimeFormat(formatLocale, { dateStyle: "medium", timeStyle: "short" }).format(new Date(window.resetsAt)) : t("unavailable")}</p>
            </article>)}
          </div>}
        </section>

        <TokenUsageSection
          state={tokenUsage.state}
          locale={preferences.locale}
          formatLocale={formatLocale}
          onQuery={tokenUsage.query}
        />

        {healthView.notice === "loading" && <div className="state-panel" role="status"><span className="spinner" aria-hidden="true" />{t("loading")}</div>}
        {healthView.notice === "empty" && <div className="state-panel" role="status">{t("empty")}</div>}
        {healthView.notice === "error" && <div className="error-banner" role="alert"><div><strong>{t("error")}</strong><span>{healthView.errorDetail}</span></div><button type="button" onClick={() => void refresh(true)}>{t("retry")}</button></div>}
        {healthView.notice === "stale" && <div className="error-banner" role="status">{t("stale")}</div>}
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
