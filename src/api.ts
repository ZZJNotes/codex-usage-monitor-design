import { invoke } from "@tauri-apps/api/core";
import type {
  ApplicationStatus,
  CredentialDeletionStatus,
  ExportReceipt,
  HealthPoint,
  HealthState,
  HistoryCleanupResult,
  LifecyclePreferences,
  MenuBarPreferences,
  QuotaState,
  TokenUsageFilters,
  TokenUsageState,
} from "./types";

export const monitorApi = {
  showDashboard: () => invoke<void>("show_dashboard"),
  getHealth: () => invoke<HealthState>("get_system_health"),
  refreshHealth: () => invoke<HealthState>("refresh_system_health"),
  getHealthHistory: () =>
    invoke<HealthPoint[]>("get_system_health_history"),
  getApplicationStatus: () =>
    invoke<ApplicationStatus>("get_application_status"),
  getQuota: () => invoke<QuotaState>("get_quota_state"),
  refreshQuota: () => invoke<QuotaState>("refresh_quota"),
  getTokenUsage: (filters: TokenUsageFilters = {}) =>
    invoke<TokenUsageState>("get_token_usage", { filters }),
  refreshTokenUsage: (filters: TokenUsageFilters = {}) =>
    invoke<TokenUsageState>("refresh_token_usage", { filters }),
  recoverQuota: () => invoke<QuotaState>("recover_quota"),
  reassignTokenSession: (sessionId: string, accountKey: string | null) =>
    invoke<void>("reassign_token_session", { sessionId, accountKey }),
  getPreferences: () =>
    invoke<LifecyclePreferences>("get_lifecycle_preferences"),
  setPaused: (paused: boolean) =>
    invoke<LifecyclePreferences>("set_monitoring_paused", { paused }),
  setTheme: (theme: LifecyclePreferences["theme"]) =>
    invoke<LifecyclePreferences>("set_theme", { theme }),
  setLocale: (locale: LifecyclePreferences["locale"]) =>
    invoke<LifecyclePreferences>("set_locale", { locale }),
  setMenuBar: (menuBar: MenuBarPreferences) =>
    invoke<LifecyclePreferences>("set_menu_bar_preferences", { menuBar }),
  setRetentionDays: (retentionDays: number) =>
    invoke<LifecyclePreferences>("set_retention_days", { retentionDays }),
  cleanupExpiredHistory: () =>
    invoke<HistoryCleanupResult>("cleanup_expired_history"),
  clearHistory: () => invoke<HistoryCleanupResult>("clear_history"),
  deleteAccountHistory: (accountKey: string) =>
    invoke<HistoryCleanupResult>("delete_account_history", { accountKey }),
  exportStatistics: (format: "json" | "csv") =>
    invoke<ExportReceipt>("export_statistics", { format }),
  getCredentialDeletionStatus: () =>
    invoke<CredentialDeletionStatus>("get_credential_deletion_status"),
  requestCredentialDeletion: (accountKey: string) =>
    invoke<void>("request_credential_deletion", { accountKey }),
};
