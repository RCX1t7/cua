// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useMemo, useState } from "react";

import type { MediaStatus } from "@cua/spacesd-html5/core/mediaSession";
import { getScreenshot, rememberScreenshot } from "../model/screenshotCache";
import type { FleetBridge } from "../native/fleet";
import { readSetting, writeSetting } from "../state/settings";
import { MediaCanvas, toMediaTicket } from "./WindowStream";

const AUTO_CONNECT_KEY = "cua.settings.autoConnect";

/** The main detail uses the same interactive player as the standalone viewer. */
export function DesktopPreview({
  fleet,
  spaceId,
  spaceName,
  available,
  unavailableReason,
}: {
  fleet: FleetBridge;
  spaceId: string;
  spaceName: string;
  available: boolean;
  unavailableReason: string;
}) {
  const [autoConnect, setAutoConnect] = useState(() => readSetting(AUTO_CONNECT_KEY, "true") !== "false");
  const [requested, setRequested] = useState(autoConnect);
  const [attempt, setAttempt] = useState(0);
  const [status, setStatus] = useState<MediaStatus>("connecting");
  const [detail, setDetail] = useState<string | undefined>();
  const [frameRendered, setFrameRendered] = useState(false);
  const [geometry, setGeometry] = useState({ width: 16, height: 10 });
  const [lastFrame, setLastFrame] = useState(() => getScreenshot(spaceId)?.dataUrl ?? null);
  const [inputError, setInputError] = useState<string | null>(null);
  const [inputDelivered, setInputDelivered] = useState(false);
  const active = available && requested;
  const live = active && status === "streaming" && frameRendered;
  const open = useMemo(
    () => () => fleet.openStream(spaceId, { kind: "display" }, { audio: true, policy: "allow_activation" }).then(toMediaTicket),
    [fleet, spaceId],
  );
  const connect = () => {
    setInputError(null);
    setInputDelivered(false);
    setFrameRendered(false);
    setDetail(undefined);
    setStatus("connecting");
    setRequested(true);
    setAttempt((n) => n + 1);
  };
  const message = !available ? unavailableReason
    : !requested ? "Desktop disconnected."
    : status === "failed" || status === "ended" ? detail ?? "The desktop stream ended."
    : status === "reconnecting" ? "Reconnecting to the desktop…"
    : status === "streaming" && !frameRendered ? "Waiting for the first desktop frame…"
    : `Connecting to ${spaceName}…`;

  return (
    <div data-stream-status={active ? status : "disconnected"} data-frame-rendered={live}>
      <div style={{ position: "relative", aspectRatio: `${geometry.width} / ${geometry.height}`, background: "#000" }}>
        {!live && lastFrame && <img src={lastFrame} alt={`${spaceName} last desktop frame`} style={{ position: "absolute", inset: 0, width: "100%", height: "100%", opacity: 0.35 }} />}
        {active && (
          <MediaCanvas
            open={open}
            interactive
            audio
            generation={attempt}
            label={`${spaceName} interactive desktop`}
            style={{ display: "block", width: "100%", height: "100%", maxWidth: "none", maxHeight: "none" }}
            onStatus={(s, d) => {
              setStatus(s);
              setDetail(d);
              if (s !== "streaming") {
                setFrameRendered(false);
                setInputDelivered(false);
              }
            }}
            onGeometry={(width, height) => setGeometry({ width, height })}
            onFrame={() => setFrameRendered(true)}
            onServerError={(error) => setInputError(`${String(error.code ?? "stream_error")}: ${String(error.message ?? "The server refused the operation.")}`)}
            onInputAcknowledgement={(result) => {
              setInputDelivered(result.delivered);
              setInputError(result.delivered ? null : `${result.error?.code ?? "delivery_failed"}: ${result.error?.message ?? "Input was not delivered; the server did not provide a reason."}`);
            }}
            onLastFrame={(canvas) => {
              try {
                const frame = canvas.toDataURL("image/png");
                rememberScreenshot(spaceId, frame, Date.now());
                setLastFrame(frame);
              } catch {
                // A failed snapshot must not interrupt closing the stream.
              }
            }}
            onStopped={(id) => void fleet.closeStream(spaceId, id).catch(() => {})}
          />
        )}
        {!live && (
          <div className="dw-preview-empty" style={{ position: "absolute", inset: 0, color: "#fff", background: "rgba(0, 0, 0, 0.35)" }} role="status">
            <span>{message}</span>
            {available && (!requested || status === "failed" || status === "ended") && (
              <button type="button" className="dw-btn dw-btn-primary" onClick={connect}>{requested ? "Try again" : "Connect"}</button>
            )}
          </div>
        )}
      </div>
      <div className="dw-card-pad" style={{ display: "flex", alignItems: "center", gap: 12, flexWrap: "wrap", color: "#f4f4f5", background: "#18181b" }}>
        <label className="dw-check" style={{ color: "inherit" }}>
          <input type="checkbox" checked={autoConnect} onChange={(event) => {
            const on = event.target.checked;
            writeSetting(AUTO_CONNECT_KEY, String(on));
            setAutoConnect(on);
            if (on) connect();
          }} />
          <span>Auto-connect to desktops</span>
        </label>
        {active && <button type="button" className="dw-btn" onClick={() => setRequested(false)}>Disconnect</button>}
        {live && <span role="status">{inputDelivered ? "Input accepted by the host" : "Click the desktop to control it"}</span>}
      </div>
      {inputError && <p className="dw-inline-error" role="alert">Input or stream operation refused: {inputError}</p>}
    </div>
  );
}
