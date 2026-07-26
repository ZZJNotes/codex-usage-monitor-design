import { useCallback, useEffect, useState } from "react";

import { monitorApi } from "../api";
import type { TokenUsageFilters, TokenUsageState } from "../types";

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

export function useTokenUsage() {
  const [state, setState] = useState<TokenUsageState>({ status: "loading" });
  const [filters, setFilters] = useState<TokenUsageFilters>({});
  const read = useCallback(async (nextFilters: TokenUsageFilters) => {
    try {
      setState(await monitorApi.getTokenUsage(nextFilters));
    } catch (error) {
      setState({ status: "error", message: errorMessage(error), lastData: null });
    }
  }, []);

  useEffect(() => {
    void read(filters);
    const timer = window.setInterval(() => void read(filters), 3_000);
    return () => window.clearInterval(timer);
  }, [filters, read]);

  return {
    state,
    refresh() {
      return read(filters);
    },
    query(nextFilters: TokenUsageFilters) {
      setFilters(nextFilters);
    },
    async reassign(sessionId: string, accountKey: string | null) {
      try {
        await monitorApi.reassignTokenSession(sessionId, accountKey);
        await read(filters);
      } catch (error) {
        setState((current) => ({
          status: "error",
          message: errorMessage(error),
          lastData: current.status === "ready" ? current.data
            : current.status === "stale" ? current.data
            : current.status === "error" ? current.lastData : null,
        }));
      }
    },
  };
}
