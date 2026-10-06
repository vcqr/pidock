/**
 * Web DataBus: HTTP (REST) + WebSocket (live push).
 *
 * - session.list / session.events -> REST
 * - agent.* / session.create      -> POST /commands (routed to the desktop)
 *    with command_id correlation against `command_result` envelopes
 * - live events                   -> /ws/web (Redis pub/sub relay), with
 *    after_seq gap-fill on reconnect
 */
import type { DataBus, ReplayEvent } from "@pidock/ui";
import type { Envelope } from "@pidock/protocol";
import type { AuthClient } from "./auth.js";

const COMMAND_TIMEOUT_MS = 30_000;
// ephemeral kinds are push-only; deltas stay desktop-local
const GAP_FILL_AFTER_SEQ = true;

export function createWebBus(auth: AuthClient): DataBus & {
  setMachine: (machineId: string) => void;
  getMachine: () => string | null;
  onMachineStatus: (cb: (machineId: string, status: string) => void) => void;
  onRawFrame: (cb: (frame: any) => void) => void;
  onSessionUpserted: (cb: (machineId: string, session: any) => void) => void;
} {
  let activeMachine: string | null = null;
  let machineStatusCb: ((machineId: string, status: string) => void) | null = null;
  let rawFrameCb: ((frame: any) => void) | null = null;
  let sessionUpsertedCb: ((machineId: string, session: any) => void) | null = null;
  let eventHandler: ((e: Envelope) => void) | null = null;
  let ws: WebSocket | null = null;
  let wsOpen = false;
  let closedByUs = false;
  const lastSeqBySession = new Map<string, number>();
  const pendingCommands = new Map<string, { resolve: (v: any) => void; reject: (e: Error) => void }>();

  function trackSeq(e: Envelope): void {
    if (e.persist && typeof e.seq === "number") {
      const prev = lastSeqBySession.get(e.session_id) ?? 0;
      if (e.seq > prev) lastSeqBySession.set(e.session_id, e.seq);
    }
  }

  function handleFrame(frame: any): void {
    rawFrameCb?.(frame);
    if (frame?.ctrl === "machine_status") {
      machineStatusCb?.(frame.machine_id, frame.status);
      return;
    }
    if (frame?.ctrl === "session_upserted") {
      sessionUpsertedCb?.(frame.machine_id, frame.session);
      return;
    }
    if (frame?.envelope) {
      const env = frame.envelope as Envelope;
      trackSeq(env);
      if (env.kind === "command_result") {
        const commandId = (env.payload as any)?.command_id;
        const pending = commandId ? pendingCommands.get(commandId) : undefined;
        if (pending) {
          pendingCommands.delete(commandId);
          const ok = (env.payload as any)?.ok === true;
          ok
            ? pending.resolve((env.payload as any).result)
            : pending.reject(new Error((env.payload as any)?.error ?? "command failed"));
        }
        // still forward for visibility
        eventHandler?.(env);
        return;
      }
      eventHandler?.(env);
    }
  }

  function connectWs(): void {
    if (ws) return;
    const url = `${auth.serverUrl.replace(/^http/, "ws")}/ws/web?token=${auth.token}`;
    const socket = new WebSocket(url);
    ws = socket;
    socket.onopen = () => {
      wsOpen = true;
      // gap-fill: refetch everything after the last seen seq per session
      if (GAP_FILL_AFTER_SEQ && eventHandler) {
        for (const [sessionId, seq] of lastSeqBySession) {
          if (!activeMachine || seq <= 0) continue;
          auth
            .request(`/machines/${activeMachine}/sessions/${sessionId}/events?token=${auth.token}&after_seq=${seq}`)
            .then((r) => {
              for (const ev of r.events ?? []) {
                eventHandler?.({
                  event_id: "",
                  session_id: sessionId,
                  seq: ev.seq,
                  persist: true,
                  ts: ev.ts,
                  kind: ev.kind,
                  payload: ev.payload,
                });
              }
            })
            .catch(() => {});
        }
      }
    };
    socket.onmessage = (msg) => {
      try {
        handleFrame(JSON.parse(String(msg.data)));
      } catch {
        // ignore bad frames
      }
    };
    socket.onclose = () => {
      ws = null;
      wsOpen = false;
      if (!closedByUs) {
        setTimeout(connectWs, 2000);
      }
    };
    socket.onerror = () => socket.close();
  }

  async function command(sessionId: string, type: string, payload: unknown): Promise<any> {
    if (!activeMachine) throw new Error("未选择机器");
    const res = await auth.request("/commands", {
      method: "POST",
      body: JSON.stringify({
        token: auth.token,
        machine_id: activeMachine,
        session_id: sessionId,
        type,
        payload,
      }),
    });
    if (res.status === "refused_offline") {
      throw new Error("机器离线，无法远程控制");
    }
    if (res.status !== "sent" || !res.command_id) {
      throw new Error(res.error ?? "command failed");
    }
    // correlate with the desktop's command_result envelope
    return await new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        pendingCommands.delete(res.command_id);
        reject(new Error("命令超时（桌面端未响应）"));
      }, COMMAND_TIMEOUT_MS);
      pendingCommands.set(res.command_id, {
        resolve: (v) => {
          clearTimeout(timer);
          resolve(v);
        },
        reject: (e) => {
          clearTimeout(timer);
          reject(e);
        },
      });
    });
  }

  return {
    transport: "Web · 远程",

    async request(method: string, params: any): Promise<any> {
      switch (method) {
        case "session.list": {
          if (!activeMachine) return { sessions: [] };
          const r = await auth.request(`/machines/${activeMachine}/sessions?token=${auth.token}`);
          const sessions = (r.sessions ?? []).map((s: any) => ({
            session_id: s.session_id,
            file: "",
            cwd: s.cwd || s.title || "",
            name: s.title || undefined,
            model: s.model || undefined,
            created_at: s.created_at,
            open: true, // never trigger session.open over the web
            state: s.status === "running" ? "responding" : s.status === "idle" ? "idle" : s.status,
          }));
          return { sessions };
        }
        case "session.events": {
          if (!activeMachine) return { events: [] };
          const q = new URLSearchParams({ token: auth.token });
          if (params?.after_seq !== undefined) q.set("after_seq", String(params.after_seq));
          const r = await auth.request(
            `/machines/${activeMachine}/sessions/${params.session_id}/events?${q}`,
          );
          return { events: r.events ?? [] };
        }
        case "session.create": {
          const result = await command(params?.session_id ?? "", "session.create", {
            cwd: params?.cwd,
            model: params?.model,
          });
          return result ?? {};
        }
        case "session.rename": {
          await command(params?.session_id ?? "", "session.rename", { name: params?.name ?? "" });
          return { ok: true };
        }
        case "session.resolve_approval": {
          await command(params?.session_id ?? "", "session.resolve_approval", {
            approval_id: params?.approval_id,
            approved: params?.approved === true,
          });
          return { ok: true };
        }
        case "session.resolve_ask": {
          await command(params?.session_id ?? "", "session.resolve_ask", {
            ask_id: params?.ask_id,
            labels: params?.labels,
            text: params?.text,
            interrupted: params?.interrupted,
          });
          return { ok: true };
        }
        case "workspace.files": {
          const result = await command(params?.session_id ?? "", "workspace.files", { cwd: params?.cwd });
          return result ?? {};
        }
        case "workspace.read_file": {
          const result = await command(params?.session_id ?? "", "workspace.read_file", {
            cwd: params?.cwd,
            path: params?.path,
          });
          return result ?? {};
        }
        case "session.remove": {
          // 先命令桌面端删 host 注册表条目（权威数据），再清 server 侧镜像；机器离线时整体拒绝
          const ids: string[] = Array.isArray(params?.session_ids) ? params.session_ids : [];
          const result = await command(params?.session_id ?? "", "session.remove", { session_ids: ids });
          if (activeMachine && ids.length) {
            await Promise.allSettled(
              ids.map((sid) =>
                auth.request(`/machines/${activeMachine}/sessions/${sid}?token=${auth.token}`, {
                  method: "DELETE",
                }),
              ),
            );
          }
          return result ?? { removed: ids.length };
        }
        case "agent.prompt":
          await command(params.session_id, "agent.prompt", { text: params.text });
          return { accepted: true };
        case "agent.steer":
          await command(params.session_id, "agent.steer", { text: params.text });
          return { accepted: true };
        case "agent.follow_up":
          await command(params.session_id, "agent.follow_up", { text: params.text });
          return { accepted: true };
        case "agent.abort":
          await command(params.session_id, "agent.abort", {});
          return { aborted: true };
        case "agent.context_usage":
        case "agent.compact":
        case "agent.clear_queue":
        case "agent.abort_retry":
        case "session.export":
        case "session.set_auto_compaction":
        case "session.set_auto_retry":
        case "session.list_tools":
        case "session.set_active_tools":
        case "session.tree":
        case "session.navigate_tree":
        case "config.prompts.list":
        case "session.thinking_info":
        case "config.reload_runtime":
        case "session.trust": {
          // 新一代会话级命令：桌面端按同名 method 透传给 host
          const { session_id, ...payload } = params ?? {};
          const result = await command(session_id ?? "", method, payload);
          return result ?? {};
        }
        case "config.get":
          // config is a desktop-local concept in v1
          throw new Error("配置管理仅在桌面端可用");
        default:
          throw new Error(`web bus: unsupported method "${method}"`);
      }
    },

    async onEvent(handler: (e: Envelope) => void): Promise<() => void> {
      eventHandler = handler;
      connectWs();
      return () => {
        closedByUs = true;
        ws?.close();
      };
    },

    async loadHistory(sessionId: string, afterSeq?: number): Promise<ReplayEvent[]> {
      if (!activeMachine) return [];
      const q = new URLSearchParams({ token: auth.token });
      if (afterSeq !== undefined) q.set("after_seq", String(afterSeq));
      const r = await auth.request(
        `/machines/${activeMachine}/sessions/${sessionId}/events?${q}`,
      );
      return r.events ?? [];
    },

    async loadAttachment(attachmentId: string): Promise<string> {
      const r = await auth.request(`/attachments/${attachmentId}/url?token=${auth.token}`);
      const res = await fetch(r.url);
      if (!res.ok) throw new Error(`附件下载失败 (${res.status})`);
      return await res.text();
    },

    setMachine(machineId: string): void {
      if (machineId !== activeMachine) {
        activeMachine = machineId;
        lastSeqBySession.clear();
      }
    },
    getMachine(): string | null {
      return activeMachine;
    },
    onMachineStatus(cb: (machineId: string, status: string) => void): void {
      machineStatusCb = cb;
    },
    onRawFrame(cb: (frame: any) => void): void {
      rawFrameCb = cb;
    },
    onSessionUpserted(cb: (machineId: string, session: any) => void): void {
      sessionUpsertedCb = cb;
    },
  };
}

export type WebBus = ReturnType<typeof createWebBus>;
