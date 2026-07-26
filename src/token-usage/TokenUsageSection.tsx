import { useState } from "react";

import { translator } from "../i18n";
import type { LifecyclePreferences, TokenCounts, TokenUsageFilters, TokenUsageState } from "../types";

function optionalUtc(value: string) {
  return value ? new Date(value).toISOString() : undefined;
}

export function TokenUsageSection({ state, locale, formatLocale, onQuery }: {
  state: TokenUsageState;
  locale: LifecyclePreferences["locale"];
  formatLocale: string;
  onQuery: (filters: TokenUsageFilters) => void;
}) {
  const t = translator(locale);
  const [startAt, setStartAt] = useState("");
  const [endAt, setEndAt] = useState("");
  const [model, setModel] = useState("");
  const [sessionId, setSessionId] = useState("");
  const data = state.status === "ready" || state.status === "stale"
    ? state.data
    : state.status === "error" ? state.lastData : null;
  const countItems: Array<[keyof TokenCounts, string]> = [
    ["inputTokens", t("tokenInput")], ["cachedInputTokens", t("tokenCached")],
    ["cacheWriteInputTokens", t("tokenCacheWrite")], ["outputTokens", t("tokenOutput")],
    ["reasoningOutputTokens", t("tokenReasoning")], ["totalTokens", t("tokenTotal")],
  ];
  const number = (value: number) => new Intl.NumberFormat(formatLocale).format(value);

  return <section className="token-section" aria-labelledby="token-heading">
    <div className="token-heading"><div>
      <p className="eyebrow">{t("tokenEyebrow")}</p>
      <h2 id="token-heading">{t("tokenTitle")}</h2>
      <p>{t("tokenSubtitle")}</p>
    </div></div>
    <form className="token-filters" onSubmit={(event) => {
      event.preventDefault();
      onQuery({ startAt: optionalUtc(startAt), endAt: optionalUtc(endAt), model: model.trim() || undefined, sessionId: sessionId.trim() || undefined });
    }}>
      <label>{t("tokenStart")}<input type="datetime-local" value={startAt} onChange={(event) => setStartAt(event.target.value)} /></label>
      <label>{t("tokenEnd")}<input type="datetime-local" value={endAt} onChange={(event) => setEndAt(event.target.value)} /></label>
      <label>{t("tokenModel")}<input type="search" value={model} onChange={(event) => setModel(event.target.value)} /></label>
      <label>{t("tokenSession")}<input type="search" value={sessionId} onChange={(event) => setSessionId(event.target.value)} /></label>
      <button type="submit">{t("tokenFilter")}</button>
    </form>
    <p className="token-semantics">{t("tokenSemantics")}</p>
    {state.status === "loading" && <div className="token-state" role="status">{t("tokenLoading")}</div>}
    {state.status === "ready" && <div className="token-state" role="status">{t("tokenFresh")} · <time dateTime={state.data.updatedAt}>{new Intl.DateTimeFormat(formatLocale, { dateStyle: "short", timeStyle: "medium" }).format(new Date(state.data.updatedAt))}</time></div>}
    {state.status === "stale" && <div className="error-banner" role="status"><div><strong>{state.reason === "paused" ? t("tokenStalePaused") : t("tokenStaleOutdated")}</strong><time dateTime={state.data.updatedAt}>{new Intl.DateTimeFormat(formatLocale, { dateStyle: "short", timeStyle: "medium" }).format(new Date(state.data.updatedAt))}</time></div></div>}
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
          <table aria-label={t("tokenBySession")}><thead><tr><th>{t("tokenSession")}</th><th>{t("tokenModel")}</th><th>{t("tokenTotal")}</th><th>{t("tokenLastSeen")}</th></tr></thead>
            <tbody>{data.sessions.map((item) => <tr key={`${item.sessionId}-${item.model}`}><td>{item.sessionId}</td><td>{item.model}</td><td>{number(item.counts.totalTokens)}</td><td><time dateTime={item.lastObservedAt}>{new Intl.DateTimeFormat(formatLocale, { dateStyle: "short", timeStyle: "short" }).format(new Date(item.lastObservedAt))}</time></td></tr>)}</tbody></table>
        </div>
      </div>
    </>}
  </section>;
}
