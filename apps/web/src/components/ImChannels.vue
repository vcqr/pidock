<script setup lang="ts">
import { nextTick, onMounted, ref, watch } from "vue";
import { Icon, appConfirm } from "@pidock/ui";
import { ApiError, type ApiClient } from "../auth.js";

interface ChannelInfo {
  id: string;
  platform: string;
  name: string;
  app_id: string;
  app_secret: string;
  encrypt_key: string;
  verification_token: string;
  machine_id: string;
  machine_hostname: string;
  session_policy: string;
  fixed_session_id: string;
  expert_id: string;
  model: string;
  thinking_level: string;
  permission_mode: string;
  workspace: string;
  enabled: boolean;
  created_at: string;
}

interface MachineInfo {
  machine_id: string;
  hostname: string;
  online: boolean;
}

const props = defineProps<{ client: ApiClient }>();

/** 与 server 侧 / core PERMISSION_MODES 对齐；IM 默认 edit-auto（触发方不可控） */
const PERM_OPTIONS = [
  { value: "edit-auto", label: "编辑自动放行（IM 推荐）" },
  { value: "plan", label: "计划模式（只读）" },
  { value: "confirm", label: "每次确认" },
  { value: "full", label: "完全访问" },
];
const THINK_OPTIONS = ["off", "minimal", "low", "medium", "high"];
const POLICY_LABELS: Record<string, string> = {
  new: "每次新建",
  latest: "最近活跃",
  fixed: "固定会话",
};

const channels = ref<ChannelInfo[]>([]);
const machines = ref<MachineInfo[]>([]);
const experts = ref<Array<{ id: string; name: string }>>([]);
const loading = ref(true);
const error = ref<string | null>(null);
const busyId = ref("");
const showForm = ref(false);
const creating = ref(false);
/** 编辑模式的目标渠道；null = 新建 */
const editingId = ref<string | null>(null);
let populating = false;
const expandedId = ref("");

// 创建/编辑表单
const fName = ref("");
const fAppId = ref("");
const fAppSecret = ref("");
const fEncryptKey = ref("");
const fVerifyToken = ref("");
const fMachine = ref("");
const fPolicy = ref<"new" | "latest" | "fixed">("new");
const fFixedSid = ref("");
const fExpert = ref("");
const fModel = ref("");
const fThink = ref("");
const fPerm = ref("edit-auto");
const fWorkspace = ref("");
const expertsLoading = ref(false);

onMounted(async () => {
  await Promise.all([load(), loadMachines()]);
});

async function load(): Promise<void> {
  loading.value = true;
  error.value = null;
  try {
    const r = await props.client.request(`/im/channels`);
    channels.value = r.channels ?? [];
  } catch (err) {
    error.value = err instanceof ApiError ? err.message : String(err);
  } finally {
    loading.value = false;
  }
}

async function loadMachines(): Promise<void> {
  try {
    const r = await props.client.request(`/machines`);
    machines.value = r.machines ?? [];
  } catch {
    machines.value = [];
  }
}

/** 目标节点变化时拉取该节点的专家列表（经 /rpc 中继，节点须在线） */
watch(fMachine, async (mid) => {
  experts.value = [];
  if (!populating) fExpert.value = "";
  if (!mid) return;
  const m = machines.value.find((x) => x.machine_id === mid);
  if (!m?.online) return;
  expertsLoading.value = true;
  try {
    const r = await props.client.request(`/rpc`, {
      method: "POST",
      body: JSON.stringify({ machine_id: mid, type: "experts.list", payload: {} }),
    });
    experts.value = (r?.result?.experts ?? []).map((e: any) => ({
      id: String(e.id ?? ""),
      name: String(e.name ?? ""),
    }));
  } catch {
    experts.value = [];
  } finally {
    expertsLoading.value = false;
  }
});

