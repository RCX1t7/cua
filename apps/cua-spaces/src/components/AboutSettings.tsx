// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useEffect, useState, useSyncExternalStore } from "react";
import { core } from "../core";
import { hostOs } from "../model/host";
import { hasTauri } from "../native/bridge";
import { checkForUpdates, installAvailableUpdate, refreshUpdatedAgents, setUpdatePreferences, subscribeUpdates, updateState, type UpdateChannel } from "../native/updater";
import { version as buildVersion } from "../../package.json";
import appLicense from "../../LICENSE?raw";

interface AboutView {
  title: string;
  versionLine: string;
  copyright: string;
  links: Array<{ id: string; label: string; url: string | null }>;
  updates: {
    autoCheckLabel: string; autoCheck: boolean;
    autoInstallLabel: string; autoInstall: boolean; autoInstallEnabled: boolean;
    channelLabel: string; channelHelp: string;
    channels: Array<{ id: string; label: string; active: boolean }>;
    checkLabel: string; checkEnabled: boolean; lastCheck: string;
  };
}

/** The same released shared About model as SwiftUI, with Tauri update state. */
export function AboutSettings({ onOpenExternal }: { onOpenExternal: (url: string) => void }) {
  const state = useSyncExternalStore(subscribeUpdates, updateState, updateState);
  const [version, setVersion] = useState(buildVersion);
  const [notices, setNotices] = useState(false);
  useEffect(() => {
    if (!hasTauri()) return;
    let cancelled = false;
    void import("@tauri-apps/api/app").then(({ getVersion }) => getVersion())
      .then((v) => { if (!cancelled) setVersion(v); }).catch(() => {});
    return () => { cancelled = true; };
  }, []);
  const busy = state.phase === "checking" || state.phase === "installing";
  const view = core<AboutView>("about.view", { input: {
    platform: hostOs(), version, build: "", os: "", updater: true,
    autoCheck: state.autoCheck, autoInstall: state.autoInstall, channel: state.channel,
    lastCheck: state.lastCheck ? new Date(state.lastCheck).toLocaleString() : null,
    checking: busy,
  } });
  const updates = view.updates;

  return (
    <section className="st-group" aria-label="About">
      <div className="st-head"><h3 className="st-title">About</h3></div>
      <div className="st-rows">
        <div className="st-row"><span className="st-label">{view.title}</span><span className="st-value">{view.versionLine}</span></div>
        <div className="st-row">
          {view.links.map((link) => (
            <button key={link.id} type="button" className="dw-link" data-owns-enter onClick={() => link.url ? onOpenExternal(link.url) : setNotices(!notices)}>{link.label}</button>
          ))}
        </div>
        {notices && (
          <div className="st-note">
            <p>Cua Spaces uses the Functional Source License, Version 1.1, with the MIT future license. Upstream attribution is preserved in this Windows port.</p>
            <button type="button" className="dw-link" data-owns-enter onClick={() => onOpenExternal("https://github.com/trycua/cua/blob/cua-spaces-v0.3.0/LICENSING.md")}>Component licenses and third-party notices</button>
            <pre style={{ whiteSpace: "pre-wrap", maxHeight: 280, overflow: "auto" }}>{appLicense}</pre>
          </div>
        )}
        <p className="st-note">{view.copyright}</p>
        {!state.configured && <p className="st-note" role="status">This build has no configured signed update feed. Install a newer Windows port manually; automatic updates are disabled.</p>}
        <div className="st-row">
          <span className="st-label">{updates.autoCheckLabel}</span>
          <button type="button" className="kv-switch" role="switch" data-owns-enter aria-label={updates.autoCheckLabel} aria-checked={state.autoCheck} disabled={!state.configured || busy} onClick={() => setUpdatePreferences({ autoCheck: !state.autoCheck })} />
        </div>
        <div className="st-row">
          <span className="st-label">{updates.autoInstallLabel}</span>
          <button type="button" className="kv-switch" role="switch" data-owns-enter aria-label={updates.autoInstallLabel} aria-checked={state.autoInstall} disabled={!state.configured || !updates.autoInstallEnabled || busy} onClick={() => setUpdatePreferences({ autoInstall: !state.autoInstall })} />
        </div>
        <div className="st-row" title={updates.channelHelp}>
          <span className="st-label">{updates.channelLabel}</span>
          <div className="st-segmented" role="radiogroup" aria-label="Update channel">
            {updates.channels.map((channel) => <button key={channel.id} type="button" role="radio" data-owns-enter aria-checked={channel.active} data-active={channel.active} disabled={!state.configured || busy || (channel.id === "beta" && !state.betaSupported)} onClick={() => setUpdatePreferences({ channel: channel.id as UpdateChannel })}>{channel.label}</button>)}
          </div>
        </div>
        {state.configured && !state.betaSupported && <p className="st-note">This update feed supports Stable releases only.</p>}
        <div className="st-row">
          <span className="st-label">{updates.lastCheck}</span>
          <button type="button" className="dw-btn" data-owns-enter disabled={!state.configured || !updates.checkEnabled} onClick={() => void checkForUpdates()}>{state.phase === "installing" ? "Installing…" : updates.checkLabel}</button>
        </div>
        <div aria-live="polite">
          {state.phase === "current" && <p className="st-note">You’re up to date on {state.channel === "beta" ? "Beta" : "Stable"}.</p>}
          {state.version && state.phase === "available" && <div className="st-row"><span className="st-label">Version {state.version} is available.</span><button type="button" className="dw-btn" data-owns-enter onClick={() => void installAvailableUpdate()}>Install and restart</button></div>}
          {state.notes && state.phase === "available" && <p className="st-note" style={{ whiteSpace: "pre-wrap" }}>{state.notes}</p>}
          {state.phase === "installing" && <p className="st-note">Downloading and installing{state.progress === null ? "…" : `: ${state.progress}%`}. The app will restart.</p>}
          {state.phase === "installed" && <p className="st-note">Update installed. Restarting…</p>}
          {state.error && <p className="st-error" role="alert">{state.error}</p>}
        </div>
        {state.refreshPending && <div className="st-row"><span className="st-label">Refresh existing Cua agent integrations after this update</span><button type="button" className="dw-btn" data-owns-enter disabled={state.refreshingAgents} onClick={() => void refreshUpdatedAgents()}>{state.refreshingAgents ? "Refreshing…" : "Retry refresh"}</button></div>}
        {state.refreshError && <p className="st-error" role="alert">The app updated, but agent refresh failed: {state.refreshError}</p>}
      </div>
    </section>
  );
}
