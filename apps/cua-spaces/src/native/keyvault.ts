// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { hasTauri } from "./bridge";

/**
 * The Keyvault page's view of the broker that `cua daemon` hosts on
 * `$CUA_HOME/keyvault.sock` (the shell's `keyvault_*` commands in
 * `src-tauri/src/keyvault.rs`). Shapes mirror `cua_keyvault`'s serde types
 * (snake_case); the overview wrapper is camelCase. Values are never here:
 * the broker's `ListItems` is the redacted view and there is no "reveal".
 */

export type KvItemKind = "cookie" | "local_storage" | "password" | "file";

export interface KvItemPolicy {
  allowed_targets: string[];
  ttl_secs: number;
  unattended: boolean;
}

/** The broker's ItemMeta: redacted metadata, never the secret value. */
export interface KvItem {
  id: string;
  kind: KvItemKind;
  provider_id: string;
  app_display: string;
  domain?: string | null;
  key: string;
  path?: string | null;
  source: string;
  session: boolean;
  expires_ms?: number | null;
  bytes: number;
  blob?: string | null;
  identity_provider: boolean;
  policy: KvItemPolicy;
  created_ms: number;
  updated_ms: number;
  rev: number;
  record_digest: string;
}

export interface KvDomainCount {
  domain: string;
  cookies: number;
  session_cookies: number;
  local_storage: number;
  passwords: number;
  signin: boolean;
  identity_provider: boolean;
  unavailable: number;
  unavailable_reason: string;
}
export interface KvInventory {
  provider_id: string;
  app_display: string;
  domains: KvDomainCount[];
  notes: string[];
}
export interface KvFavicon { site: string; png: string }
export interface KvLockOutcome { changed: string[]; skipped: string[] }

export type KvSigning =
  | { kind: "signed"; team_id: string; identifier: string; cdhash: string }
  | { kind: "windows_signed"; certificate_sha256: string; publisher: string; executable_sha256: string }
  | { kind: "ad_hoc"; identifier: string; cdhash: string }
  | { kind: "unsigned" }
  | { kind: "unknown" };

export interface KvCaller {
  pid: number;
  uid: number;
  path?: string | null;
  signing: KvSigning;
  first_party: boolean;
  os_verified: boolean;
  launched_by?: string | null;
  verified_name?: string | null;
}

export type KvSelector =
  | { kind: "item"; id: string }
  | { kind: "site"; app: string; site: string }
  | { kind: "app"; app: string }
  | { kind: "login"; site: string };

export interface KvAccessRequest {
  selectors: KvSelector[];
  targets: string[];
  actions: string[];
  duration_secs?: number | null;
  uses?: number | null;
  reason: string;
  claimed_name?: string | null;
  agent?: string | null;
}

export interface KvPending {
  id: string;
  caller: KvCaller;
  caller_fp: string;
  caller_display: string;
  request: KvAccessRequest;
  items: KvItem[];
  needs_import: KvSelector[];
  created_ms: number;
}

export interface KvGrant {
  id: string;
  request_id: string;
  caller_fp: string;
  caller_display: string;
  items: string[];
  targets: string[];
  actions: string[];
  created_ms: number;
  not_after_ms: number;
  uses_left?: number | null;
  revoked: boolean;
  agent?: string | null;
}

export interface KvRule {
  id: string;
  items: string[];
  targets: string[];
  callers: Array<{ fp: string; display: string }>;
  created_ms: number;
  not_after_ms: number;
  enabled: boolean;
  note: string;
}

export interface KvDelivery {
  import_id: string;
  target: string;
  provider_id: string;
  items: string[];
  caller_fp: string;
  delivered_ms: number;
  expires_ms: number;
  wiped: boolean;
}

export interface KvAuditEntry {
  seq: number;
  ts_ms: number;
  kind: string;
  actor: string;
  caller_fp: string;
  item?: string | null;
  target?: string | null;
  decision: string;
  detail?: string;
  authless?: boolean;
}

export interface KvStatus {
  version: string;
  initialized: boolean;
  unlocked: boolean;
  disabled: boolean;
  caller_first_party: boolean;
  caller_display: string;
  items: number;
  pending: number;
  unlock_policy?: "auto" | "presence" | null;
  /** The daemon can create the OS key store protector (setup offers Touch ID). */
  os_protector_available?: boolean;
  passphrase_available?: boolean;
  /** Protectors that can unlock this vault now ("macos-keychain", "passphrase", ...). */
  unlock_protectors?: string[];
  auto_wipe?: boolean | null;
  browse_until_ms?: number | null;
  skip_unlock_prompt?: boolean | null;
  reset_notice?: string | null;
}

export interface KvVerification {
  ok: boolean;
  entries: number;
  unauthenticated: number;
  tampered_line?: number | null;
  reason?: string | null;
}

export type KvAvailability =
  | "ready"
  | "not_running"
  | "impostor"
  | "connect"
  | "no_vault"
  | "locked"
  | "not_first_party"
  | "unsupported"
  | string;

export interface KeyvaultOverview {
  availability: KvAvailability;
  message?: string;
  status?: KvStatus;
  serverVerified: boolean;
  items: KvItem[];
  namesVisible: boolean;
  itemsTotal: number;
  pending: KvPending[];
  grants: KvGrant[];
  rules: KvRule[];
  deliveries: KvDelivery[];
  audit: KvAuditEntry[];
  auditVerification?: KvVerification;
  partialErrors: string[];
}

