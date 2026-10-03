// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/** Released shared Keyvault presentation/reducers. No access decisions or values live here. */
import { core } from "../core";
import type { KeyvaultOverview, KvAuditEntry, KvCaller, KvDelivery, KvGrant, KvRule } from "../native/keyvault";

export type KvCategory = "all" | "waiting" | "access" | "recent";
export type KvSelection = { kind: "category"; category: KvCategory } | { kind: "app"; key: string };
export type Tri = "on" | "off" | "mixed";
export type KvCommand =
  | { type: "setup" | "unlock" | "browse" | "end-browse" }
  | { type: "set-disabled"; disabled: boolean }
  | { type: "set-unattended"; itemIds: string[]; unattended: boolean }
  | { type: "set-locked"; itemIds: string[]; locked: boolean }
  | { type: "delete-items"; itemIds: string[] }
  | { type: "set-auto-wipe" | "set-skip-unlock-prompt"; on: boolean }
  | { type: "revoke-grant" | "remove-rule"; id: string }
  | { type: "release"; target: string }
  | { type: "approve"; requestId: string; items: string[] | null }
  | { type: "deny"; requestId: string };
export interface SigningBadge { text: string; tone: "ok" | "warn" | "danger" }
export interface KvSidebar {
  categories: { category: KvCategory; title: string; symbol: string; count: number | null; badge: number | null }[];
  apps: { key: string; title: string; items: number; waiting: boolean }[];
}
export interface Decision { entry: KvAuditEntry; verb: string; tone: "ok" | "deny" | "info"; what: string }
export interface KvPendingRow { id: string; caller: string; badge: SigningBadge; summary: string; wants: string; claims: string[] }
export interface KvAccessRow { kind: "grant" | "rule" | "delivery"; key: string; text: string; detail: string; actionLabel: string; command: KvCommand; imports: string[] }
export interface KvListView { title: string; vault: boolean; pending: KvPendingRow[]; access: KvAccessRow[]; recent: { decision: Decision; age: string }[]; emptyText: string | null }
export interface KvPage {
  ready: boolean; unavailableTitle: string | null; message: string | null;
  canSetup: boolean; canUnlock: boolean; killSwitchVisible: boolean; killSwitchEnabled: boolean;
  disabled: boolean; disabledBanner: string | null; partialErrors: string[];
  logStatus: string | null; logTampered: boolean; protection: { label: string; value: string }[];
  revokeAll: boolean; hasItems: boolean; searchVisible: boolean; skipUnlockPrompt: boolean;
  resetNotice: string | null; pendingCount: number; killSwitchHelp: string;
  labels: { deny: string; review: string; cancel: string; setUp: string; unlock: string; revokeAll: string; confirmNote: string; protectionTitle: string };
  form: KvCredentialForm | null;
}
export interface VaultState { query: string; selected: string[]; expanded: string[]; app: string | null }
export type VaultAction = { type: "query"; text: string } | { type: "toggle"; id: string } | { type: "toggle-group" | "toggle-open"; key: string } | { type: "select-all" | "clear" };
export interface VaultRow { id: string; kind: string; kindLabel: string; symbol: string; title: string; subtitle: string; updated: string; locked: boolean; lockSymbol: string; lockHelp: string; identityProvider: boolean; selected: boolean }
export interface VaultGroup { key: string; count: number; selected: Tri; lock: "locked" | "unlocked" | "mixed"; unlockIds: string[]; lockIds: string[]; open: boolean; rows: VaultRow[] }
export interface VaultSite extends VaultGroup { site: string; counts: string; updated: string }
export interface VaultApp extends Omit<VaultGroup, "rows"> { providerId: string; name: string; summary: string; updated: string; sites: VaultSite[]; files: VaultGroup | null }
export interface VaultView {
  apps: VaultApp[]; shown: number; total: number; emptyText: string | null;
  namesHidden: boolean; hiddenNote: string | null; showNamesLabel: string; searchPrompt: string;
  selection: { count: number; ids: string[]; title: string; canUnlock: boolean; canLock: boolean; alwaysAsk: number; unlockIds: string[]; lockIds: string[] };
  canSelectAll: boolean;
}
export interface ApprovalState { requestId: string; selected: string[] }
export type ApprovalAction = { type: "toggle"; key: string } | { type: "select-all" | "clear" };
export interface ApprovalView {
  requestId: string; title: string; caller: string; badge: SigningBadge; targets: string; wants: string; summary: string; claims: string[];
  rows: { key: string; title: string; account: string; providerId: string; items: number; itemIds: string[]; selected: boolean; isImport: boolean }[];
  canApprove: boolean; blockedReason: string | null; approveLabel: string; gone: boolean;
}
export type KvFormMode = "setup" | "unlock";
export interface KvCredentialForm { mode: KvFormMode; method: "touch-id" | "passphrase"; help: string; passphraseLabel: string | null; confirmLabel: string | null; submitLabel: string }
export interface KvPassphraseCheck { canSubmit: boolean; strength: "weak" | "fair" | "strong" | null; hint: string | null }
export interface UnlockPrompt { title: string; message: string; subject: string; deny: string; allow: string; neverAsk: string }
export interface KvDeleteConfirm { title: string; message: string; confirm: string; cancel: string }

