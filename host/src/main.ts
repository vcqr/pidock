import { createInterface } from "node:readline";
import { ConfigService } from "./config.js";
import { emitError, emitResponse } from "./emit.js";
import { ExpertsService, readAvatarFile } from "./experts.js";
import { HOST_VERSION, RpcError, SessionPool } from "./pool.js";
import { Method } from "@pidock/protocol";

const pool = new SessionPool();
const config = new ConfigService(pool);
const experts = new ExpertsService();

type Handler = (params: any) => Promise<unknown> | unknown;

const handlers: Record<string, Handler> = {
  [Method.PING]: () => ({
    host_version: HOST_VERSION,
    sessions_open: pool.listSessions().sessions.filter((s) => s.open).length,
  }),
  [Method.SESSION_CREATE]: (p) => pool.createSession(p),
  [Method.SESSION_LIST]: () => pool.listSessions(),
  [Method.SESSION_OPEN]: (p) => pool.openSession(p),
  [Method.SESSION_CLOSE]: (p) => pool.closeSession(p),
  [Method.SESSION_RENAME]: (p) => pool.renameSession(p),
  [Method.SESSION_REMOVE]: (p) => pool.removeSessions(p),
  [Method.WORKSPACE_FILES]: (p) => pool.listWorkspaceFiles(p),  [Method.WORKSPACE_READ_FILE]: (p) => pool.readWorkspaceFile(p),
  [Method.SESSION_EVENTS]: (p) => pool.replayEvents(p),
  [Method.SESSION_SET_PERMISSION_MODE]: (p) => pool.setPermissionMode(p),
  [Method.SESSION_RESOLVE_APPROVAL]: (p) => pool.resolveApproval(p),
  [Method.SESSION_RESOLVE_ASK]: (p) => pool.resolveAsk(p),
  [Method.SESSION_SET_THINKING_LEVEL]: (p) => pool.setThinkingLevel(p),
  [Method.SESSION_SET_MODEL]: (p) => pool.setModel(p),
  [Method.SESSION_FILE_CHANGES]: (p) => pool.fileChanges(p),
  [Method.SESSION_FILE_DIFF]: (p) => pool.fileDiff(p),
  [Method.SESSION_REVERT_FILES]: (p) => pool.revertFiles(p),
  [Method.AGENT_PROMPT]: (p) => pool.prompt(p),
  [Method.AGENT_STEER]: (p) => pool.steer(p),
  [Method.AGENT_FOLLOW_UP]: (p) => pool.followUp(p),
  [Method.AGENT_ABORT]: (p) => pool.abort(p),
  [Method.CONFIG_GET]: () => config.get(),
  [Method.CONFIG_SETTINGS_SET]: (p) => config.set(p),
  [Method.CONFIG_PROVIDERS_LIST]: () => config.providersList(),
  [Method.CONFIG_PROVIDER_SET_KEY]: (p) => config.providerSetKey(p),
  [Method.CONFIG_PROVIDER_REMOVE_KEY]: (p) => config.providerRemoveKey(p),
  [Method.CONFIG_MODELS_LIST]: () => config.modelsList(),
  [Method.CONFIG_MODELS_SET_DEFAULT]: (p) => config.modelsSetDefault(p),
  [Method.MODEL_OVERRIDE_SET]: (p) => config.modelOverrideSet(p),
  [Method.CONFIG_EXTENSIONS_LIST]: (p) => config.extensionsList(p),
  [Method.CONFIG_EXTENSIONS_TOGGLE]: (p) => config.extensionsToggle(p),
  [Method.CONFIG_EXTENSION_READ]: (p) => config.extensionsRead(p),
  [Method.CONFIG_SKILLS_LIST]: (p) => config.skillsList(p),
  [Method.CONFIG_SKILLS_TOGGLE]: (p) => config.skillsToggle(p),
  [Method.CONFIG_SKILL_FILES]: (p) => config.skillsFiles(p),
  [Method.CONFIG_SKILL_READ]: (p) => config.skillsRead(p),
  [Method.CONFIG_SKILLS_INSTALL]: (p) => config.skillsInstall(p),
  [Method.CONFIG_EXTENSIONS_INSTALL]: (p) => config.extensionsInstall(p),
  [Method.CONFIG_MCP_GET]: () => config.mcpGet(),
  [Method.CONFIG_MCP_SET]: (p) => config.mcpSet(p),
  [Method.CONFIG_AGENTS_READ]: () => config.agentsRead(),
  [Method.CONFIG_AGENTS_WRITE]: (p) => config.agentsWrite(p),
  [Method.STATS_USAGE]: (p) => pool.usageStats(p),
  [Method.PIDOCK_SETTINGS_GET]: () => config.appSettingsGet(),
  [Method.PIDOCK_SETTINGS_SET]: (p) => config.appSettingsSet(p),
  [Method.CONFIG_PROVIDERS_CUSTOM_GET]: () => config.customProvidersGet(),
  [Method.CONFIG_PROVIDERS_CUSTOM_SET]: (p) => config.customProvidersSet(p),
  [Method.CONFIG_PROVIDERS_CUSTOM_REMOVE]: (p) => config.customProvidersRemove(p),
  [Method.CONFIG_PROVIDERS_FETCH_MODELS]: (p) => config.fetchProviderModels(p),
  [Method.ATTACHMENT_GET]: (p) => config.attachmentGet(p),
  [Method.EXPERTS_LIST]: () => experts.list(),
  [Method.EXPERTS_GET]: (p) => experts.get(p),
  [Method.EXPERTS_SAVE]: (p) => experts.save(p),
  [Method.EXPERTS_DELETE]: (p) => experts.delete(p),
  [Method.EXPERTS_INSTALL_RESOURCE]: (p) => experts.installResource(p),
  [Method.EXPERTS_PRIVATE_LIST]: (p) => experts.privateList(p),
  [Method.EXPERTS_REMOVE_RESOURCE]: (p) => experts.removeResource(p),
  [Method.EXPERTS_READ_AVATAR_FILE]: (p) => readAvatarFile(String(p?.srcPath ?? "")),
};

