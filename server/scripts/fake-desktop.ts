/**
 * M4 end-to-end pipeline test against a running PiDock server:
 *
 *   register/login (HTTP) -> fake desktop WS (register machine, envelopes,
 *   heartbeats) -> wait for ingest (Kafka) -> web client checks machines /
 *   sessions / history over HTTP + live push over /ws/web -> send a command.
 *
 * Usage:  bun run server/scripts/fake-desktop.ts  [http://localhost:8080]
 * (run from WSL or Windows; the server must be reachable)
 */
import WebSocket from "ws";

const BASE = process.argv[2] ?? "http://localhost:8080";
const WS_BASE = BASE.replace(/^http/, "ws");
const failures: string[] = [];
const check = (name: string, cond: boolean, detail = "") => {
  console.log(`${cond ? "  ✔" : "  ✘"} ${name}${detail ? ` — ${detail}` : ""}`);
  if (!cond) failures.push(name);
};
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function api(method: string, path: string, token?: string, body?: unknown): Promise<any> {
  const res = await fetch(`${BASE}${path}`, {
    method,
    headers: {
      "content-type": "application/json",
      ...(token ? { authorization: `Bearer ${token}` } : {}),
    },
    body: body ? JSON.stringify(body) : undefined,
  });
  const json = await res.json().catch(() => ({}));
  if (!res.ok && !json?.error) throw new Error(`${method} ${path} -> ${res.status}`);
  return json;
}

function connectWs(url: string): Promise<WebSocket> {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(url);
    ws.on("open", () => resolve(ws));
    ws.on("error", reject);
  });
}

function nextMessage(ws: WebSocket, timeoutMs = 8000): Promise<any> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("ws message timeout")), timeoutMs);
    ws.once("message", (data) => {
      clearTimeout(timer);
      resolve(JSON.parse(data.toString()));
    });
  });
}

