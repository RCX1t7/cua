// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { Update } from "@tauri-apps/plugin-updater";
import { hasTauri } from "./bridge";
import { telemetryBridge, type TelemetryBridge } from "./telemetry";
import { readJson, writeJson } from "../state/settings";

export type UpdateChannel = "stable" | "beta";
interface UpdatePreferences {
  autoCheck: boolean;
  autoInstall: boolean;
  channel: UpdateChannel;
}
export interface UpdateState extends UpdatePreferences {
  configured: boolean;
  betaSupported: boolean;
  phase: "idle" | "checking" | "available" | "current" | "installing" | "installed" | "failed";
  version: string | null;
  notes: string | null;
  lastCheck: string | null;
  progress: number | null;
  error: string | null;
  refreshPending: boolean;
  refreshingAgents: boolean;
  refreshError: string | null;
}

const PREFS = "cua.settings.updates";
const LAST_CHECK = "cua.settings.updates.lastCheck";
const REFRESH = "cua.settings.updates.refreshAfterVersion";
// A distributor must BOTH configure a signed Tauri updater endpoint/key and
// declare its supported channels. An empty declaration disables all checks,
// even if a developer accidentally builds without the fork's config overlay.
// Dynamic feeds receive X-Cua-Update-Channel; a static feed declares stable
// only. Beta must include stable releases as well as prereleases, and must
// never downgrade a newer beta when the user goes back to Stable.
const declaredChannels = String(import.meta.env.VITE_CUA_UPDATE_CHANNELS ?? "")
  .split(",").map((s) => s.trim());
const configured = hasTauri() && declaredChannels.includes("stable");
const betaSupported = configured && declaredChannels.includes("beta");
const stored = readJson<Partial<UpdatePreferences>>(PREFS, {});
let state: UpdateState = {
  autoCheck: stored.autoCheck !== false,
  autoInstall: stored.autoInstall === true,
  channel: betaSupported && stored.channel === "beta" ? "beta" : "stable",
  configured, betaSupported, phase: "idle", version: null, notes: null,
  lastCheck: readJson<string | null>(LAST_CHECK, null), progress: null, error: null,
  refreshPending: false, refreshingAgents: false, refreshError: null,
};
let available: Update | null = null;
let timer: ReturnType<typeof setInterval> | null = null;
let started = false;
const listeners = new Set<() => void>();

export function updateState(): UpdateState { return state; }
export function subscribeUpdates(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}
function publish(patch: Partial<UpdateState>): void {
  state = { ...state, ...patch };
  for (const listener of listeners) listener();
}
function busy(): boolean { return state.phase === "checking" || state.phase === "installing"; }
function record(action: "checked" | "found" | "not_found" | "installed" | "failed", trigger: "user" | "background", telemetry: TelemetryBridge): void {
  telemetry.recordSignals([{ type: "app-update", action, channel: state.channel, trigger }]);
}

export function setUpdatePreferences(patch: Partial<UpdatePreferences>): void {
  if (busy()) return;
  const channel = patch.channel === "beta" && !betaSupported ? "stable" : patch.channel;
  if (channel && channel !== state.channel) {
    void available?.close().catch(() => {});
    available = null;
    publish({ phase: "idle", version: null, notes: null, error: null });
  }
  publish({ ...patch, ...(channel ? { channel } : {}) });
  writeJson(PREFS, { autoCheck: state.autoCheck, autoInstall: state.autoInstall, channel: state.channel });
}

/** Check only: a manual check never installs an update on its own. */
export async function checkForUpdates(trigger: "user" | "background" = "user", telemetry: TelemetryBridge = telemetryBridge()): Promise<void> {
  if (!configured || busy()) return;
  publish({ phase: "checking", error: null, progress: null });
  try {
    await available?.close();
    available = null;
    const { check } = await import("@tauri-apps/plugin-updater");
    const result = await check({ timeout: 30_000, headers: { "X-Cua-Update-Channel": state.channel }, allowDowngrades: false });
    const lastCheck = new Date().toISOString();
    writeJson(LAST_CHECK, lastCheck);
    available = result;
    publish({ lastCheck, phase: result ? "available" : "current", version: result?.version ?? null, notes: result?.body ?? null });
    record("checked", trigger, telemetry);
    record(result ? "found" : "not_found", trigger, telemetry);
    if (result && trigger === "background" && state.autoCheck && state.autoInstall) {
      await installAvailableUpdate(trigger, telemetry);
    }
  } catch (error) {
    publish({ phase: "failed", error: error instanceof Error ? error.message : String(error) });
    record("failed", trigger, telemetry);
  }
}

/** Persist before Windows' installer exits this process. */
async function persistRefreshVersion(version: string | null): Promise<void> {
  writeJson(REFRESH, version);
  if (hasTauri()) {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("ui_storage_set", { key: REFRESH, value: JSON.stringify(version) });
  }
}

export async function installAvailableUpdate(trigger: "user" | "background" = "user", telemetry: TelemetryBridge = telemetryBridge()): Promise<void> {
  if (!configured || !available || busy()) return;
  const update = available;
  publish({ phase: "installing", progress: null, error: null });
  try {
    let downloaded = 0;
    let total = 0;
    await update.download((event) => {
      if (event.event === "Started") total = event.data.contentLength ?? 0;
      if (event.event === "Progress") downloaded += event.data.chunkLength;
      if (event.event === "Finished") publish({ progress: 100 });
      else if (total > 0) publish({ progress: Math.min(100, Math.round(downloaded * 100 / total)) });
    }, { timeout: 120_000 });
    await persistRefreshVersion(update.version);
    await update.install();
    publish({ phase: "installed" });
    record("installed", trigger, telemetry);
    // Windows install exits/restarts the app; on other platforms relaunch it.
    const { relaunch } = await import("@tauri-apps/plugin-process");
    await relaunch();
  } catch (error) {
    // Keep the marker if install succeeded but relaunch failed: a later
    // matching-version launch can still refresh its existing agent skills.
    publish({ phase: "failed", error: error instanceof Error ? error.message : String(error) });
    record("failed", trigger, telemetry);
  }
}

/** Existing engine's `cua agents update`: never a fresh agent setup. */
export async function refreshUpdatedAgents(): Promise<void> {
  if (!hasTauri() || state.refreshingAgents || !state.refreshPending) return;
  publish({ refreshingAgents: true, refreshError: null });
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("agent_setup_update");
    await persistRefreshVersion(null);
    publish({ refreshPending: false });
  } catch (error) {
    publish({ refreshError: error instanceof Error ? error.message : String(error) });
  } finally {
    publish({ refreshingAgents: false });
  }
}

/** Called once by the main app surface; never by secondary viewer windows. */
export async function checkForUpdateSilently(telemetry: TelemetryBridge = telemetryBridge()): Promise<void> {
  if (started || !hasTauri()) return;
  started = true;
  const pending = readJson<string | null>(REFRESH, null);
  if (pending) {
    try {
      const { getVersion } = await import("@tauri-apps/api/app");
      if (await getVersion() === pending) {
        publish({ refreshPending: true });
        await refreshUpdatedAgents();
      }
    } catch (error) {
      publish({ refreshError: error instanceof Error ? error.message : String(error) });
    }
  }
  if (!configured) return;
  if (state.autoCheck) await checkForUpdates("background", telemetry);
  timer ??= setInterval(() => {
    if (state.autoCheck) void checkForUpdates("background", telemetry);
  }, 24 * 60 * 60 * 1000);
}