function openAdd(): void {
  populating = false;
  editingId.value = null;
  fName.value = "";
  fAppId.value = "";
  fAppSecret.value = "";
  fEncryptKey.value = "";
  fVerifyToken.value = "";
  fMachine.value = "";
  fPolicy.value = "new";
  fFixedSid.value = "";
  fExpert.value = "";
  fModel.value = "";
  fThink.value = "";
  fPerm.value = "edit-auto";
  fWorkspace.value = "";
  showForm.value = true;
}

function openEdit(t: ChannelInfo): void {
  populating = true;
  editingId.value = t.id;
  fName.value = t.name;
  fAppId.value = t.app_id;
  fAppSecret.value = t.app_secret;
  fEncryptKey.value = t.encrypt_key;
  fVerifyToken.value = t.verification_token;
  fMachine.value = t.machine_id;
  fPolicy.value = (["new", "latest", "fixed"].includes(t.session_policy) ? t.session_policy : "latest") as
    | "new"
    | "latest"
    | "fixed";
  fFixedSid.value = t.fixed_session_id;
  fExpert.value = t.expert_id;
  fModel.value = t.model;
  fThink.value = t.thinking_level;
  fPerm.value = t.permission_mode || "edit-auto";
  fWorkspace.value = t.workspace;
  showForm.value = true;
  nextTick(() => (populating = false));
}

function formBody(): string {
  return JSON.stringify({
    platform: "feishu",
    name: fName.value,
    app_id: fAppId.value,
    app_secret: fAppSecret.value,
    encrypt_key: fEncryptKey.value,
    verification_token: fVerifyToken.value,
    machine_id: fMachine.value,
    session_policy: fPolicy.value,
    fixed_session_id: fPolicy.value === "fixed" ? fFixedSid.value.trim() : "",
    expert_id: fExpert.value,
    model: fModel.value,
    thinking_level: fThink.value,
    permission_mode: fPerm.value,
    workspace: fWorkspace.value,
  });
}

async function save(): Promise<void> {
  if (!fName.value.trim() || !fAppId.value.trim() || !fVerifyToken.value.trim() || !fMachine.value) return;
  creating.value = true;
  error.value = null;
  try {
    if (editingId.value) {
      await props.client.request(`/im/channels/${encodeURIComponent(editingId.value)}`, {
        method: "PATCH",
        body: formBody(),
      });
    } else {
      await props.client.request(`/im/channels`, { method: "POST", body: formBody() });
    }
    showForm.value = false;
    await load();
  } catch (err) {
    error.value = err instanceof ApiError ? err.message : String(err);
  } finally {
    creating.value = false;
  }
}

async function toggle(t: ChannelInfo): Promise<void> {
  busyId.value = t.id;
  error.value = null;
  try {
    await props.client.request(`/im/channels/${encodeURIComponent(t.id)}`, {
      method: "PATCH",
      body: JSON.stringify({ enabled: !t.enabled }),
    });
    await load();
  } catch (err) {
    error.value = err instanceof ApiError ? err.message : String(err);
  } finally {
    busyId.value = "";
  }
}

async function remove(t: ChannelInfo): Promise<void> {
  if (
    !(await appConfirm({
      title: `删除 IM 渠道「${t.name}」？`,
      message: "飞书侧的事件订阅将立即失效，需要重新配置。",
      danger: true,
    }))
  )
    return;
  busyId.value = t.id;
  error.value = null;
  try {
    await props.client.request(`/im/channels/${encodeURIComponent(t.id)}`, { method: "DELETE" });
    await load();
  } catch (err) {
    error.value = err instanceof ApiError ? err.message : String(err);
  } finally {
    busyId.value = "";
  }
}

async function copyText(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard && window.isSecureContext) {
      await navigator.clipboard.writeText(text);
    } else {
      const ta = document.createElement("textarea");
      ta.value = text;
      ta.style.position = "fixed";
      ta.style.opacity = "0";
      document.body.appendChild(ta);
      ta.select();
      document.execCommand("copy");
      ta.remove();
    }
    return true;
  } catch {
    return false;
  }
}