async function dispatch(line: string): Promise<void> {
  let request: { id?: string; method?: string; params?: unknown };
  try {
    request = JSON.parse(line);
  } catch {
    emitFatalOut("bad_frame", `unparseable line: ${line.slice(0, 200)}`);
    return;
  }
  const { id, method, params } = request;
  if (!id || typeof method !== "string") {
    if (id) emitError(id, "bad_request", "missing method");
    return;
  }
  const handler = handlers[method];
  if (!handler) {
    emitError(id, "unknown_method", `no handler for "${method}"`);
    return;
  }
  try {
    const result = await handler(params ?? {});
    emitResponse(id, result);
  } catch (err) {
    if (err instanceof RpcError) {
      emitError(id, err.code, err.message);
    } else {
      emitError(id, "internal", String(err instanceof Error ? err.message ?? err.stack : err));
    }
  }
}

function emitFatalOut(code: string, message: string): void {
  emitError("", code, message);
}

function main(): void {
  // Extensions may console.log — stdout is the protocol channel, so divert all
  // console output to stderr before the SDK (and any extension) loads.
  for (const level of ["log", "info", "warn", "error", "debug"] as const) {
    console[level] = (...args: unknown[]) =>
      process.stderr.write(`[host:console] ${args.map(String).join(" ")}\n`);
  }

  const rl = createInterface({ input: process.stdin, crlfDelay: Infinity });
  rl.on("line", (line) => {
    const trimmed = line.trim();
    if (trimmed) void dispatch(trimmed);
  });
  rl.on("close", () => {
    pool.disposeAll();
    // let pending stdout writes flush, then exit
    setTimeout(() => process.exit(0), 50);
  });
  process.on("SIGINT", () => {
    pool.disposeAll();
    process.exit(0);
  });
  // readiness banner (structured, not stdout-frame; goes to stderr for logs)
  process.stderr.write(`pidock-host ${HOST_VERSION} ready\n`);
}

main();