export const kvSidebar = (overview: KeyvaultOverview, now: number): KvSidebar => core("keyvault.sidebar", { overview, now });
export const kvList = (overview: KeyvaultOverview, selection: KvSelection, now: number): KvListView => core("keyvault.list", { overview, selection, now });
export const kvPage = (overview: KeyvaultOverview, now: number): KvPage => core("keyvault.page", { overview, now });
export const vaultInitial = (): VaultState => ({ query: "", selected: [], expanded: [], app: null });
export const vaultReduce = (overview: KeyvaultOverview, state: VaultState, action: VaultAction): VaultState => core("keyvault.vaultReduce", { overview, state, action });
export const vaultView = (overview: KeyvaultOverview, state: VaultState, now: number): VaultView => core("keyvault.vaultView", { overview, state, now });
export const kvUnlockPrompt = (overview: KeyvaultOverview, count: number, name: string | null = null): UnlockPrompt | null => core("keyvault.unlockPrompt", { overview, count, name });
export const kvDeleteConfirm = (count: number, liveCopies: number): KvDeleteConfirm => core("keyvault.deleteConfirm", { count, liveCopies });
export const openApproval = (requestId: string): ApprovalState => core("approval.open", { requestId });
export const reduceApproval = (overview: KeyvaultOverview, state: ApprovalState, action: ApprovalAction): ApprovalState => core("approval.reduce", { overview, state, action });
export const approvalView = (overview: KeyvaultOverview, state: ApprovalState): ApprovalView => core("approval.view", { overview, state });
export const approvalCommand = (overview: KeyvaultOverview, state: ApprovalState): KvCommand | null => core("approval.approveCommand", { overview, state });
export const kvCredentialForm = (overview: KeyvaultOverview): KvCredentialForm | null => core("keyvault.credentialForm", { overview });
export const kvPassphraseCheck = (mode: KvFormMode, passphrase: string, confirm: string): KvPassphraseCheck => core("keyvault.passphraseCheck", { mode, passphrase, confirm });
export const kvRecoveryKeyText = (key: string): string => core("keyvault.recoveryKeyText", { key });
export const liveGrant = (grant: KvGrant, now: number): boolean => core("keyvault.liveGrant", { grant, now });
export const liveRule = (rule: KvRule, now: number): boolean => core("keyvault.liveRule", { rule, now });
export const liveDelivery = (delivery: KvDelivery, now: number): boolean => core("keyvault.liveDelivery", { delivery, now });
export const signingBadge = (caller: KvCaller): SigningBadge => core("keyvault.signingBadge", { caller });
