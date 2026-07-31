import { invoke } from "@tauri-apps/api/core";
import type {
  AccountSummary,
  ApplicationStatus,
  CredentialDeletionStatus,
  DiscoveredAccount,
  ExportReceipt,
  HealthPoint,
  HealthState,
  HistoryCleanupResult,
  LifecyclePreferences,
  MenuBarPreferences,
  NotificationPolicy,
  NotificationStatus,
  OAuthLoginResult,
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
  getNotificationStatus: () =>
    invoke<NotificationStatus>("get_notification_status"),
  getQuota: () => invoke<QuotaState>("get_quota_state"),
  refreshQuota: () => invoke<QuotaState>("refresh_quota"),
  getAllQuotas: () => invoke<Array<[string, QuotaState]>>("get_all_quotas"),
  refreshAccount: (accountKey: string) =>
    invoke<QuotaState>("refresh_account", { accountKey }),
  refreshQuotas: () => invoke<Array<[string, QuotaState]>>("refresh_quotas"),
  removeAccount: (accountKey: string, deleteHistory: boolean) =>
    invoke<void>("remove_account", { accountKey, deleteHistory }),
  setAccountAlias: (accountKey: string, alias: string) =>
    invoke<DiscoveredAccount>("set_account_alias", { accountKey, alias }),
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
  setDockVisibility: (showInDock: boolean) =>
    invoke<LifecyclePreferences>("set_dock_visibility", { showInDock }),
  setLaunchAtLogin: (launchAtLogin: boolean) =>
    invoke<LifecyclePreferences>("set_launch_at_login", { launchAtLogin }),
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
  setNotifications: (notifications: NotificationPolicy) =>
    invoke<LifecyclePreferences>("set_notification_preferences", { notifications }),
  discoverAccounts: () => invoke<DiscoveredAccount[]>("discover_accounts"),
  listAccounts: () => invoke<AccountSummary[]>("list_accounts"),
  activateAccount: (accountKey: string) => invoke<DiscoveredAccount>("activate_account", { accountKey }),
  startCodexLogin: () => invoke<OAuthLoginResult>("start_codex_login"),
};
