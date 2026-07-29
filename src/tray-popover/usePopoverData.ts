import { useEffect, useState } from "react";
import { monitorApi } from "../api";
import type { HealthMetrics, LifecyclePreferences, QuotaSnapshot, QuotaState } from "../types";

type HealthNotice = "loading" | "empty" | "error" | "stale" | null;

export type PopoverData = {
  preferences: LifecyclePreferences;
  health: {
    metrics: HealthMetrics | null;
    notice: HealthNotice;
    updatedAt: string | null;
  };
  quota: {
    snapshot: QuotaSnapshot | null;
    notice: "loading" | "stale" | "error" | "cooldown" | null;
  };
};

function extractHealthMetrics(state: import("../types").HealthState) {
  switch (state.status) {
    case "ready":
      return { metrics: state.metrics, notice: null as HealthNotice, updatedAt: state.updatedAt };
    case "stale":
      return { metrics: state.metrics, notice: "stale" as HealthNotice, updatedAt: state.updatedAt };
    case "error":
      return { metrics: state.lastMetrics, notice: "error" as HealthNotice, updatedAt: state.updatedAt };
    case "loading":
      return { metrics: null, notice: "loading" as HealthNotice, updatedAt: null };
  }
}

function extractQuota(state: QuotaState) {
  switch (state.status) {
    case "ready":
      return { snapshot: state.snapshot, notice: null as PopoverData["quota"]["notice"] };
    case "stale":
      return { snapshot: state.snapshot, notice: "stale" as PopoverData["quota"]["notice"] };
    case "error":
      return { snapshot: state.lastSnapshot, notice: "error" as PopoverData["quota"]["notice"] };
    case "loading":
      return { snapshot: null, notice: "loading" as PopoverData["quota"]["notice"] };
    case "cooldown":
      return { snapshot: state.snapshot, notice: "cooldown" as PopoverData["quota"]["notice"] };
  }
}

export function usePopoverData() {
  const [data, setData] = useState<PopoverData | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;

    async function fetchAll() {
      try {
        const [preferences, healthState, quotaState] = await Promise.all([
          monitorApi.getPreferences(),
          monitorApi.getHealth(),
          monitorApi.getQuota(),
        ]);
        if (!active) return;
        setData({
          preferences,
          health: extractHealthMetrics(healthState),
          quota: extractQuota(quotaState),
        });
        setError(null);
      } catch (err) {
        if (!active) return;
        setError(err instanceof Error ? err.message : String(err));
      }
    }

    fetchAll();
    const interval = window.setInterval(fetchAll, 3_000);
    return () => {
      active = false;
      window.clearInterval(interval);
    };
  }, []);

  return { data, error };
}
