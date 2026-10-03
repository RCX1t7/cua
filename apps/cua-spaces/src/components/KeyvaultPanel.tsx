// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { createKeyvaultBridge, type KeyvaultBridge, type KeyvaultOverview, type KvInventory } from "../native/keyvault";
import { approvalCommand, approvalView, kvDeleteConfirm, kvList, kvPage, kvPassphraseCheck, kvRecoveryKeyText, kvSidebar, kvUnlockPrompt, liveDelivery, openApproval, reduceApproval, vaultInitial, vaultReduce, vaultView, type ApprovalState, type KvCommand, type KvSelection, type Tri, type VaultAction, type VaultGroup, type VaultRow } from "../model/keyvault";
import { hostOs } from "../model/host";
import "../styles/keyvault.css";

export interface KeyvaultPanelProps { bridge?: KeyvaultBridge; onClose?: () => void }
const emptyOverview = (): KeyvaultOverview => ({ availability: "not_running", serverVerified: false, items: [], namesVisible: false, itemsTotal: 0, pending: [], grants: [], rules: [], deliveries: [], audit: [], partialErrors: [] });
interface Confirmation { title: string; message: string; label: string; command: KvCommand }
function Check({ value, label, disabled, onChange }: { value: Tri; label: string; disabled?: boolean; onChange: () => void }) {
  const ref = useRef<HTMLInputElement>(null);
  useEffect(() => { if (ref.current) ref.current.indeterminate = value === "mixed"; }, [value]);
  return <input ref={ref} type="checkbox" aria-label={label} checked={value === "on"} disabled={disabled} onChange={onChange} />;
}

