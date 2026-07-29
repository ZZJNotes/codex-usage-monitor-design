import { useEffect, useMemo, useState } from "react";

import { translator } from "../i18n";
import type { LifecyclePreferences, TokenCounts, TokenUsageFilters, TokenUsageState } from "../types";

function optionalUtc(value: string) {
  return value ? new Date(value).toISOString() : undefined;
}

const SESSION_PAGE_SIZE = 10;

export function TokenUsageSection({ state, locale, formatLocale, onQuery, onReassign }: {
  state: TokenUsageState;
  locale: LifecyclePreferences["locale"];
  formatLocale: string;
  onQuery: (filters: TokenUsageFilters) => void;
  onReassign: (sessionId: string, accountKey: string | null) => void;
}) {
  const t = translator(locale);
  const [startAt, setStartAt] = useState("");
  const [endAt, setEndAt] = useState("");
  const [model, setModel] = useState("");
  const [sessionId, setSessionId] = useState("");
  const [accountKey, setAccountKey] = useState("");
  const [sessionPage, setSessionPage] = useState(1);
  const data = state.status === "ready"
    ? state.data
    : state.status === "stale" ? state.data
    : state.status === "error" ? state.lastData : null;
  const countItems: Array<[keyof TokenCounts, string]> = [
    ["inputTokens", t("tokenInput")], ["cachedInputTokens", t("tokenCached")],
    ["cacheWriteInputTokens", t("tokenCacheWrite")], ["outputTokens", t("tokenOutput")],
    ["reasoningOutputTokens", t("tokenReasoning")], ["totalTokens", t("tokenTotal")],
  ];
  const number = (value: number) => new Intl.NumberFormat(formatLocale).format(value);
  const orderedSessions = useMemo(
    () => [...(data?.sessions ?? [])].sort((left, right) =>
      new Date(right.lastObservedAt).getTime() - new Date(left.lastObservedAt).getTime()),
    [data?.sessions],
  );
  const sessionPageCount = Math.max(1, Math.ceil(orderedSessions.length / SESSION_PAGE_SIZE));
  const visibleSessions = orderedSessions.slice(
    (sessionPage - 1) * SESSION_PAGE_SIZE,
    sessionPage * SESSION_PAGE_SIZE,
  );

  useEffect(() => {
    setSessionPage((current) => Math.min(current, sessionPageCount));
  }, [sessionPageCount]);

  return <section className="token-section" aria-labelledby="token-heading">
    <div className="token-heading"><div>
      <p className="eyebrow">{t("tokenEyebrow")}</p>
      <h2 id="token-heading">{t("tokenTitle")}</h2>
      <p>{t("tokenSubtitle")}</p>
    </div></div>
    <form className="token-filters" onSubmit={(event) => {
      event.preventDefault();
      setSessionPage(1);
      onQuery({ startAt: optionalUtc(startAt), endAt: optionalUtc(endAt), model: model.trim() || undefined, sessionId: sessionId.trim() || undefined, accountKey: accountKey || undefined });
    }}>
      <label>{t("tokenStart")}<input type="datetime-local" value={startAt} onChange={(event) => setStartAt(event.target.value)} /></label>
      <label>{t("tokenEnd")}<input type="datetime-local" value={endAt} onChange={(event) => setEndAt(event.target.value)} /></label>
      <label>{t("tokenModel")}<input type="search" value={model} onChange={(event) => setModel(event.target.value)} /></label>
      <label>{t("tokenSession")}<input type="search" value={sessionId} onChange={(event) => setSessionId(event.target.value)} /></label>
      <label>{t("tokenAccount")}<select value={accountKey} onChange={(event) => setAccountKey(event.target.value)}>
        <option value="">{t("tokenAllAccounts")}</option>
        <option value="unassigned">{t("tokenUnassigned")}</option>
        {data?.accounts.map((account) => <option key={account.accountKey} value={account.accountKey}>{account.displayName}</option>)}
      </select></label>
      <button type="submit">{t("tokenFilter")}</button>
    </form>
    <p className="token-semantics">{t("tokenSemantics")}</p>
    {state.status === "loading" && <div className="token-state" role="status">{t("tokenLoading")}</div>}
    {state.status === "ready" && <div className="token-state" role="status">{t("tokenFresh")} · <time dateTime={state.data.updatedAt}>{new Intl.DateTimeFormat(formatLocale, { dateStyle: "short", timeStyle: "medium" }).format(new Date(state.data.updatedAt))}</time></div>}
    {state.status === "stale" && <div className="error-banner" role="status"><div><strong>{state.reason === "paused" ? t("tokenStalePaused") : t("tokenStaleOutdated")}</strong>{state.data && <time dateTime={state.data.updatedAt}>{new Intl.DateTimeFormat(formatLocale, { dateStyle: "short", timeStyle: "medium" }).format(new Date(state.data.updatedAt))}</time>}</div></div>}
    {state.status === "error" && <div className="error-banner" role="alert"><div><strong>{t("tokenError")}</strong><span>{state.message}</span>{state.lastData && <time dateTime={state.lastData.updatedAt}>{new Intl.DateTimeFormat(formatLocale, { dateStyle: "short", timeStyle: "medium" }).format(new Date(state.lastData.updatedAt))}</time>}</div></div>}
    {data && data.sessions.length === 0 && <div className="token-state" role="status">{t("tokenEmpty")}</div>}
    {data && data.sessions.length > 0 && <>
      <div className="token-counts" aria-label={t("tokenTitle")}>
        {countItems.map(([key, label]) => <article key={key} className={key === "totalTokens" ? "token-count token-count--total" : "token-count"}>
          <h3>{label}</h3><p>{number(data.totals[key])}</p>
        </article>)}
      </div>
      <div className="token-tables">
        <div className="token-table-wrap"><h3>{t("tokenByModel")}</h3>
          <table aria-label={t("tokenByModel")}><thead><tr><th>{t("tokenModel")}</th><th>{t("tokenTotal")}</th></tr></thead>
            <tbody>{data.models.map((item) => <tr key={item.model}><td>{item.model}</td><td>{number(item.counts.totalTokens)}</td></tr>)}</tbody></table>
        </div>
        <div className="token-table-wrap"><h3>{t("tokenBySession")}</h3>
          <table aria-label={t("tokenBySession")}><thead><tr><th>{t("tokenSession")}</th><th>{t("tokenModel")}</th><th>{t("tokenTotal")}</th><th>{t("tokenLastSeen")}</th><th>{t("tokenAccount")}</th><th>{t("tokenCorrection")}</th></tr></thead>
            <tbody>{visibleSessions.map((item) => <tr key={`${item.sessionId}-${item.model}`}>
              <td>{item.sessionId}</td><td>{item.model}</td><td>{number(item.counts.totalTokens)}</td><td><time dateTime={item.lastObservedAt}>{new Intl.DateTimeFormat(formatLocale, { dateStyle: "short", timeStyle: "short" }).format(new Date(item.lastObservedAt))}</time></td>
              <td><strong>{item.assignment.account?.displayName ?? t("tokenUnassigned")}</strong><span className="token-assignment-audit">{t("tokenSource")}: {t(item.assignment.source === "activeAccount" ? "tokenSourceActive" : item.assignment.source === "manual" ? "tokenSourceManual" : "tokenSourceUnassigned")} · <time dateTime={item.assignment.assignedAt}>{new Intl.DateTimeFormat(formatLocale, { dateStyle: "short", timeStyle: "short" }).format(new Date(item.assignment.assignedAt))}</time></span></td>
              <td><select aria-label={t("tokenCorrectFor", { session: item.sessionId })} value={item.assignment.account?.accountKey ?? "unassigned"} onChange={(event) => onReassign(item.sessionId, event.target.value === "unassigned" ? null : event.target.value)}>
                <option value="unassigned">{t("tokenUnassigned")}</option>
                {data.accounts.map((account) => <option key={account.accountKey} value={account.accountKey}>{account.displayName}</option>)}
              </select></td>
            </tr>)}</tbody></table>
          {sessionPageCount > 1 && <nav className="token-pagination" aria-label={t("tokenPagination")}>
            <button type="button" disabled={sessionPage === 1} onClick={() => setSessionPage((page) => page - 1)}>{t("previousPage")}</button>
            <span aria-live="polite">{t("pageStatus", { current: String(sessionPage), total: String(sessionPageCount) })}</span>
            <button type="button" disabled={sessionPage === sessionPageCount} onClick={() => setSessionPage((page) => page + 1)}>{t("nextPage")}</button>
          </nav>}
        </div>
      </div>
    </>}
  </section>;
}