const copiedUrlId = ref("");
let urlTimer: ReturnType<typeof setTimeout> | undefined;
async function copyUrl(t: ChannelInfo): Promise<void> {
  if (!(await copyText(eventUrl(t)))) return;
  copiedUrlId.value = t.id;
  if (urlTimer) clearTimeout(urlTimer);
  urlTimer = setTimeout(() => (copiedUrlId.value = ""), 1500);
}

function eventUrl(t: ChannelInfo): string {
  return `${window.location.origin}/im/feishu/event/${t.id}`;
}

function fmtDate(s: string): string {
  const d = new Date(s);
  return Number.isNaN(d.getTime()) ? s : d.toLocaleDateString();
}
</script>

<template>
  <div class="im-wrap">
    <div class="im-inner">
      <div class="im-titlerow">
        <b>IM 渠道</b>
        <span class="sub">把飞书等 IM 群聊接入 PiDock：@机器人发消息 → 指定节点执行 → 结果回群</span>
        <button class="ghost new-btn" @click="showForm ? (showForm = false) : openAdd()">
          {{ showForm ? "收起" : "新建渠道" }}
        </button>
      </div>

      <div v-if="showForm" class="im-form">
        <div class="f-grid">
          <div class="f-field">
            <label>渠道名称</label>
            <input v-model="fName" placeholder="例如「研发群助手」" maxlength="64" spellcheck="false" />
          </div>
          <div class="f-field">
            <label>平台</label>
            <select disabled>
              <option>飞书（更多平台陆续接入）</option>
            </select>
          </div>
          <div class="f-field">
            <label>App ID</label>
            <input v-model="fAppId" placeholder="cli_xxx" spellcheck="false" />
          </div>
          <div class="f-field">
            <label>App Secret</label>
            <input v-model="fAppSecret" placeholder="开发者后台「凭证与基础信息」" spellcheck="false" />
          </div>
          <div class="f-field">
            <label>Verification Token</label>
            <input v-model="fVerifyToken" placeholder="事件与回调 → 加密策略" spellcheck="false" />
          </div>
          <div class="f-field">
            <label>Encrypt Key（可选）</label>
            <input v-model="fEncryptKey" placeholder="配置后事件加密推送，服务端自动解密验签" spellcheck="false" />
          </div>
          <div class="f-field">
            <label>目标节点</label>
            <select v-model="fMachine">
              <option value="" disabled>选择节点</option>
              <option v-for="m in machines" :key="m.machine_id" :value="m.machine_id">
                {{ m.hostname }}{{ m.online ? "" : "（离线）" }}
              </option>
            </select>
          </div>
          <div class="f-field">
            <label>会话策略</label>
            <div class="seg">
              <button :class="{ on: fPolicy === 'new' }" @click="fPolicy = 'new'">每次新建</button>
              <button :class="{ on: fPolicy === 'latest' }" @click="fPolicy = 'latest'">最近活跃</button>
              <button :class="{ on: fPolicy === 'fixed' }" @click="fPolicy = 'fixed'">固定会话</button>
            </div>
          </div>
          <div v-if="fPolicy === 'fixed'" class="f-field">
            <label>会话 ID</label>
            <input v-model="fFixedSid" placeholder="粘贴目标 session_id" spellcheck="false" />
          </div>
          <div class="f-field">
            <label>雇佣专家（可选）</label>
            <select v-model="fExpert" :disabled="!fMachine || expertsLoading">
              <option value="">
                {{ expertsLoading ? "专家加载中…" : fMachine ? "不使用专家" : "先选择节点" }}
              </option>
              <option v-if="fExpert && !experts.some((e) => e.id === fExpert)" :value="fExpert">
                当前专家（{{ fExpert.slice(0, 8) }}…，节点离线或已改名）
              </option>
              <option v-for="e in experts" :key="e.id" :value="e.id">{{ e.name }}</option>
            </select>
          </div>
          <div class="f-field">
            <label>权限模式</label>
            <select v-model="fPerm">
              <option v-for="o in PERM_OPTIONS" :key="o.value" :value="o.value">{{ o.label }}</option>
            </select>
          </div>
          <div class="f-field">
            <label>模型（可选）</label>
            <input v-model="fModel" placeholder="provider/model-id，留空 = 默认" spellcheck="false" />
          </div>
          <div class="f-field">
            <label>思考级别（可选）</label>
            <select v-model="fThink">
              <option value="">默认</option>
              <option v-for="t in THINK_OPTIONS" :key="t" :value="t">{{ t }}</option>
            </select>
          </div>
          <div class="f-field f-wide">
            <label>工作空间（可选）</label>
            <input v-model="fWorkspace" placeholder="会话工作目录，留空 = 节点主目录" spellcheck="false" />
          </div>
        </div>
        <div class="f-actions">
          <span class="f-hint">
            IM 里任何 @机器人的人都可能触发执行，权限模式请按需收紧；飞书侧需给应用开通
            「接收群聊消息」与「以应用身份发消息」权限并发布版本。
          </span>
          <div class="f-btns">
            <button class="ghost" @click="showForm = false">取消</button>
            <button
              class="primary"
              :disabled="creating || !fName.trim() || !fAppId.trim() || !fVerifyToken.trim() || !fMachine"
              @click="save"
            >
              {{ creating ? (editingId ? "保存中…" : "创建中…") : editingId ? "保存修改" : "创建渠道" }}
            </button>
          </div>
        </div>
      </div>

      <div v-if="error" class="im-error">{{ error }}</div>

      <div class="im-list">
        <div v-if="loading" class="im-empty">加载中…</div>
        <div v-else-if="!channels.length" class="im-empty">
          还没有 IM 渠道。创建一个，把事件订阅 URL 填到飞书开发者后台即可在群里驱动 Agent。
        </div>
        <template v-else>
          <template v-for="t in channels" :key="t.id">
            <div class="im-item">
              <div class="im-row" :class="{ expanded: expandedId === t.id }">
                <button
                  class="chev"
                  :class="{ open: expandedId === t.id }"
                  :title="expandedId === t.id ? '收起详情' : '展开详情'"
                  @click="expandedId = expandedId === t.id ? '' : t.id"
                >
                  <Icon name="arrow-down-s-line" :size="14" />
                </button>
                <div class="row-main">
                  <span class="tname">{{ t.name }}</span>
                  <span class="chip accent">飞书</span>
                  <span class="chip">{{ t.machine_hostname || "未知节点" }}</span>
                  <span class="chip">{{ POLICY_LABELS[t.session_policy] ?? t.session_policy }}</span>
                  <span v-if="t.expert_id" class="chip accent">专家</span>
                  <span v-if="!t.enabled" class="chip dead">已停用</span>
                </div>
                <span class="tdate">创建于 {{ fmtDate(t.created_at) }}</span>
                <button class="toggle" @click="openEdit(t)">编辑</button>
                <button class="toggle" :disabled="busyId === t.id" @click="toggle(t)">
                  {{ t.enabled ? "停用" : "启用" }}
                </button>
                <button class="revoke" :disabled="busyId === t.id" @click="remove(t)">删除</button>
              </div>
              <div v-if="expandedId === t.id" class="im-detail">
                <div class="url-row">
                  <span class="d-k">事件订阅 URL</span>
                  <code class="mono url">{{ eventUrl(t) }}</code>
                  <button class="ghost" @click="copyUrl(t)">{{ copiedUrlId === t.id ? "已复制 ✓" : "复制" }}</button>
                </div>
                <div class="d-grid">
                  <span class="d-k">App ID</span>
                  <span class="d-v mono">{{ t.app_id }}</span>
                  <span class="d-k">Verification Token</span>
                  <span class="d-v mono">{{ t.verification_token }}</span>
                  <span class="d-k">Encrypt Key</span>
                  <span class="d-v mono">{{ t.encrypt_key || "未配置（明文推送）" }}</span>
                  <span class="d-k">会话策略</span>
                  <span class="d-v">
                    {{ POLICY_LABELS[t.session_policy] ?? t.session_policy
                    }}{{ t.session_policy === "fixed" && t.fixed_session_id ? ` · ${t.fixed_session_id}` : "" }}
                  </span>
                  <span class="d-k">专家</span>
                  <span class="d-v">{{ t.expert_id || "无" }}</span>
                  <span class="d-k">模型 / 思考 / 权限</span>
                  <span class="d-v">
                    {{ t.model || "默认" }} / {{ t.thinking_level || "默认" }} / {{ t.permission_mode }}
                  </span>
                  <span class="d-k">工作空间</span>
                  <span class="d-v">{{ t.workspace || "节点主目录" }}</span>
                  <span class="d-k">渠道 ID</span>
                  <span class="d-v mono">{{ t.id }}</span>
                </div>
                <div class="d-steps">
                  <span class="f-hint">
                    接入步骤：① 开发者后台创建企业自建应用 → ② 事件与回调 → 订阅方式选「将事件发送至开发者服务器」，
                    粘贴上方 URL → ③ 开通 im:message.receive_v1 订阅与「以应用身份发消息」权限 → ④ 发布应用版本，
                    在群里 @机器人 试试。
                  </span>
                </div>
              </div>
            </div>
          </template>
        </template>
      </div>
    </div>
  </div>
