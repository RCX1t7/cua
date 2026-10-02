// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { hostOs } from "../model/host";
import { displayPaths } from "../model/paths";
import { vaultInitial, vaultReduce, vaultView, type VaultAction, type VaultState } from "../model/keyvault";
import { formatBytes, type TeleportPickerController } from "../model/teleportFlow";
import { pickerConsent, pickerReview, reviewVaultSource, type TeleportReviewHost } from "../model/teleportReview";
import type { KeyvaultOverview } from "../native/keyvault";
import { readSetting, writeSetting } from "../state/settings";

const errorText = (error: unknown) => error instanceof Error ? error.message : String(error);
const cookieKey = (key: string) => key.split(/[\\/]/).at(-1)?.toLowerCase() === "cookies";

/** Released shared consent decisions; inventory contains counts, never values. */
export function TeleportReview({ controller, host, home, onClose, autoReview }: {
  controller: TeleportPickerController;
  host: TeleportReviewHost;
  home: string | null;
  onClose: () => void;
  autoReview: boolean;
}) {
  const state = useSyncExternalStore(controller.subscribe, () => controller.state);
  const review = pickerReview(state)!;
  const provider = state.entry?.providerId ?? state.plan?.app.providerId ?? null;
  const key = `cua.teleport.domains.${provider ?? ""}|${state.plan?.spaceId ?? state.spaceName}`;
  const [busy, setBusy] = useState(false);
  const [inventoryError, setInventoryError] = useState<string | null>(null);
  const [inventoryNotes, setInventoryNotes] = useState<string[]>([]);
  const [vaultError, setVaultError] = useState<string | null>(null);
  const [overview, setOverview] = useState<KeyvaultOverview | null>(null);
  const [vault, setVault] = useState<VaultState>(vaultInitial);
  const mounted = useRef(true);
  const browsing = useRef(false);
  const openingPlan = useRef(state.plan?.json);
  const uncheckWholeCookieStore = () => {
    for (const item of pickerReview(controller.state)?.toggles ?? []) {
      if (cookieKey(item.key) && item.selected) controller.dispatch({ type: "toggle-item", key: item.key });
    }
  };

  const loadInventory = async () => {
    if (!provider || busy) return;
    setBusy(true);
    setInventoryError(null);
    setInventoryNotes([]);
    try {
      if (!host.keyvault) throw new Error("Site inventory is unavailable from this host.");
      const inventory = await host.keyvault.inventory(provider);
      if (!mounted.current || controller.state.plan?.json !== openingPlan.current) return;
      let remembered: string[] | null = null;
      try {
        const saved: unknown = JSON.parse(readSetting(key, "null"));
        if (Array.isArray(saved) && saved.every((domain) => typeof domain === "string")) remembered = saved;
      } catch { /* A malformed saved preference cannot widen consent. */ }
      controller.dispatch({ type: "domains-loaded", inventory, remembered });
      setInventoryNotes(inventory.notes);
      if (inventory.domains.length === 0) uncheckWholeCookieStore();
    } catch (error) {
      if (!mounted.current || controller.state.plan?.json !== openingPlan.current) return;
      setInventoryError(errorText(error));
      controller.dispatch({ type: "domains-failed" });
      // No per-site review was possible. Sending the whole cookie store needs
      // its own explicit checkbox selection, never a silent broader fallback.
      uncheckWholeCookieStore();
    } finally { if (mounted.current) setBusy(false); }
  };

  useEffect(() => {
    mounted.current = true;
    let cancelled = false;
    if (provider && host.keyvault) {
      void host.keyvault.overview().then((value) => {
        if (cancelled) return;
        setOverview(value);
        const source = reviewVaultSource(value, provider);
        setVault({ query: "", selected: source.ids, expanded: [provider], app: provider });
        controller.dispatch({ type: "vault-items", count: source.count, newestMs: source.newestMs, nowMs: Date.now(), selected: source.ids, passwordIds: source.passwordIds });
      }).catch((error: unknown) => { if (!cancelled) setVaultError(errorText(error)); });
    }
    // The native path reaches this page when Review is pressed. Demo autoReview
    // waits for the user's explicit Read sites button before requesting inventory.
    if (review.needsDomains && !autoReview) void loadInventory();
    return () => {
      cancelled = true;
      mounted.current = false;
      if (browsing.current) { browsing.current = false; void host.keyvault?.endBrowse().catch(() => {}); }
    };
    // The component mounts once for one prepared plan, not for every choice.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [controller, host, provider]);

  useEffect(() => {
    const hide = () => {
      if (!document.hidden || !browsing.current) return;
      browsing.current = false;
      setOverview((value) => value ? { ...value, namesVisible: false, items: value.items.map((item) => ({ ...item, domain: undefined, key: "", path: undefined })) } : value);
      void host.keyvault?.endBrowse().catch(() => {});
    };
    document.addEventListener("visibilitychange", hide);
    const refresh = window.setInterval(() => {
      if (!browsing.current || !host.keyvault || document.hidden) return;
      void host.keyvault.overview().then((value) => { if (mounted.current) setOverview(value); }).catch((error: unknown) => { if (mounted.current) setVaultError(errorText(error)); });
    }, 5000);
    return () => { document.removeEventListener("visibilitychange", hide); window.clearInterval(refresh); };
  }, [host]);

  useEffect(() => {
    const until = overview?.status?.browse_until_ms;
    if (!overview?.namesVisible || typeof until !== "number" || until <= 0) return;
    const timer = window.setTimeout(() => setOverview((value) => value ? { ...value, namesVisible: false, items: value.items.map((item) => ({ ...item, domain: undefined, key: "", path: undefined })) } : value), Math.max(0, until - Date.now()));
    return () => window.clearTimeout(timer);
  }, [overview?.namesVisible, overview?.status?.browse_until_ms]);

  const savedOverview = overview ? { ...overview, items: overview.items.filter((item) => item.kind !== "password") } : null;
  const saved = savedOverview ? vaultView(savedOverview, vault, Date.now()) : null;
  const sendVault = (action: VaultAction) => {
    if (!savedOverview) return;
    const next = vaultReduce(savedOverview, vault, action);
    setVault(next);
    controller.dispatch({ type: "vault-selection", selected: next.selected });
  };
  const showNames = async () => {
    if (!host.keyvault || busy) return;
    setBusy(true); setVaultError(null);
    try {
      await host.keyvault.browse();
      if (!mounted.current || document.hidden) { void host.keyvault.endBrowse().catch(() => {}); return; }
      browsing.current = true;
      const value = await host.keyvault.overview();
      if (mounted.current) setOverview(value);
    } catch (error) { if (mounted.current) setVaultError(errorText(error)); }
    finally { if (mounted.current) setBusy(false); }
  };
  const confirm = () => {
    if (busy || !review.canConfirm) return;
    const consent = pickerConsent(controller.state);
    if (consent.cookieDomains && consent.fromVault == null) writeSetting(key, JSON.stringify(consent.cookieDomains));
    void controller.confirm();
  };
  const local = hostOs() === "macos" ? "this Mac" : "this computer";

  return <div className="hp-consent ta-consent">
    <header className="hp-consent-head"><h1 className="hp-consent-title">{review.title}?</h1></header>
    <div style={{ flex: 1, minHeight: 0, overflowY: "auto", display: "flex", flexDirection: "column", gap: 12 }}>
    {review.offersVault && <fieldset className="ta-choices"><legend>Send from</legend>
      <label className="ta-choice"><input type="radio" name="source" checked={review.source === "live"} onChange={() => controller.dispatch({ type: "send-from", source: "live" })} />{`Read ${state.plan!.app.name} on ${local}`}</label>
      <label className="ta-choice"><input type="radio" name="source" checked={review.source === "vault"} onChange={() => controller.dispatch({ type: "send-from", source: "vault" })} />{review.vaultLabel}</label>
    </fieldset>}
    {review.source === "vault" ? <section aria-label="Saved in the Keyvault">
      <p className="hp-note">Choose saved items. No live app data is read for this source.</p>
      {saved?.namesHidden ? <><p>{hostOs() === "windows" ? "Item names are hidden until Windows verifies your presence." : saved.hiddenNote}</p><button type="button" className="hp-button" disabled={busy} onClick={() => void showNames()}>{saved.showNamesLabel}</button></> : saved ? <>
        <input type="search" aria-label="Search saved items" placeholder={saved.searchPrompt} value={vault.query} onChange={(event) => sendVault({ type: "query", text: event.target.value })} />
        <p>{saved.selection.count} of {saved.shown} selected</p>
        <button type="button" className="hp-cancel" onClick={() => sendVault({ type: "select-all" })}>All</button> <button type="button" className="hp-cancel" onClick={() => sendVault({ type: "clear" })}>None</button>
        {saved.apps.flatMap((app) => [...app.sites.map((site) => <div key={site.key}>
          <label className="ta-choice"><input type="checkbox" checked={site.selected === "on"} ref={(input) => { if (input) input.indeterminate = site.selected === "mixed"; }} onChange={() => sendVault({ type: "toggle-group", key: site.key })} /><span>{site.site} ({site.counts})</span></label>
          <button type="button" className="hp-cancel" aria-expanded={site.open} onClick={() => sendVault({ type: "toggle-open", key: site.key })}>{site.open ? "Hide items" : "Show items"}</button>
          {site.rows.map((row) => <label key={row.id} className="ta-choice"><input type="checkbox" checked={row.selected} onChange={() => sendVault({ type: "toggle", id: row.id })} /><span>{row.title} — {row.kindLabel}</span></label>)}
        </div>), ...(app.files ? [<div key={app.files.key}><label className="ta-choice"><input type="checkbox" checked={app.files.selected === "on"} onChange={() => sendVault({ type: "toggle-group", key: app.files!.key })} />Saved files ({app.files.count})</label></div>] : [])])}
        {saved.emptyText && <p className="hp-note">{saved.emptyText}</p>}
      </> : <p className="hp-note">Saved item details are unavailable.</p>}
    </section> : <>
      {review.offersDomains && <section aria-label="Sites">
        <h2>Sites</h2>
        {busy ? <p role="status">Reading site counts...</p> : review.needsDomains ? <button className="hp-button" type="button" onClick={() => void loadInventory()}>Read sites</button> : <>
          <input type="search" aria-label="Search sites" value={review.domainQuery} onChange={(event) => controller.dispatch({ type: "domain-query", text: event.target.value })} />
          <p>{review.domainSummary}</p>
          <button className="hp-cancel" type="button" onClick={() => controller.dispatch({ type: "select-shown-domains", value: true })}>All</button> <button className="hp-cancel" type="button" onClick={() => controller.dispatch({ type: "select-shown-domains", value: false })}>None</button>
          {review.domains.map((domain) => <div key={domain.domain}>
            <label className="ta-choice"><input type="checkbox" checked={domain.selected} disabled={!domain.selectable} onChange={() => controller.dispatch({ type: "toggle-domain", domain: domain.domain })} /><span>{domain.domain} — {domain.counts}{domain.identityProvider ? " — Identity provider: its session signs in to other apps" : domain.signin ? " — Signs you in" : ""}</span></label>
            {domain.unavailable > 0 && <p className="hp-note">{domain.unavailableNote}</p>}
          </div>)}
          {review.domains.length === 0 && <p className="hp-note">No sites are available to choose. Review the individual items below.</p>}
        </>}
        {inventoryError && <><p role="alert" className="hp-status-error">Could not read site counts: {inventoryError}</p><button type="button" className="hp-cancel" disabled={busy} onClick={() => void loadInventory()}>Retry site inventory</button></>}
        {inventoryNotes.map((note) => <p key={note} className="hp-note">{note}</p>)}
      </section>}
      <ul className="hp-items" aria-label="What moves" style={{ flex: "none", overflow: "visible" }}>
        {review.items.filter((item) => item.kind === "install").map((item) => <li key={item.key} className="hp-item"><span>{item.label}</span><span>{item.detail}</span></li>)}
        {review.toggles.map((item) => <li key={item.key}><label className="ta-choice" title={displayPaths(item.detail, home)}><input type="checkbox" checked={item.selected} onChange={() => controller.dispatch({ type: "toggle-item", key: item.key })} /><span>{displayPaths(item.label, home)}{item.sensitive ? " — Secret" : item.bytes ? ` — ${formatBytes(item.bytes)}` : ""}</span></label></li>)}
      </ul>
    </>}
    {vaultError && <p className="hp-status-error" role="alert">Saved Keyvault items unavailable: {vaultError}</p>}
    {review.offersPasswords && <label className="ta-choice"><input type="checkbox" checked={review.includePasswords} onChange={(event) => controller.dispatch({ type: "toggle-passwords", value: event.target.checked })} /><span>{review.passwordsLabel} (off by default)</span></label>}
    {review.warnings.map((warning) => <p key={warning} className="hp-note">{warning}</p>)}
    {review.needsAcknowledgement && <label className="ta-choice ta-ack"><input type="checkbox" checked={review.acknowledged} onChange={(event) => controller.dispatch({ type: "acknowledge", value: event.target.checked })} /><span>Send the secrets above</span></label>}
    {review.offersSaveToKeyvault && review.source === "live" && <label className="ta-choice ta-save-to-keyvault"><input type="checkbox" checked={review.saveToKeyvault} onChange={(event) => controller.dispatch({ type: "save-to-keyvault", value: event.target.checked })} /><span>Save to Keyvault for reuse</span></label>}
    {review.needsRelayPlaintextAcknowledgement && <label className="ta-choice ta-ack"><input type="checkbox" checked={review.acknowledgedRelayPlaintext} onChange={(event) => controller.dispatch({ type: "acknowledge-relay-plaintext", value: event.target.checked })} /><span>Send without end-to-end encryption</span></label>}
    </div>
    <footer className="hp-footer"><button type="button" className="hp-cancel" onClick={() => controller.dispatch({ type: "back" })}>Back</button><span className="hp-footer-note">{review.leavesText?.replace("this Mac", local) ?? `Nothing leaves ${local}`}</span><div className="hp-footer-right"><button type="button" className="hp-cancel" onClick={onClose}>Cancel</button><button type="button" className="hp-button hp-button-primary" disabled={busy || review.needsDomains || !review.canConfirm} onClick={confirm}>Teleport</button></div></footer>
  </div>;
}
