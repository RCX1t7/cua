// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { Consent, ConsentItem, PickerState, TeleportHost } from "@trycua/cua/teleport";
import { core } from "../core";
import type { KeyvaultBridge, KeyvaultOverview, KvInventory } from "../native/keyvault";

/** The released shared review contract; no secret values are in these models. */
export interface ReviewConsent extends Consent {
  cookieDomains?: string[] | null;
  exclude?: string[];
  fromVault?: string[] | null;
  includePasswords?: boolean;
}

export interface ReviewDomain {
  domain: string;
  counts: string;
  count: number;
  signin: boolean;
  identityProvider: boolean;
  selected: boolean;
  selectable: boolean;
  unavailable: number;
  unavailableNote: string;
}

export interface ReviewToggle { key: string; label: string; detail: string; bytes: number; sensitive: boolean; selected: boolean }
export interface VaultSource { available: boolean; items: number; saved: string; selected: string[]; passwordIds: string[] }
export interface ReviewView {
  title: string;
  items: ConsentItem[];
  needsAcknowledgement: boolean;
  acknowledged: boolean;
  offersSaveToKeyvault: boolean;
  saveToKeyvault: boolean;
  needsRelayPlaintextAcknowledgement: boolean;
  acknowledgedRelayPlaintext: boolean;
  canConfirm: boolean;
  leavesText: string | null;
  warnings: string[];
  toggles: ReviewToggle[];
  offersDomains: boolean;
  needsDomains: boolean;
  domains: ReviewDomain[];
  domainSummary: string;
  domainQuery: string;
  selectedDomains: string[];
  source: "live" | "vault";
  offersVault: boolean;
  vault: VaultSource;
  vaultLabel: string;
  liveLabel: string;
  offersPasswords: boolean;
  includePasswords: boolean;
  passwordsLabel: string;
  sourceNote: string;
}

export interface ReviewVaultSource { count: number; newestMs: number; ids: string[]; passwordIds: string[] }
export type TeleportReviewHost = TeleportHost & { keyvault?: Pick<KeyvaultBridge, "overview" | "inventory" | "browse" | "endBrowse"> };
export type ReviewEvent =
  | { type: "domains-loaded"; inventory: KvInventory; remembered?: string[] | null }
  | { type: "domains-failed" }
  | { type: "toggle-domain"; domain: string }
  | { type: "select-shown-domains"; value: boolean }
  | { type: "domain-query"; text: string }
  | { type: "toggle-item"; key: string }
  | { type: "send-from"; source: "live" | "vault" }
  | { type: "vault-items"; count: number; newestMs: number; nowMs: number; selected: string[]; passwordIds: string[] }
  | { type: "toggle-passwords"; value: boolean }
  | { type: "vault-selection"; selected: string[] };

export const pickerReview = (state: PickerState): ReviewView | null => core("flow.review", { state });
export const pickerConsent = (state: PickerState): ReviewConsent => core("flow.consent", { state });
export const reviewVaultSource = (overview: KeyvaultOverview, providerId: string): ReviewVaultSource => core("keyvault.vaultSource", { overview, providerId });