</template>

<style scoped>
.im-wrap {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 16px 28px;
}
.im-inner {
  max-width: 720px;
  margin: 0 auto;
  display: flex;
  flex-direction: column;
  gap: 12px;
}
.im-titlerow {
  display: flex;
  align-items: baseline;
  gap: 10px;
  flex-wrap: wrap;
}
.im-titlerow b {
  color: var(--pd-text);
  font-size: calc(15px * var(--pd-font-scale));
  flex: none;
}
.im-titlerow .sub {
  color: var(--pd-text-4);
  font-size: calc(11px * var(--pd-font-scale));
  flex: 1;
  min-width: 0;
}
.new-btn {
  flex: none;
}
.ghost {
  background: transparent;
  border: 1px solid var(--pd-border);
  color: var(--pd-text-2);
  border-radius: 8px;
  font-size: calc(12px * var(--pd-font-scale));
  padding: 5px 12px;
  cursor: pointer;
}
.ghost:hover {
  border-color: var(--pd-accent);
  color: var(--pd-text);
}
.im-form {
  border: 1px solid var(--pd-border);
  border-radius: 10px;
  background: var(--pd-bg-panel);
  padding: 14px 16px;
  display: flex;
  flex-direction: column;
  gap: 12px;
}
.f-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 12px 14px;
}
.f-field {
  display: flex;
  flex-direction: column;
  gap: 6px;
  min-width: 0;
}
.f-wide {
  grid-column: 1 / -1;
}
.f-field label {
  color: var(--pd-text-2);
  font-size: calc(11.5px * var(--pd-font-scale));
}
.f-field input,
.f-field select {
  background: var(--pd-bg-card);
  border: 1px solid var(--pd-border);
  border-radius: 8px;
  color: var(--pd-text);
  padding: 7px 10px;
  font-size: calc(12.5px * var(--pd-font-scale));
  min-width: 0;
  width: 100%;
  box-sizing: border-box;
}
.f-field input:focus,
.f-field select:focus {
  outline: none;
  border-color: var(--pd-accent);
}
.f-field input:disabled,
.f-field select:disabled {
  opacity: 0.55;
  cursor: not-allowed;
}
.seg {
  display: flex;
  border: 1px solid var(--pd-border);
  border-radius: 8px;
  overflow: hidden;
}
.seg button {
  flex: 1;
  background: transparent;
  border: none;
  color: var(--pd-text-4);
  font-size: calc(11.5px * var(--pd-font-scale));
  padding: 7px 4px;
  cursor: pointer;
}
.seg button.on {
  background: var(--pd-accent);
  color: #fff;
}
.f-hint {
  color: var(--pd-text-4);
  font-size: calc(11px * var(--pd-font-scale));
}
.f-actions {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
}
.f-btns {
  display: flex;
  gap: 8px;
  flex: none;
}
.primary {
  background: var(--pd-accent);
  border: none;
  color: #fff;
  border-radius: 8px;
  font-size: calc(12.5px * var(--pd-font-scale));
  padding: 7px 16px;
  cursor: pointer;
}
.primary:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
.im-error {
  color: var(--pd-red-text, #e5484d);
  font-size: calc(12px * var(--pd-font-scale));
}
.im-list {
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.im-empty {
  color: var(--pd-text-4);
  font-size: calc(12px * var(--pd-font-scale));
  padding: 24px 0;
  text-align: center;
}
.im-item {
  display: flex;
  flex-direction: column;
}
.chev {
  flex: none;
  background: transparent;
  border: none;
  color: var(--pd-text-4);
  cursor: pointer;
  display: flex;
  align-items: center;
  padding: 2px;
  transition: transform 0.15s;
}
.chev.open {
  transform: rotate(180deg);
  color: var(--pd-accent);
}
.im-row {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 9px 12px;
  background: var(--pd-bg-panel);
  border: 1px solid var(--pd-border);
  border-radius: 8px;
}
.im-row.expanded {
  border-radius: 8px 8px 0 0;
}
.im-detail {
  border: 1px solid var(--pd-border);
  border-top: none;
  border-radius: 0 0 8px 8px;
  background: var(--pd-bg-card);
  padding: 12px 14px;
  display: flex;
  flex-direction: column;
  gap: 10px;
}
.url-row {
  display: flex;
  align-items: center;
  gap: 8px;
}
.url-row .d-k {
  flex: none;
}
.url-row code {
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--pd-accent);
  font-size: calc(11.5px * var(--pd-font-scale));
}
.d-grid {
  display: grid;
  grid-template-columns: 130px 1fr;
  gap: 6px 12px;
  font-size: calc(11.5px * var(--pd-font-scale));
}
.d-k {
  color: var(--pd-text-4);
}
.d-v {
  color: var(--pd-text-2);
  overflow-wrap: anywhere;
}
.mono {
  font-family: ui-monospace, monospace;
}
.d-steps {
  border-top: 1px solid var(--pd-border);
  padding-top: 10px;
}
.row-main {
  flex: 1;
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
}
.tname {
  color: var(--pd-text);
  font-size: calc(12.5px * var(--pd-font-scale));
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.tdate {
  flex: none;
  color: var(--pd-text-4);
  font-size: calc(11px * var(--pd-font-scale));
}
.chip {
  flex: none;
  font-size: calc(10.5px * var(--pd-font-scale));
  padding: 1px 8px;
  border-radius: 999px;
  border: 1px solid var(--pd-border);
  color: var(--pd-text-4);
}
.chip.accent {
  color: var(--pd-accent);
  border-color: var(--pd-accent);
}
.chip.dead {
  color: var(--pd-red-text, #e5484d);
  border-color: var(--pd-red-text, #e5484d);
}
.toggle,
.revoke {
  flex: none;
  background: transparent;
  border: 1px solid var(--pd-border);
  color: var(--pd-text-2);
  border-radius: 6px;
  font-size: calc(11px * var(--pd-font-scale));
  padding: 2px 8px;
  cursor: pointer;
}
.toggle:disabled,
.revoke:disabled {
  color: var(--pd-text-4);
  cursor: not-allowed;
}
.revoke {
  color: var(--pd-red-text, #e5484d);
}
</style>
