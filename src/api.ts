import { invoke } from "@tauri-apps/api/core";
import type { HealthState, LifecyclePreferences } from "./types";

export const monitorApi = {
  getHealth: () => invoke<HealthState>("get_system_health"),
  getPreferences: () =>
    invoke<LifecyclePreferences>("get_lifecycle_preferences"),
  setPaused: (paused: boolean) =>
    invoke<LifecyclePreferences>("set_monitoring_paused", { paused }),
  setTheme: (theme: LifecyclePreferences["theme"]) =>
    invoke<LifecyclePreferences>("set_theme", { theme }),
  setLocale: (locale: LifecyclePreferences["locale"]) =>
    invoke<LifecyclePreferences>("set_locale", { locale }),
};