/** Actual broker metadata and shared core decisions. There is no item-value/reveal API. */
export function KeyvaultPanel({ bridge: supplied, onClose }: KeyvaultPanelProps) {
  const bridge = useMemo(() => supplied ?? createKeyvaultBridge(), [supplied]);
  const [overview, setOverview] = useState<KeyvaultOverview>(emptyOverview);
  const [loaded, setLoaded] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [selection, setSelection] = useState<KvSelection>({ kind: "category", category: "all" });
  const [vault, setVault] = useState(vaultInitial);
  const [approval, setApproval] = useState<ApprovalState | null>(null);
  const [confirmation, setConfirmation] = useState<Confirmation | null>(null);
  const [passphrase, setPassphrase] = useState("");
  const [confirmPassphrase, setConfirmPassphrase] = useState("");
  const [recoveryKey, setRecoveryKey] = useState<string | null>(null);
  const [inventory, setInventory] = useState<KvInventory | null>(null);
  const [icons, setIcons] = useState<Record<string, string>>({});
  const browsing = useRef(false);
  const mounted = useRef(true);
  const inFlight = useRef(false);
  const generation = useRef(0);
  const now = Date.now();
  const windows = hostOs() === "windows";
  const page = kvPage(overview, now);
  const sidebar = kvSidebar(overview, now);
  const list = kvList(overview, selection, now);
  const state = { ...vault, app: selection.kind === "app" ? selection.key : null };
  const view = vaultView(overview, state, now);
  const sheet = approval ? approvalView(overview, approval) : null;
  const trusted = page.ready && overview.serverVerified && overview.status?.caller_first_party === true;

  const refresh = useCallback(async () => {
    if (inFlight.current) return;
    inFlight.current = true;
    const requestGeneration = generation.current;
    try {
      const value = await bridge.overview();
      if (mounted.current && generation.current === requestGeneration) { setOverview(value); setLoaded(true); }
    } catch (e) {
      if (mounted.current) { setOverview({ ...emptyOverview(), availability: "error", message: String(e) }); setError(String(e)); setLoaded(true); }
    } finally { inFlight.current = false; }
  }, [bridge]);
  const hideNames = useCallback(() => {
    generation.current += 1;
    setOverview((value) => ({ ...value, namesVisible: false, items: value.items.map((item) => ({ ...item, domain: undefined, key: "", path: undefined })) }));
    setIcons({}); setInventory(null);
  }, []);
  useEffect(() => {
    mounted.current = true;
    void refresh();
    const timer = window.setInterval(() => { if (!document.hidden) void refresh(); }, 5000);
    const visibility = () => {
      if (document.hidden && browsing.current) {
        browsing.current = false; hideNames();
        void bridge.endBrowse().catch((e: unknown) => { if (mounted.current) setError(String(e)); });
      } else if (!document.hidden) void refresh();
    };
    document.addEventListener("visibilitychange", visibility);
    return () => {
      mounted.current = false;
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", visibility);
      if (browsing.current) { browsing.current = false; void bridge.endBrowse().catch(() => {}); }
    };
  }, [bridge, hideNames, refresh]);
  useEffect(() => {
    if (!overview.namesVisible) { setIcons({}); return; }
    let cancelled = false;
    void bridge.favicons().then((rows) => { if (!cancelled) setIcons(Object.fromEntries(rows.map((row) => [row.site, `data:image/png;base64,${row.png}`]))); }).catch((e: unknown) => { if (!cancelled) setError(String(e)); });
    return () => { cancelled = true; };
  }, [bridge, overview.namesVisible]);
  const act = async (run: () => Promise<unknown>) => {
    if (busy) return;
    setBusy(true); setError(null);
    try { await run(); await refresh(); }
    catch (e) { if (mounted.current) setError(String(e)); }
    finally { if (mounted.current) setBusy(false); }
  };
  const execute = async (command: KvCommand): Promise<unknown> => {
    switch (command.type) {
      case "setup": { const key = await bridge.setup(); setRecoveryKey(key); return; }
      case "unlock": return bridge.unlock();
      case "browse": { await bridge.browse(); browsing.current = true; return; }
      case "end-browse": browsing.current = false; hideNames(); return bridge.endBrowse();
      case "set-disabled": return bridge.setDisabled(command.disabled);
      case "set-unattended": return bridge.setUnattended(command.itemIds, command.unattended);
      case "set-locked": return bridge.setLocked(command.itemIds, command.locked);
      case "delete-items": return bridge.deleteItems(command.itemIds);
      case "set-auto-wipe": return bridge.setAutoWipe(command.on);
      case "set-skip-unlock-prompt": return bridge.setSkipUnlockPrompt(command.on);
      case "revoke-grant": return bridge.revokeGrant(command.id);
      case "remove-rule": return bridge.removeRule(command.id);
      case "release": return bridge.release(command.target);
      case "approve": return bridge.approve(command.requestId, command.items);
      case "deny": return bridge.deny(command.requestId);
    }
  };
  const send = (command: KvCommand) => void act(() => execute(command));
  const reduce = (action: VaultAction) => setVault((previous) => vaultReduce(overview, { ...previous, app: selection.kind === "app" ? selection.key : null }, action));
  const changeLock = (itemIds: string[], locked: boolean) => {
    if (!itemIds.length) return;
    const command: KvCommand = { type: "set-locked", itemIds, locked };
    const prompt = !locked ? kvUnlockPrompt(overview, itemIds.length) : null;
    if (prompt) setConfirmation({ title: prompt.title, message: `${prompt.subject}. ${prompt.message}`, label: prompt.allow, command });
    else send(command);
  };
  const removeSelected = () => {
    const ids = view.selection.ids;
    const liveCopies = new Set(overview.deliveries.filter((delivery) => liveDelivery(delivery, now) && delivery.items.some((id) => ids.includes(id))).map((delivery) => delivery.target)).size;
    const prompt = kvDeleteConfirm(ids.length, liveCopies);
    setConfirmation({ title: prompt.title, message: prompt.message, label: prompt.confirm, command: { type: "delete-items", itemIds: ids } });
  };
  const group = (value: VaultGroup, title: string, subtitle?: string) => <div className="kv-group" key={value.key}>
    <div className="kv-group-heading">
      <Check value={value.selected} label={`Select ${title}`} disabled={!trusted || busy} onChange={() => reduce({ type: "toggle-group", key: value.key })} />
      <button className="kv-expand" aria-expanded={value.open} onClick={() => reduce({ type: "toggle-open", key: value.key })}>{value.open ? "▾" : "▸"} {icons[title] && <img src={icons[title]} alt="" />} {title}</button>
      <span className="kv-muted">{subtitle ?? `${value.count} items`}</span>
      <button disabled={!trusted || busy || !(value.lockIds.length || value.unlockIds.length)} onClick={() => changeLock(value.lock === "unlocked" ? value.lockIds : value.unlockIds, value.lock === "unlocked")} title={value.lock === "unlocked" ? "Require approval for each use" : "Allow unattended access"}>{value.lock === "locked" ? "Locked" : value.lock === "mixed" ? "Mixed" : "Unlocked"}</button>
    </div>
    {value.rows.map(renderRow)}
  </div>;
  function renderRow(row: VaultRow) {
    return <div className="kv-item" key={row.id}>
      <input type="checkbox" aria-label={`Select ${row.title || row.kindLabel}`} checked={row.selected} disabled={!trusted || busy} onChange={() => reduce({ type: "toggle", id: row.id })} />
      <div className="kv-item-name"><strong>{row.title || "Name hidden"}</strong><small>{row.kindLabel} · {row.subtitle}</small></div>
      <small className="kv-muted">{row.updated}</small>
      <button title={row.lockHelp} aria-label={row.lockHelp} disabled={!trusted || busy || row.identityProvider} onClick={() => changeLock([row.id], !row.locked)}>{row.identityProvider ? "Always asks" : row.locked ? "Locked" : "Unlocked"}</button>
    </div>;
  }
  const credentialSubmit = async () => {
    const form = page.form;
    if (!form) return;
    const secret = passphrase;
    setPassphrase(""); setConfirmPassphrase("");
    await act(async () => {
      if (form.mode === "setup") setRecoveryKey(form.method === "passphrase" ? await bridge.setupWithPassphrase(secret) : await bridge.setup());
      else if (form.method === "passphrase") await bridge.unlockWithPassphrase(secret);
      else await bridge.unlock();
    });
  };
  useEffect(() => {
    const expires = overview.status?.browse_until_ms;
    if (!overview.namesVisible || !expires) return;
    const timer = window.setTimeout(() => { browsing.current = false; hideNames(); void refresh(); }, Math.max(0, Math.min(expires - Date.now(), 2147483647)));
    return () => window.clearTimeout(timer);
  }, [hideNames, overview.namesVisible, overview.status?.browse_until_ms, refresh]);
  const check = page.form?.method === "passphrase" ? kvPassphraseCheck(page.form.mode, passphrase, confirmPassphrase) : null;

  return <section className="keyvault-panel" aria-label="Keyvault">
    <header className="kv-header"><div><h1>Keyvault</h1><p>Control which credentials can be delivered to your Spaces.</p></div><button disabled={busy} onClick={() => void refresh()}>Refresh</button>{onClose && <button aria-label="Close Keyvault" onClick={() => { hideNames(); if (browsing.current) { browsing.current = false; void bridge.endBrowse().catch(() => {}); } onClose(); }}>Close</button>}</header>
    <div className="kv-status" role="status">{!loaded ? "Connecting to the Cua daemon…" : `Broker: ${overview.availability} · ${overview.serverVerified ? "signature verified" : "signature not verified"}`}</div>
    {error && <div className="kv-banner kv-danger" role="alert">{error}</div>}
    {page.partialErrors.map((message, index) => <div className="kv-banner kv-danger" role="alert" key={index}>{message}</div>)}
    {page.disabledBanner && <div className="kv-banner">{page.disabledBanner}</div>}
    {page.resetNotice && <div className="kv-banner">{page.resetNotice}</div>}
    {recoveryKey && <div className="kv-banner kv-recovery"><strong>{kvRecoveryKeyText(recoveryKey)}</strong><p>Save this recovery key somewhere private. It is not stored in this page.</p><button onClick={() => setRecoveryKey(null)}>I saved it</button></div>}
    {!page.ready && loaded && <div className="kv-unavailable"><h2>{page.unavailableTitle}</h2><p>{page.message}</p>{page.form && <form onSubmit={(event) => { event.preventDefault(); void credentialSubmit(); }}>
      <p>{windows && page.form.method !== "passphrase" ? "The Cua daemon requests Windows account verification. Your vault key remains in the Windows account credential store." : page.form.help}</p>
      {page.form.passphraseLabel && <label>{page.form.passphraseLabel}<input type="password" value={passphrase} autoComplete={page.form.mode === "setup" ? "new-password" : "current-password"} onChange={(event) => setPassphrase(event.target.value)} /></label>}
      {page.form.confirmLabel && <label>{page.form.confirmLabel}<input type="password" value={confirmPassphrase} autoComplete="new-password" onChange={(event) => setConfirmPassphrase(event.target.value)} /></label>}
      {check?.hint && <p>{check.hint}</p>}
      <button type="submit" disabled={busy || (check !== null && !check.canSubmit)}>{page.form.submitLabel}</button>
    </form>}</div>}
    {page.ready && <div className="kv-layout">
      <nav className="kv-sidebar" aria-label="Keyvault categories">{sidebar.categories.map((row) => <button key={row.category} aria-current={selection.kind === "category" && selection.category === row.category ? "page" : undefined} onClick={() => setSelection({ kind: "category", category: row.category })}>{row.title}<span>{row.count ?? ""}</span></button>)}<hr />{sidebar.apps.map((row) => <button key={row.key} aria-current={selection.kind === "app" && selection.key === row.key ? "page" : undefined} onClick={() => setSelection({ kind: "app", key: row.key })}>{row.title}<span>{row.items}</span></button>)}</nav>
      <main className="kv-content"><h2>{list.title}</h2>
        {list.vault && <>
          {view.namesHidden && <div className="kv-banner"><p>{windows ? "Item names stay hidden until the broker verifies your Windows account presence." : view.hiddenNote}</p><button disabled={!trusted || busy} onClick={() => send({ type: "browse" })}>{view.showNamesLabel}</button></div>}
          {!view.namesHidden && <div className="kv-toolbar"><input type="search" aria-label="Search Keyvault" placeholder={view.searchPrompt} value={vault.query} onChange={(event) => reduce({ type: "query", text: event.target.value })} /><button disabled={busy} onClick={() => send({ type: "end-browse" })}>Hide names</button></div>}
          <div className="kv-toolbar"><span>{view.selection.count ? view.selection.title : `${view.shown} of ${view.total} items`}</span><button disabled={!trusted || busy || !view.canSelectAll} onClick={() => reduce({ type: "select-all" })}>Select all</button><button disabled={!view.selection.count} onClick={() => reduce({ type: "clear" })}>Clear</button>{view.selection.count > 0 && <><button disabled={!trusted || busy || !view.selection.canLock} onClick={() => changeLock(view.selection.lockIds, true)}>Lock</button><button disabled={!trusted || busy || !view.selection.canUnlock} onClick={() => changeLock(view.selection.unlockIds, false)}>Allow unattended</button><button className="kv-delete" disabled={!trusted || busy} onClick={removeSelected}>Delete</button></>}</div>
          {view.selection.alwaysAsk > 0 && <p className="kv-muted">{view.selection.alwaysAsk} selected identity-provider items always require approval.</p>}
          {view.emptyText && <p>{view.emptyText}</p>}
          {view.apps.map((app) => <section className="kv-app" key={app.key}><div className="kv-group-heading"><Check value={app.selected} label={`Select ${app.name}`} disabled={!trusted || busy} onChange={() => reduce({ type: "toggle-group", key: app.key })} /><button className="kv-expand" aria-expanded={app.open} onClick={() => reduce({ type: "toggle-open", key: app.key })}>{app.open ? "▾" : "▸"} {app.name}</button><span className="kv-muted">{app.summary}</span><button disabled={!trusted || busy || !(app.unlockIds.length || app.lockIds.length)} onClick={() => changeLock(app.lock === "unlocked" ? app.lockIds : app.unlockIds, app.lock === "unlocked")}>{app.lock === "locked" ? "Locked" : app.lock === "mixed" ? "Mixed" : "Unlocked"}</button></div>{app.open && <>{app.sites.map((site) => group(site, site.site, site.counts))}{app.files && group(app.files, "Files")}</>}</section>)}
          {selection.kind === "app" && <button disabled={!trusted || busy} onClick={() => void act(async () => setInventory(await bridge.inventory(selection.key)))}>Inspect available domain counts</button>}
          {inventory && <section className="kv-inventory"><h3>{inventory.app_display}: available metadata</h3>{inventory.notes.map((note) => <p key={note}>{note}</p>)}{inventory.domains.map((domain) => <p key={domain.domain}>{domain.domain}: {domain.cookies} cookies, {domain.local_storage} storage values, {domain.passwords} passwords{domain.identity_provider ? " · always asks" : ""}{domain.unavailable > 0 ? ` · ${domain.unavailable} unavailable: ${domain.unavailable_reason}` : ""}</p>)}</section>}
        </>}
        {list.emptyText && <p>{list.emptyText}</p>}
        {list.pending.map((row) => <article className="kv-record" key={row.id}><div><strong>{row.caller}</strong><span className={`kv-badge ${row.badge.tone}`}>{row.badge.text}</span><p>{row.summary}</p><p>{row.wants}</p>{row.claims.map((claim) => <p key={claim}>{claim}</p>)}</div><button disabled={!trusted || busy} onClick={() => setApproval(openApproval(row.id))}>{page.labels.review}</button><button disabled={!trusted || busy} onClick={() => send({ type: "deny", requestId: row.id })}>{page.labels.deny}</button></article>)}
        {list.access.map((row) => <article className="kv-record" key={row.key}><div><strong>{row.text}</strong><p>{row.detail}</p></div><button disabled={!trusted || busy} onClick={() => row.kind === "delivery" ? setConfirmation({ title: "Wipe delivered credentials?", message: `Remove the recorded credential copies from ${row.text}.`, label: row.actionLabel, command: row.command }) : send(row.command)}>{row.actionLabel}</button></article>)}
        {list.recent.map((row) => <article className="kv-record" key={row.decision.entry.seq}><div><strong>{row.decision.verb}</strong><p>{row.decision.what}</p><small>{row.decision.entry.actor} · {row.decision.entry.decision}</small></div><small>{row.age}</small></article>)}
        {page.revokeAll && list.access.some((row) => row.kind === "grant") && <button disabled={!trusted || busy} onClick={() => void act(async () => { for (const row of list.access.filter((row) => row.kind === "grant")) await execute(row.command); })}>{page.labels.revokeAll}</button>}
        {page.logStatus && <p className={page.logTampered ? "kv-danger" : "kv-muted"}>{page.logStatus}</p>}
      </main>
    </div>}
    {overview.status && <footer className="kv-protection"><h3>Protection</h3><p>This app: {overview.status.caller_first_party ? "verified by the broker" : "not verified"}. Cua daemon: {overview.serverVerified ? "signature verified" : "not verified"}.</p><p>{windows ? "Windows account credential protection and broker presence verification" : "OS credential protection and broker presence verification"}</p>{page.killSwitchVisible && <label><input type="checkbox" checked={page.disabled} disabled={busy || !page.killSwitchEnabled} onChange={(event) => send({ type: "set-disabled", disabled: event.target.checked })} /> Disable Keyvault and revoke access</label>}{trusted && <><label><input type="checkbox" checked={overview.status.auto_wipe ?? true} disabled={busy} onChange={(event) => send({ type: "set-auto-wipe", on: event.target.checked })} /> Automatically wipe delivered credentials when a Space disconnects</label><label><input type="checkbox" checked={!page.skipUnlockPrompt} disabled={busy} onChange={(event) => send({ type: "set-skip-unlock-prompt", on: !event.target.checked })} /> Show the unattended-access explanation before account verification</label><button disabled={busy} onClick={() => void act(async () => { hideNames(); browsing.current = false; await bridge.lock(); })}>Lock vault</button></>}</footer>}
    {sheet && approval && <div className="kv-modal-backdrop"><section className="kv-modal" role="dialog" aria-modal="true" aria-label={sheet.title}><h2>{sheet.title}</h2><p>{sheet.caller} <span className={`kv-badge ${sheet.badge.tone}`}>{sheet.badge.text}</span></p><p>To: {sheet.targets}</p><p>{sheet.wants}</p><p>{sheet.summary}</p>{sheet.claims.map((claim) => <p key={claim}>{claim}</p>)}{sheet.rows.map((row) => <label className="kv-approval-row" key={row.key}><input type="checkbox" checked={row.selected} disabled={!trusted || busy || sheet.gone} onChange={() => setApproval(reduceApproval(overview, approval, { type: "toggle", key: row.key }))} /><span><strong>{row.title}</strong><small>{row.account}</small></span></label>)}<p>{windows ? "Approval is confirmed by the Cua daemon using your Windows account." : page.labels.confirmNote}</p>{sheet.blockedReason && <p role="alert">{sheet.blockedReason}</p>}<div className="kv-toolbar"><button disabled={busy} onClick={() => setApproval(null)}>Cancel</button><button disabled={!trusted || busy || sheet.gone} onClick={() => void act(async () => { await bridge.deny(approval.requestId); setApproval(null); })}>Deny</button><button disabled={!trusted || busy || !sheet.canApprove} onClick={() => { const command = approvalCommand(overview, approval); if (command) void act(async () => { await execute(command); setApproval(null); }); }}>{sheet.approveLabel}</button></div></section></div>}
    {confirmation && <div className="kv-modal-backdrop"><section className="kv-modal" role="dialog" aria-modal="true" aria-label={confirmation.title}><h2>{confirmation.title}</h2><p>{confirmation.message}</p><div className="kv-toolbar"><button disabled={busy} onClick={() => setConfirmation(null)}>Cancel</button><button disabled={!trusted || busy} onClick={() => void act(async () => { await execute(confirmation.command); setConfirmation(null); })}>{confirmation.label}</button></div></section></div>}
  </section>;
}