async function main(): Promise<number> {
  console.log(`== PiDock M4 e2e against ${BASE} ==`);

  const health = await api("GET", "/health");
  check("health", health.ok === true, `pipeline=${health.pipeline}`);

  // --- auth
  const email = `e2e-${Date.now()}@pidock.test`;
  const password = "e2e-password-123";
  await api("POST", "/auth/register", undefined, { email, password });
  const login = await api("POST", "/auth/login", undefined, { email, password });
  const token = login.access_token;
  check("register + login", typeof token === "string");
  const me = await api("GET", `/me?token=${token}`);
  check("/me", typeof me.user_id === "string");
  const refreshed = await api("POST", "/auth/refresh", undefined, { refresh_token: login.refresh_token });
  check("token refresh", typeof refreshed.access_token === "string");

  // --- fake desktop: machine + session + envelopes
  const wsDesktop = await connectWs(`${WS_BASE}/ws/desktop?token=${token}`);
  const machineId = `e2e-machine-${Date.now()}`;
  const sessionId = `e2e-session-${Date.now()}`;
  wsDesktop.send(
    JSON.stringify({ ctrl: "register", machine_id: machineId, hostname: "e2e-host", os: "linux", version: "0.1.0" }),
  );
  const registered = await nextMessage(wsDesktop);
  check("desktop register ack", registered.ctrl === "registered" && registered.machine_id === machineId);

  const envelope = (seq: number, kind: string, payload: any, persist = true) => ({
    event_id: crypto.randomUUID(),
    session_id: sessionId,
    ...(persist ? { seq } : {}),
    persist,
    ts: new Date().toISOString(),
    kind,
    payload,
  });

  wsDesktop.send(JSON.stringify(envelope(1, "message_complete", {
    role: "user",
    blocks: [{ type: "text", text: "e2e 提问：1+1=?" }],
    entry_id: "e1",
  })));
  wsDesktop.send(JSON.stringify(envelope(2, "message_complete", {
    role: "assistant",
    blocks: [
      { type: "thinking", text: "thinking..." },
      { type: "text", text: "1+1等于2。" },
    ],
    provider: "e2e",
    model: "e2e-model",
    entry_id: "e2",
    message_id: `${sessionId}#1`,
  })));

  // --- web client: live push subscription before events are ingested
  const wsWeb = await connectWs(`${WS_BASE}/ws/web?token=${token}`);

  // poll HTTP until ingest has landed the events (Kafka round-trip)
  let history: any[] = [];
  for (let i = 0; i < 30; i++) {
    await sleep(1000);
    const h = await api("GET", `/machines/${machineId}/sessions/${sessionId}/events?token=${token}`);
    history = h.events ?? [];
    if (history.length >= 2) break;
  }
  check("history via HTTP (kafka -> mongo)", history.length >= 2, `${history.length} events`);
  check("history ordered by seq", history[0]?.seq === 1 && history[1]?.seq === 2);

  const machines = await api("GET", `/machines?token=${token}`);
  const m = machines.machines?.find((x: any) => x.machine_id === machineId);
  check("machine online in /machines", m?.online === true);

  const sessions = await api("GET", `/machines/${machineId}/sessions?token=${token}`);
  const s = sessions.sessions?.find((x: any) => x.session_id === sessionId);
  check("session row exists with status", s !== undefined && s.status === "running", s?.status);

  // --- web live push: ingest of a new event should arrive over /ws/web
  const pushPromise = nextMessage(wsWeb, 10000);
  wsDesktop.send(JSON.stringify(envelope(3, "message_complete", {
    role: "user",
    blocks: [{ type: "text", text: "第二条" }],
    entry_id: "e3",
  })));
  try {
    const pushed = await pushPromise;
    check("live push via /ws/web", pushed?.envelope?.session_id === sessionId || pushed?.kind === "message_complete");
  } catch {
    check("live push via /ws/web", false, "no push received");
  }

  // --- command routing: web -> server -> desktop
  const cmdPromise = nextMessage(wsDesktop, 8000);
  const sent = await api("POST", "/commands", undefined, {
    token,
    machine_id: machineId,
    session_id: sessionId,
    type: "agent.abort",
    payload: {},
  });
  check("POST /commands", sent.status === "sent", sent.command_id?.slice(0, 8));
  try {
    const cmd = await cmdPromise;
    check("command routed to desktop", cmd.ctrl === "command" && cmd.command?.type === "agent.abort");
  } catch {
    check("command routed to desktop", false, "no command received");
  }

  // --- attachments: presign -> upload -> authorized download
  const { createHash } = await import("node:crypto");
  const big = "x".repeat(300 * 1024) + `-${Date.now()}`;
  const sha = createHash("sha256").update(big).digest("hex");
  const pres = await api("POST", "/attachments/presign", undefined, {
    token,
    sha256: sha,
    size: Buffer.byteLength(big),
  });
  check("presign (content addressed)", pres.attachment_id === sha && typeof pres.upload_url === "string");
  const put = await fetch(pres.upload_url, { method: "PUT", body: big });
  check("attachment upload (presigned PUT)", put.ok, `status ${put.status}`);
  const urlRes = await api("GET", `/attachments/${sha}/url?token=${token}`);
  const fetched = await (await fetch(urlRes.url)).text();
  check("attachment download round-trip", fetched === big);

  // ownership: another account must not read it
  const other = `e2e-other-${Date.now()}@pidock.test`;
  await api("POST", "/auth/register", undefined, { email: other, password: "other-password-123" });
  const otherLogin = await api("POST", "/auth/login", undefined, { email: other, password: "other-password-123" });
  const denied = await fetch(`${BASE}/attachments/${sha}/url?token=${otherLogin.access_token}`);
  check("attachment ownership enforced", denied.status === 403, `status ${denied.status}`);

  // --- compaction resync: history is cleared and rebuilt from scratch
  const cSession = `e2e-compaction-${Date.now()}`;
  const env = (seq: number, text: string, entryId: string) => ({
    event_id: crypto.randomUUID(),
    session_id: cSession,
    seq,
    persist: true,
    ts: new Date().toISOString(),
    kind: "message_complete",
    payload: { role: "user", blocks: [{ type: "text", text }], entry_id: entryId },
  });
  for (const [seq, text, eid] of [[1, "旧消息一", "c1"], [2, "旧消息二", "c2"], [3, "旧消息三", "c3"]]) {
    wsDesktop.send(JSON.stringify(env(seq, text, eid)));
  }
  let cHistory: any[] = [];
  for (let i = 0; i < 30; i++) {
    await sleep(1000);
    cHistory = (await api("GET", `/machines/${machineId}/sessions/${cSession}/events?token=${token}`)).events ?? [];
    if (cHistory.length >= 3) break;
  }
  check("compaction pre-state (3 events)", cHistory.length === 3, `${cHistory.length} events`);

  // pi compacts: history shrinks — host emits the resync marker then re-drains
  wsDesktop.send(JSON.stringify({
    event_id: crypto.randomUUID(),
    session_id: cSession,
    persist: false,
    ts: new Date().toISOString(),
    kind: "session_resynced",
    payload: { reason: "compaction" },
  }));
  for (const [seq, text, eid] of [[1, "压缩后摘要", "n1"], [2, "压缩后新消息", "n2"]]) {
    wsDesktop.send(JSON.stringify(env(seq, text, eid)));
  }
  let rebuilt: any[] = [];
  for (let i = 0; i < 30; i++) {
    await sleep(1000);
    rebuilt = (await api("GET", `/machines/${machineId}/sessions/${cSession}/events?token=${token}`)).events ?? [];
    if (rebuilt.length === 2 && rebuilt[0]?.payload?.blocks?.[0]?.text === "压缩后摘要") break;
  }
  check("compaction resync rebuilds history", rebuilt.length === 2 && rebuilt[0]?.payload?.blocks?.[0]?.text === "压缩后摘要", `${rebuilt.length} events after resync`);

  // command to an offline machine is refused
  const offline = await api("POST", "/commands", undefined, {
    token,
    machine_id: "no-such-machine",
    session_id: sessionId,
    type: "agent.abort",
    payload: {},
  });
  check("offline machine refused", offline.status === "refused_offline");

  wsDesktop.close();
  wsWeb.close();

  // after close, machine should go offline (pub/sub may lag; poll /machines)
  for (let i = 0; i < 10; i++) {
    await sleep(500);
    const mm = await api("GET", `/machines?token=${token}`);
    const now = mm.machines?.find((x: any) => x.machine_id === machineId);
    if (now?.online === false) break;
  }
  const machines2 = await api("GET", `/machines?token=${token}`);
  const m2 = machines2.machines?.find((x: any) => x.machine_id === machineId);
  check("machine offline after WS close", m2?.online === false);

  console.log("== summary ==");
  if (failures.length) {
    console.log(`FAIL (${failures.length}): ${failures.join("; ")}`);
    return 1;
  }
  console.log("PASS — full pipeline green");
  return 0;
}

main()
  .then((code) => process.exit(code))
  .catch((err) => {
    console.error("e2e crashed:", err);
    process.exit(1);
  });