export interface KeyvaultBridge {
  readonly isNative: boolean;
  overview(): Promise<KeyvaultOverview>;
  browse(): Promise<number>;
  endBrowse(): Promise<void>;
  inventory(app: string, profile?: string | null): Promise<KvInventory>;
  favicons(): Promise<KvFavicon[]>;
  lock(): Promise<void>;
  setLocked(itemIds: string[], locked: boolean): Promise<KvLockOutcome>;
  deleteItems(itemIds: string[]): Promise<string[]>;
  setAutoWipe(on: boolean): Promise<void>;
  setSkipUnlockPrompt(on: boolean): Promise<void>;
  /** Creates the vault with the OS key store; returns the recovery key to show once. */
  setup(): Promise<string | null>;
  /** Creates the vault with a passphrase (sent only to the broker, never kept). */
  setupWithPassphrase(passphrase: string): Promise<string | null>;
  unlock(): Promise<void>;
  /** Unlocks with the passphrase (sent only to the broker, never kept). */
  unlockWithPassphrase(passphrase: string): Promise<void>;
  /** The kill switch. Turning it off makes the daemon ask for Touch ID. */
  setDisabled(disabled: boolean): Promise<void>;
  /** Per-item "allow in unattended rules". Turning it on asks for Touch ID. */
  setUnattended(itemIds: string[], unattended: boolean): Promise<KvItem[]>;
  revokeGrant(id: string): Promise<number>;
  removeRule(id: string): Promise<void>;
  release(target: string): Promise<string[]>;
  /** Approves exactly `items` (`null`: everything asked; the sheet only
   * sends that when every row, imports included, is ticked). */
  approve(requestId: string, items: string[] | null): Promise<KvGrant>;
  deny(requestId: string): Promise<void>;
}

export function createTauriKeyvaultBridge(): KeyvaultBridge {
  const core = import("@tauri-apps/api/core");
  const invoke = async <T>(command: string, args?: Record<string, unknown>) =>
    (await core).invoke<T>(command, args);
  return {
    isNative: true,
    overview: () => invoke<KeyvaultOverview>("keyvault_overview"),
    browse: () => invoke<number>("keyvault_browse"),
    endBrowse: () => invoke<void>("keyvault_end_browse"),
    inventory: (app, profile) => invoke<KvInventory>("keyvault_inventory", { app, profile: profile ?? null }),
    favicons: () => invoke<KvFavicon[]>("keyvault_favicons"),
    lock: () => invoke<void>("keyvault_lock"),
    setLocked: (itemIds, locked) => invoke<KvLockOutcome>("keyvault_set_locked", { itemIds, locked }),
    deleteItems: (itemIds) => invoke<string[]>("keyvault_delete_items", { itemIds }),
    setAutoWipe: (on) => invoke<void>("keyvault_set_auto_wipe", { on }),
    setSkipUnlockPrompt: (on) => invoke<void>("keyvault_set_skip_unlock_prompt", { on }),
    setup: () => invoke<string | null>("keyvault_setup"),
    setupWithPassphrase: (passphrase) => invoke<string | null>("keyvault_setup_passphrase", { passphrase }),
    unlock: () => invoke<void>("keyvault_unlock"),
    unlockWithPassphrase: (passphrase) => invoke<void>("keyvault_unlock_passphrase", { passphrase }),
    setDisabled: (disabled) => invoke<void>("keyvault_set_disabled", { disabled }),
    setUnattended: (itemIds, unattended) =>
      invoke<KvItem[]>("keyvault_set_unattended", { args: { itemIds, unattended } }),
    revokeGrant: (id) => invoke<number>("keyvault_revoke_grant", { id }),
    removeRule: (id) => invoke<void>("keyvault_remove_rule", { id }),
    release: (target) => invoke<string[]>("keyvault_release", { target }),
    approve: (requestId, items) => invoke<KvGrant>("keyvault_approve", { requestId, items }),
    deny: (requestId) => invoke<void>("keyvault_deny", { requestId }),
  };
}

/** Outside the app there is no daemon to talk to: say so, show nothing. */
export function createFallbackKeyvaultBridge(): KeyvaultBridge {
  const unavailable = () => Promise.reject(new Error("The Keyvault needs the Cua Spaces app"));
  return {
    isNative: false,
    overview: async () => ({
      availability: "not_running",
      message: "The Keyvault needs the Cua Spaces app and the Cua daemon.",
      serverVerified: false,
      items: [],
      namesVisible: false,
      itemsTotal: 0,
      pending: [],
      grants: [],
      rules: [],
      deliveries: [],
      audit: [],
      partialErrors: [],
    }),
    browse: unavailable,
    endBrowse: unavailable,
    inventory: unavailable,
    favicons: unavailable,
    lock: unavailable,
    setLocked: unavailable,
    deleteItems: unavailable,
    setAutoWipe: unavailable,
    setSkipUnlockPrompt: unavailable,
    setup: unavailable,
    setupWithPassphrase: unavailable,
    unlock: unavailable,
    unlockWithPassphrase: unavailable,
    setDisabled: unavailable,
    setUnattended: unavailable,
    revokeGrant: unavailable,
    removeRule: unavailable,
    release: unavailable,
    approve: unavailable,
    deny: unavailable,
  };
}

export function createKeyvaultBridge(): KeyvaultBridge {
  return hasTauri() ? createTauriKeyvaultBridge() : createFallbackKeyvaultBridge();
}
