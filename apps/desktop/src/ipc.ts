/**
 * 统一 IPC 层：同一套命令面，两种传输。
 * - Tauri 桌面壳：@tauri-apps/api 的 invoke / pidock:event 事件；
 * - 浏览器（webhost 模式）：`/ws` JSON 帧协议，语义与 Tauri 完全一致
 *   （`{t:"req",id,cmd,args}` → `{t:"res",id,ok,result|error}`，事件
 *   `{t:"event",envelope}` 对应 `pidock:event`）。
 */
import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen as tauriListen } from "@tauri-apps/api/event";
import type { Envelope } from "@pidock/protocol";

export const isTauri =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

// ------------------------------------------------------------- browser WS ---

const TOKEN_KEY = "pidock.webToken";

type Pending = { resolve: (v: unknown) => void; reject: (e: string) => void };

let ws: WebSocket | null = null;
let connecting: Promise<WebSocket> | null = null;
let reqSeq = 0;
const pending = new Map<string, Pending>();
const eventHandlers = new Set<(e: Envelope) => void>();
let reconnectTimer: ReturnType<typeof setTimeout> | undefined;
let askingToken = false;

function wsUrl(): string {
  const proto = location.protocol === "https:" ? "wss" : "ws";
  const token = localStorage.getItem(TOKEN_KEY) ?? "";
  return `${proto}://${location.host}/ws?token=${encodeURIComponent(token)}`;
}

function failPending(err: string): void {
  for (const [, p] of pending) p.reject(err);
  pending.clear();
}

function scheduleReconnect(delayMs = 1500): void {
  clearTimeout(reconnectTimer);
  reconnectTimer = setTimeout(() => {
    void connect().catch(() => {
      // 连不上继续退避重试（服务重启场景）
      scheduleReconnect();
    });
  }, delayMs);
}

function promptForToken(): void {
  if (askingToken) return;
  askingToken = true;
  // 服务端设置了 PIDOCK_WEB_TOKEN：询问一次并重试（取消则保持未授权状态）
  const t = window.prompt("该服务已开启访问令牌（PIDOCK_WEB_TOKEN），请输入：");
  askingToken = false;
  if (t && t.trim()) {
    localStorage.setItem(TOKEN_KEY, t.trim());
    scheduleReconnect(0);
  }
}

function connect(): Promise<WebSocket> {
  if (ws && ws.readyState === WebSocket.OPEN) return Promise.resolve(ws);
  if (connecting) return connecting;
  let opened = false;
  connecting = new Promise((resolve, reject) => {
    const socket = new WebSocket(wsUrl());
    socket.onopen = () => {
      opened = true;
      ws = socket;
      connecting = null;
      resolve(socket);
    };
    socket.onmessage = (ev) => {
      let frame: any;
      try {
        frame = JSON.parse(ev.data as string);
      } catch {
        return;
      }
      if (frame?.t === "res" && frame.id != null) {
        const p = pending.get(String(frame.id));
        if (p) {
          pending.delete(String(frame.id));
          if (frame.ok) p.resolve(frame.result);
          else p.reject(String(frame.error ?? "unknown error"));
        }
      } else if (frame?.t === "event" && frame.envelope) {
        for (const h of eventHandlers) h(frame.envelope as Envelope);
      }
    };
    socket.onclose = (ev) => {
      if (ws === socket) ws = null;
      connecting = null;
      failPending(
        ev.code === 4401
          ? "服务需要访问令牌（PIDOCK_WEB_TOKEN）"
          : `与服务的连接已断开（code ${ev.code}）`,
      );
      if (!opened) {
        reject(new Error(`无法连接服务（code ${ev.code}）`));
        return;
      }
      if (ev.code === 4401) {
        promptForToken(); // 令牌错误：不自动重连，询问后手动重试
        return;
      }
      if (eventHandlers.size > 0 || pending.size > 0) {
        scheduleReconnect();
      }
    };
  });
  return connecting;
}

// ------------------------------------------------------------------- ipc ---

export async function ipc<T = unknown>(
  cmd: string,
  args?: Record<string, unknown>,
): Promise<T> {
  if (isTauri) {
    return (await tauriInvoke(cmd, args as never)) as T;
  }
  const socket = await connect();
  const id = `r${++reqSeq}`;
  return new Promise<T>((resolve, reject) => {
    pending.set(id, {
      resolve: resolve as (v: unknown) => void,
      reject: (e: string) => reject(new Error(e)),
    });
    socket.send(JSON.stringify({ t: "req", id, cmd, args: args ?? {} }));
  });
}

export async function onPidockEvent(
  handler: (e: Envelope) => void,
): Promise<() => void> {
  if (isTauri) {
    return tauriListen<Envelope>("pidock:event", (ev) => handler(ev.payload));
  }
  eventHandlers.add(handler);
  void connect().catch(() => {}); // 有订阅者才真正建连
  return () => {
    eventHandlers.delete(handler);
  };
}
