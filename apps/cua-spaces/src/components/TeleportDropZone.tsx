// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The Teleport drop zone: the dashed region beneath the Windows section on the
 * pop-out list window's second page (a Space's targets).
 *
 * Two things can be teleported into it, and they are NOT the same operation:
 *
 *   * a FILE — copied to the Space's `~/Downloads`, verified by digest there
 *     before this reports it landed (`send_files_to_space`); and
 *   * an APP — the existing logged-in-session teleport, which is the picker flow
 *     the page-2 footer already opens. This zone calls straight into that same
 *     path (`onTeleportApp`) rather than growing a second teleport.
 *
 * Both accept a drop as well as a click, including a dragged window "the same
 * way you can into the notch": the notch tiles hit-test the global AX
 * window-drag against their own bounds and mark themselves
 * `data-drop-target="true"`, and so does this.
 *
 * Page 1 has no selected Space, so the zone is only ever rendered on page 2 —
 * the same rule the footer actions follow.
 */
import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from "react";

import { handleViewerAppDrop } from "../native/appDrop";
import { core } from "../core";
import { isResize } from "../model/dragTrigger";
import { detailCopy } from "../model/window";
import { hostOs } from "../model/host";
import { Sym } from "./desktop/Sym";
import { FileSendFailure, type FileSendBridge, type SentFile } from "../native/fileSend";
import { screenToClient, type WindowDragBridge } from "../native/windowDrag";

export interface TeleportDropZoneProps {
  spaceId: string;
  spaceName: string;
  fileSend: FileSendBridge;
  /** Opens the existing app-teleport picker for this Space (footer's action). */
  onTeleportApp: () => void;
  /** Global window-drag monitor, so a dragged window can be dropped in here. */
  windowDrag?: WindowDragBridge;
  /**
   * An app bundle dropped here (Finder, Dock) opens "Teleport an app…" for
   * it; resolves true when the drop was an app. Default: the shell picker.
   */
  appDrop?: (paths: string[]) => Promise<boolean>;
}

type Status =
  | { kind: "idle" }
  | { kind: "choosing" }
  | { kind: "routing" }
  | { kind: "sending"; paths: string[]; completedItems: number; totalItems: number; currentPath: string | null; files: SentFile[] }
  | { kind: "sent"; files: SentFile[]; totalItems: number }
  | { kind: "failed"; message: string; files: SentFile[]; completedItems: number; totalItems: number; remainingPaths: string[] };

/** Keep the actual transfer/error alive when the Space's Teleport tab is
 * hidden. Each bridge/Space has its own state; changing Spaces cannot attach
 * an earlier send's receipts to the newly selected destination. */
interface TransferState {
  status: Status;
  subscribe: (listener: () => void) => () => void;
  snapshot: () => Status;
  set: (status: Status) => void;
}
const transfers = new WeakMap<FileSendBridge, Map<string, TransferState>>();
function transferState(bridge: FileSendBridge, spaceId: string): TransferState {
  let spaces = transfers.get(bridge);
  if (!spaces) { spaces = new Map(); transfers.set(bridge, spaces); }
  let state = spaces.get(spaceId);
  if (!state) {
    const listeners = new Set<() => void>();
    const created: TransferState = {
      status: { kind: "idle" },
      subscribe: (listener) => { listeners.add(listener); return () => { listeners.delete(listener); }; },
      snapshot: () => created.status,
      set: (status) => { created.status = status; listeners.forEach((listener) => listener()); },
    };
    state = created;
    spaces.set(spaceId, state);
  }
  return state;
}
function isBusy(status: Status): boolean {
  return status.kind === "sending" || status.kind === "choosing" || status.kind === "routing";
}
function failure(message: string, previous: Status): Status {
  return { kind: "failed", message, files: "files" in previous ? previous.files : [],
    completedItems: "completedItems" in previous ? previous.completedItems : 0,
    totalItems: "totalItems" in previous ? previous.totalItems : 0, remainingPaths: [] };
}
function fileName(path: string): string { return path.split(/[\\/]/).filter(Boolean).pop() ?? path; }

/** What the zone says it did (the app core's words). Deliberately concrete:
 * a vague "Sent" is how a lost file passes for a delivered one. */
function statusLine(status: Status): string | null {
  switch (status.kind) {
    case "idle":
      return null;
    case "choosing":
      return "Choose files or folders to send.";
    case "routing":
      return "Checking the dropped items…";
    case "sending":
      return core<string>("transfer.dropSendingText", { paths: status.paths });
    case "sent": {
      const line = core<string>("transfer.dropSentText", {
        files: status.files.map((f) => ({ name: f.name, dest: f.dest, bytes: f.bytes })),
      });
      return line || `Finished sending ${status.totalItems} selected item(s). No file receipts were returned (folders may be empty or ignored).`;
    }
    case "failed":
      return status.message;
  }
}

