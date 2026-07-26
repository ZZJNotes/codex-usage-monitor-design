import { useEffect, useState } from "react";

import { monitorApi } from "../api";
import { translator } from "../i18n";
import type { CredentialDeletionStatus, TokenAccount } from "../types";

type Props = {
  locale: "zh-CN" | "en";
  retentionDays: number;
  accounts: TokenAccount[];
  onRetentionChange: (days: number) => Promise<void>;
  onHistoryChanged: () => Promise<void>;
};

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

export function DataGovernanceSection({
  locale,
  retentionDays,
  accounts,
  onRetentionChange,
  onHistoryChanged,
}: Props) {
  const t = translator(locale);
  const [credentialDeletion, setCredentialDeletion] = useState<CredentialDeletionStatus>({
    status: "unavailable",
    reason: "keychainIntegrationUnavailable",
  });
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [accountKey, setAccountKey] = useState("");

  useEffect(() => {
    monitorApi.getCredentialDeletionStatus().then(setCredentialDeletion).catch((reason) => {
      setError(errorMessage(reason));
    });
  }, []);

  async function run(action: Promise<unknown>) {
    try {
      await action;
      await onHistoryChanged();
      setMessage(t("governanceDone"));
      setError(null);
    } catch (reason) {
      setError(errorMessage(reason));
    }
  }

  async function download(format: "json" | "csv") {
    try {
      const artifact = await monitorApi.exportStatistics(format);
      const url = URL.createObjectURL(new Blob([artifact.content], { type: artifact.mimeType }));
      const link = document.createElement("a");
      link.href = url;
      link.download = artifact.filename;
      link.click();
      URL.revokeObjectURL(url);
      setMessage(t("governanceDone"));
      setError(null);
    } catch (reason) {
      setError(errorMessage(reason));
    }
  }

  return (
    <section className="governance-card" aria-labelledby="governance-heading">
      <div className="governance-heading">
        <div>
          <p className="eyebrow">{t("localOnly")}</p>
          <h2 id="governance-heading">{t("dataPrivacy")}</h2>
          <p>{t("dataPrivacyHelp")}</p>
        </div>
        <div className="governance-actions">
          <button type="button" onClick={() => void download("json")}>{t("exportJson")}</button>
          <button type="button" onClick={() => void download("csv")}>{t("exportCsv")}</button>
        </div>
      </div>
      <div className="governance-grid">
        <label>{t("retention")}
          <select value={retentionDays} onChange={(event) => void onRetentionChange(Number(event.target.value))}>
            {[7, 30, 90, 180, 365].map((days) => <option key={days} value={days}>{t("retentionDays", { days: String(days) })}</option>)}
          </select>
        </label>
        <button type="button" onClick={() => void run(monitorApi.cleanupExpiredHistory())}>{t("cleanupExpired")}</button>
        <button className="danger-button" type="button" onClick={() => {
          if (window.confirm(t("governanceConfirmClear"))) void run(monitorApi.clearHistory());
        }}>{t("clearAllHistory")}</button>
      </div>
      <div className="governance-grid">
        <label>{t("accountHistory")}
          <select value={accountKey} onChange={(event) => setAccountKey(event.target.value)}>
            <option value="">{t("chooseAccount")}</option>
            {accounts.map((account) => <option key={account.accountKey} value={account.accountKey}>{account.displayName}</option>)}
          </select>
        </label>
        <button type="button" disabled={!accountKey} onClick={() => {
          if (accountKey && window.confirm(t("governanceConfirmAccount"))) void run(monitorApi.deleteAccountHistory(accountKey));
        }}>{t("deleteAccountHistory")}</button>
        <button type="button" disabled={credentialDeletion.status === "unavailable"}>{t("deleteCredentials")}</button>
      </div>
      {credentialDeletion.status === "unavailable" && <p className="governance-note" role="status">{t("credentialUnavailable")}</p>}
      <p className="governance-note">{t("checkpointRetentionHelp")}</p>
      {message && <p className="governance-note" role="status">{message}</p>}
      {error && <p className="error-banner" role="alert">{error}</p>}
    </section>
  );
}
