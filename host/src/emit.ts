import { randomUUID } from "node:crypto";
import type { Envelope } from "@pidock/protocol";

/** Single writer for stdout JSONL frames; keeps ordering intact. */
export function emitEvent(
  sessionId: string,
  kind: string,
  payload: unknown,
  opts: { persist?: boolean; seq?: number; ts?: Date } = {},
): void {
  const envelope: Envelope = {
    event_id: randomUUID(),
    session_id: sessionId,
    persist: opts.persist ?? false,
    ...(opts.persist && opts.seq !== undefined ? { seq: opts.seq } : {}),
    ts: (opts.ts ?? new Date()).toISOString(),
    kind,
    payload,
  };
  writeFrame(envelope);
}

export function emitResponse(
  id: string,
  result: unknown,
): void {
  writeFrame({ id, ok: true, result });
}

export function emitError(id: string, code: string, message: string): void {
  writeFrame({ id, ok: false, error: { code, message } });
}

export function emitFatal(message: string, sessionId?: string): void {
  if (sessionId) {
    emitEvent(sessionId, "error", { message, fatal: true });
  } else {
    writeFrame({ event_id: randomUUID(), session_id: "", persist: false, ts: new Date().toISOString(), kind: "error", payload: { message, fatal: true } });
  }
}

function writeFrame(frame: unknown): void {
  try {
    process.stdout.write(JSON.stringify(frame) + "\n");
  } catch {
    // stdout closed (parent died); main loop exits via stdin 'close'
  }
}
