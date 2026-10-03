// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Sending host files into a Space, for the pop-out list window's Teleport drop
 * zone. Files land in the Space's `~/Downloads`.
 *
 * The shell's `send_files_to_space` verifies every file by sha256 IN THE SPACE
 * before it resolves, so a resolved promise here means the bytes are on the
 * target's filesystem — not merely that a request was accepted. Anything less
 * rejects, and the zone shows the rejection. Outside Tauri the bridge refuses
 * rather than pretending, so the browser/test build can never claim a send.
 */
import { hasTauri } from "./bridge";

/** Where a UI send lands. The MCP's `send_file` tool defaults to the same
 * directory but, unlike this, lets an agent override it. */
export const GUEST_DOWNLOADS_DIR = "~/Downloads";

export interface SentFile {
  name: string;
  /** Absolute path in the Space, with `~` already resolved there. */
  dest: string;
  bytes: number;
  /** The digest the Space computed for the landed file. */
  sha256: string;
}

/** Completion of selected host items, not an estimate of bytes in flight.
 * A folder can yield several verified files (or none when empty/ignored). */
export interface FileSendProgress {
  completedItems: number;
  totalItems: number;
  currentPath: string | null;
  files: SentFile[];
}

export class FileSendFailure extends Error {
  readonly progress: FileSendProgress;
  readonly remainingPaths: string[];

  constructor(error: unknown, progress: FileSendProgress, remainingPaths: string[]) {
    super(error instanceof Error ? error.message : String(error));
    this.name = "FileSendFailure";
    this.progress = progress;
    this.remainingPaths = remainingPaths;
  }
}

export interface FileSendBridge {
  readonly isNative: boolean;
  /** Native open panel; resolves to the chosen host paths ([] if cancelled). */
  pickFiles(): Promise<string[]>;
  /** Windows' native dialog uses a separate folder-selection mode. */
  pickFolders?(): Promise<string[]>;
  /** Send host files to the Space's ~/Downloads. Rejects unless all landed. */
  sendFiles(spaceId: string, paths: string[]): Promise<SentFile[]>;
  /** Optional for older bridge implementations; advances only after the
   * existing native command verifies that selected item in the Space. */
  sendFilesWithProgress?(
    spaceId: string,
    paths: string[],
    onProgress: (progress: FileSendProgress) => void,
  ): Promise<SentFile[]>;
}

export function createFallbackFileSendBridge(): FileSendBridge {
  const refuse = async (): Promise<never> => {
    throw new Error("sending files needs the Cua Spaces app");
  };
  return { isNative: false, pickFiles: async () => [], sendFiles: refuse };
}

export function createTauriFileSendBridge(): FileSendBridge {
  const core = import("@tauri-apps/api/core");
  const invoke = async <T>(command: string, args?: Record<string, unknown>) =>
    (await core).invoke<T>(command, args);
  const pick = async (directory: boolean): Promise<string[]> => {
    const chosen = await invoke<string[] | string | null>("plugin:dialog|open", {
      options: { multiple: true, directory, title: directory ? "Send folders" : "Teleport" },
    });
    if (!chosen) return [];
    return Array.isArray(chosen) ? chosen : [chosen];
  };
  const send = async (
    spaceId: string,
    paths: string[],
    onProgress?: (progress: FileSendProgress) => void,
  ): Promise<SentFile[]> => {
    if (!paths.length) throw new Error("nothing to send");
    const files: SentFile[] = [];
    for (let index = 0; index < paths.length; index++) {
      const progress = (): FileSendProgress => ({
        completedItems: index, totalItems: paths.length, currentPath: paths[index], files: [...files],
      });
      onProgress?.(progress());
      try {
        const landed = await invoke<SentFile[]>("send_files_to_space", { spaceId, paths: [paths[index]] });
        files.push(...landed);
      } catch (error) {
        // Earlier commands committed and verified their files. Never discard
        // those receipts or automatically resend them after a later failure.
        throw new FileSendFailure(error, progress(), paths.slice(index));
      }
      onProgress?.({ completedItems: index + 1, totalItems: paths.length, currentPath: null, files: [...files] });
    }
    return files;
  };
  return {
    isNative: true,
    pickFiles: () => pick(false),
    pickFolders: () => pick(true),
    sendFiles: send,
    sendFilesWithProgress: send,
  };
}

export function createFileSendBridge(): FileSendBridge {
  return hasTauri() ? createTauriFileSendBridge() : createFallbackFileSendBridge();
}