export default function TeleportDropZone({
  spaceId,
  spaceName,
  fileSend,
  onTeleportApp,
  windowDrag,
  appDrop,
}: TeleportDropZoneProps) {
  const copy = detailCopy();
  const windowsHost = hostOs() === "windows";
  const transfer = transferState(fileSend, spaceId);
  const status = useSyncExternalStore(transfer.subscribe, transfer.snapshot);
  const [over, setOver] = useState(false);
  const zoneRef = useRef<HTMLDivElement | null>(null);
  const busy = isBusy(status);

  const send = useCallback(
    async (paths: string[], prior?: Extract<Status, { kind: "failed" }>) => {
      if (!paths.length || isBusy(transfer.status)) return;
      const earlierFiles = prior?.files ?? [];
      const earlierCount = prior?.completedItems ?? 0;
      const totalItems = prior?.totalItems ?? paths.length;
      // Set the shared lock before the first await (also covers drops during
      // the React render gap or while this tab is hidden).
      transfer.set({ kind: "sending", paths, completedItems: earlierCount, totalItems,
        currentPath: paths[0], files: earlierFiles });
      try {
        const files = fileSend.sendFilesWithProgress
          ? await fileSend.sendFilesWithProgress(spaceId, paths, (progress) =>
            transfer.set({ kind: "sending", paths, completedItems: earlierCount + progress.completedItems,
              totalItems, currentPath: progress.currentPath, files: [...earlierFiles, ...progress.files] }))
          : await fileSend.sendFiles(spaceId, paths);
        transfer.set({ kind: "sent", files: [...earlierFiles, ...files], totalItems });
      } catch (error) {
        const progress = error instanceof FileSendFailure ? error.progress : null;
        transfer.set({ kind: "failed", message: error instanceof Error ? error.message : String(error),
          files: [...earlierFiles, ...(progress?.files ?? [])],
          completedItems: earlierCount + (progress?.completedItems ?? 0), totalItems,
          remainingPaths: error instanceof FileSendFailure ? error.remainingPaths : paths });
      }
    },
    [fileSend, spaceId, transfer],
  );

  const selectFile = useCallback(async (folders = false) => {
    if (isBusy(transfer.status)) return;
    const previous = transfer.status;
    transfer.set({ kind: "choosing" });
    try {
      const paths = folders ? await fileSend.pickFolders?.() ?? [] : await fileSend.pickFiles();
      transfer.set(previous); // Cancel keeps the previous result/error visible.
      await send(paths);
    } catch (error) {
      transfer.set(failure(error instanceof Error ? error.message : String(error), previous));
    }
  }, [fileSend, send, transfer]);

  // -- dropping a FILE from Finder (Tauri's native webview drag-drop) ---------
  useEffect(() => {
    if (!fileSend.isNative) return;
    let dispose: (() => void) | null = null;
    let cancelled = false;
    void (async () => {
      try {
        const { getCurrentWebview } = await import("@tauri-apps/api/webview");
        const unlisten = await getCurrentWebview().onDragDropEvent((event) => {
          const payload = event.payload;
          if (payload.type === "leave") {
            setOver(false);
            return;
          }
          if (payload.type !== "over" && payload.type !== "drop") return;
          // Drag positions are PHYSICAL pixels; the DOM hit-test is logical.
          const scale = window.devicePixelRatio || 1;
          const inside = isInsideZone(zoneRef.current, {
            x: payload.position.x / scale,
            y: payload.position.y / scale,
          });
          if (payload.type === "over") {
            setOver(inside);
            return;
          }
          setOver(false);
          if (inside && payload.paths?.length && !isBusy(transfer.status)) {
            const paths = payload.paths;
            const previous = transfer.status;
            transfer.set({ kind: "routing" });
            const route = appDrop ?? ((p: string[]) => handleViewerAppDrop(p, { id: spaceId, name: spaceName }));
            void route(paths)
              .then((wasApp) => {
                transfer.set(previous);
                if (!wasApp) void send(paths);
              })
              .catch((error: unknown) =>
                transfer.set(failure(error instanceof Error ? error.message : String(error), previous)),
              );
          }
        });
        if (cancelled) unlisten();
        else dispose = unlisten;
      } catch {
        // No webview drag-drop here; the buttons still work.
      }
    })();
    return () => {
      cancelled = true;
      dispose?.();
    };
  }, [fileSend.isNative, send, appDrop, spaceId, spaceName, transfer]);

  // -- dropping a WINDOW, the way the notch tiles accept one -----------------
  // The AX monitor reports the drag in screen points, so the zone converts them
  // into this window's client space before hit-testing itself.
  useEffect(() => {
    if (!windowDrag?.isNative) return;
    let dispose: (() => void) | null = null;
    let cancelled = false;
    let origin: { x: number; y: number } | null = null;
    void (async () => {
      try {
        const { getCurrentWindow } = await import("@tauri-apps/api/window");
        const win = getCurrentWindow();
        const refreshOrigin = async () => {
          const [position, scale] = await Promise.all([win.innerPosition(), win.scaleFactor()]);
          origin = { x: position.x / scale, y: position.y / scale };
        };
        await refreshOrigin();
        // A window resized from an edge or a corner is not dropped anywhere
        // (the app core compares its frames).
        let resizing = false;
        const unlisten = await windowDrag.onWindowDrag((drag) => {
          if (drag.phase === "start") {
            resizing = isResize(drag);
            void refreshOrigin();
          }
          if (!origin || resizing) return;
          const inside = isInsideZone(zoneRef.current, screenToClient(drag, origin));
          if (drag.phase === "end") {
            setOver(false);
            // A window carries a logged-in app session, so it takes the app
            // teleport path — the same one the footer button opens.
            if (inside && !isBusy(transfer.status)) onTeleportApp();
            return;
          }
          setOver(inside);
        });
        if (cancelled) unlisten();
        else dispose = unlisten;
      } catch {
        // No window-drag monitor (no Accessibility permission): clicks still work.
      }
    })();
    return () => {
      cancelled = true;
      dispose?.();
    };
  }, [windowDrag, onTeleportApp, transfer]);

  const line = statusLine(status);
  return (
    <section className="swl-section sl-teleport" aria-label={`Teleport to ${spaceName}`}>
      {/* No label here: the page's section title names it (one label per thing). */}
      <div
        ref={zoneRef}
        className="sl-dropzone"
        data-drop-target={over ? "true" : undefined}
        data-busy={busy ? "true" : undefined}
      >
        <Sym name={over ? copy.teleportSymbolActive : copy.teleportSymbol} size={22} className="sl-dropzone-icon" />
        <p className="sl-dropzone-caption">{windowsHost ? "Drop files or folders to send them, or drag an app window here to review its transfer" : copy.dropCaption}</p>
        {windowsHost && (
          <p className="sl-dropzone-status" role="note">
            Window dragging opens app review. Session transfer depends on the provider and native approval; sign in inside the Space when transfer is unavailable.
          </p>
        )}
        <div className="sl-dropzone-actions">
          <button type="button" className="sl-dropzone-button" onClick={() => void selectFile()} disabled={busy}>
            {copy.sendFile}
          </button>
          {fileSend.pickFolders && (
            <button type="button" className="sl-dropzone-button" onClick={() => void selectFile(true)} disabled={busy}>
              Send folders…
            </button>
          )}
          <button type="button" className="sl-dropzone-button" onClick={onTeleportApp} disabled={busy}>
            {copy.teleportApp}
          </button>
        </div>
        {line ? (
          <p className="sl-dropzone-status" data-kind={status.kind} role="status" aria-live={status.kind === "failed" ? "assertive" : "polite"}>
            {line}
          </p>
        ) : null}
        {status.kind === "sending" && (
          <div className="sl-dropzone-status" role="status">
            <progress value={status.completedItems} max={status.totalItems} aria-label="Verified selected items" />
            <span> {status.completedItems} of {status.totalItems} selected items finished</span>
            {status.currentPath && <span> — sending {fileName(status.currentPath)}</span>}
          </div>
        )}
        {(status.kind === "sending" || status.kind === "failed") && status.files.length > 0 && (
          <p className="sl-dropzone-status" role="status">
            Verified in the Space: {core<string>("transfer.dropSentText", {
              files: status.files.map((file) => ({ name: file.name, dest: file.dest, bytes: file.bytes })),
            })}
          </p>
        )}
        {status.kind === "failed" && status.remainingPaths.length > 0 && (
          <>
            <p className="sl-dropzone-status" role="note">
              {status.completedItems} of {status.totalItems} selected items finished. The interrupted item may have partially reached the Space.
              Retrying skips completed items; the default conflict policy keeps both copies if the interrupted item already arrived.
            </p>
            <button type="button" className="sl-dropzone-button" onClick={() => void send(status.remainingPaths, status)}>
              Retry remaining {status.remainingPaths.length} item(s)
            </button>
          </>
        )}
      </div>
    </section>
  );
}

/** Whether a client-space point falls inside the zone's box. Exported so the
 * drop hit-test is testable without a real drag. */
export function isInsideZone(
  element: HTMLElement | null,
  point: { x: number; y: number },
): boolean {
  if (!element) return false;
  const box = element.getBoundingClientRect();
  if (box.width === 0 && box.height === 0) return false;
  return (
    point.x >= box.left && point.x <= box.right && point.y >= box.top && point.y <= box.bottom
  );
}
