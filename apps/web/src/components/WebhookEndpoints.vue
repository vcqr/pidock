<script setup lang="ts">
import { nextTick, onMounted, ref, watch } from "vue";
import { Icon, appConfirm } from "@pidock/ui";
import { ApiError, type ApiClient } from "../auth.js";

interface EndpointInfo {
  id: string;
  name: string;
  machine_id: string;
  machine_hostname: string;
  session_policy: string;
  fixed_session_id: string;
  expert_id: string;
  model: string;
  thinking_level: string;
  permission_mode: string;
  workspace: string;
  callback_url: string;
  callback_secret: string;
  enabled: boolean;
  created_at: string;
}

interface MachineInfo {
  machine_id: string;
  hostname: string;
  online: boolean;
}

const props = defineProps<{ client: ApiClient }>();

/** 与 server 侧 / core PERMISSION_MODES 对齐 */
const PERM_OPTIONS = [
  { value: "full", label: "完全访问（无人值守推荐）" },
  { value: "edit-auto", label: "编辑自动放行" },
  { value: "confirm", label: "每次确认" },
  { value: "plan", label: "计划模式（只读）" },
];
const THINK_OPTIONS = ["off", "minimal", "low", "medium", "high"];
const POLICY_LABELS: Record<string, string> = {
  new: "每次新建",
  latest: "最近活跃",
  fixed: "固定会话",
};

const endpoints = ref<EndpointInfo[]>([]);
const machines = ref<MachineInfo[]>([]);
const experts = ref<Array<{ id: string; name: string }>>([]);
const loading = ref(true);
const error = ref<string | null>(null);
const busyId = ref("");
const showForm = ref(false);
const creating = ref(false);

// 创建表单
const fName = ref("");
const fMachine = ref("");
const fPolicy = ref<"new" | "latest" | "fixed">("new");
const fFixedSid = ref("");
const fExpert = ref("");
const fModel = ref("");
const fThink = ref("");
const fPerm = ref("full");
const fWorkspace = ref("");
const fCallback = ref("");
const expertsLoading = ref(false);

/** 新端点令牌明文只显示一次（重新生成令牌同样只显示一次） */
const fresh = ref<{ token: string; name: string; curl: string; callback_url: string; callback_secret: string } | null>(null);
const copied = ref(false);
let copyTimer: ReturnType<typeof setTimeout> | undefined;
/** 编辑模式的目标端点；null = 新建 */
const editingId = ref<string | null>(null);
/** 编辑回填期间抑制 fMachine watch 清空已选专家 */
let populating = false;
/** 展开详情的端点 id */
const expandedId = ref("");
const copiedSecretId = ref("");
let secretTimer: ReturnType<typeof setTimeout> | undefined;

onMounted(async () => {
  await Promise.all([load(), loadMachines()]);
});

async function load(): Promise<void> {
  loading.value = true;
  error.value = null;
  try {
    const r = await props.client.request(`/webhooks`);
    endpoints.value = r.endpoints ?? [];
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

function buildCurl(token: string): string {
  return `curl -X POST ${window.location.origin}/webhooks/invoke \\\n  -H "Authorization: Bearer ${token}" \\\n  -H "Content-Type: application/json" \\\n  -d '{"text": "你好，请检查昨晚的构建日志"}'`;
}

function openAdd(): void {
  editingId.value = null;
  fName.value = "";
  fMachine.value = "";
  fPolicy.value = "new";
  fFixedSid.value = "";
  fExpert.value = "";
  fModel.value = "";
  fThink.value = "";
  fPerm.value = "full";
  fWorkspace.value = "";
  fCallback.value = "";
  showForm.value = true;
}

function openEdit(t: EndpointInfo): void {
  populating = true;
  editingId.value = t.id;
  fName.value = t.name;
  fMachine.value = t.machine_id;
  fPolicy.value = (["new", "latest", "fixed"].includes(t.session_policy) ? t.session_policy : "latest") as
    | "new"
    | "latest"
    | "fixed";
  fFixedSid.value = t.fixed_session_id;
  fExpert.value = t.expert_id;
  fModel.value = t.model;
  fThink.value = t.thinking_level;
  fPerm.value = t.permission_mode || "full";
  fWorkspace.value = t.workspace;
  fCallback.value = t.callback_url;
  showForm.value = true;
  // watch(fMachine) 是 pre-flush：先于 nextTick 执行，此刻 populating 仍为 true
  nextTick(() => (populating = false));
}

/** 新建 POST / 编辑 PATCH 共用一套字段 */
async function save(): Promise<void> {
  if (!fName.value.trim() || !fMachine.value) return;
  creating.value = true;
  error.value = null;
  copied.value = false;
  const body = JSON.stringify({
    name: fName.value,
    machine_id: fMachine.value,
    session_policy: fPolicy.value,
    fixed_session_id: fFixedSession(),
    expert_id: fExpert.value,
    model: fModel.value,
    thinking_level: fThink.value,
    permission_mode: fPerm.value,
    workspace: fWorkspace.value,
    callback_url: fCallback.value,
  });
  try {
    if (editingId.value) {
      await props.client.request(`/webhooks/${encodeURIComponent(editingId.value)}`, {
        method: "PATCH",
        body,
      });
      showForm.value = false;
    } else {
      const r = await props.client.request(`/webhooks`, { method: "POST", body });
      fresh.value = {
        token: r.token,
        name: r.name,
        curl: buildCurl(r.token),
        callback_url: r.callback_url ?? "",
        callback_secret: r.callback_secret ?? "",
      };
      showForm.value = false;
    }
    await load();
  } catch (err) {
    error.value = err instanceof ApiError ? err.message : String(err);
  } finally {
    creating.value = false;
  }
}

function fFixedSession(): string {
  return fPolicy.value === "fixed" ? fFixedSid.value.trim() : "";
}

/** 令牌明文只存哈希无法找回：丢失时换新 secret，配置不变 */
async function regenerate(t: EndpointInfo): Promise<void> {
  if (
    !(await appConfirm({
      title: `重新生成「${t.name}」的令牌？`,
      message: "旧令牌立即失效，正在使用的外部系统需要更换新令牌。",
      danger: true,
    }))
  )
    return;
  busyId.value = t.id;
  error.value = null;
  copied.value = false;
  try {
    const r = await props.client.request(`/webhooks/${encodeURIComponent(t.id)}/regenerate`, {
      method: "POST",
    });
    fresh.value = { token: r.token, name: t.name, curl: buildCurl(r.token), callback_url: "", callback_secret: "" };
    await load();
  } catch (err) {
    error.value = err instanceof ApiError ? err.message : String(err);
  } finally {
    busyId.value = "";
  }
}

async function toggle(t: EndpointInfo): Promise<void> {
  busyId.value = t.id;
  error.value = null;
  try {
    await props.client.request(`/webhooks/${encodeURIComponent(t.id)}`, {
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

async function remove(t: EndpointInfo): Promise<void> {
  if (
    !(await appConfirm({
      title: `删除 webhook 端点「${t.name}」？`,
      message: "使用该端点令牌的外部系统将立即无法调用。",
      danger: true,
    }))
  )
    return;
  busyId.value = t.id;
  error.value = null;
  try {
    await props.client.request(`/webhooks/${encodeURIComponent(t.id)}`, {
      method: "DELETE",
    });
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

async function copy(): Promise<void> {
  if (!fresh.value) return;
  if (await copyText(fresh.value.token)) {
    copied.value = true;
    if (copyTimer) clearTimeout(copyTimer);
    copyTimer = setTimeout(() => (copied.value = false), 1500);
  }
}

async function copySecret(t: EndpointInfo): Promise<void> {
  if (!(await copyText(t.callback_secret))) return;
  copiedSecretId.value = t.id;
  if (secretTimer) clearTimeout(secretTimer);
  secretTimer = setTimeout(() => (copiedSecretId.value = ""), 1500);
}

function fmtDate(s: string): string {
  const d = new Date(s);
  return Number.isNaN(d.getTime()) ? s : d.toLocaleDateString();
}
</script>

<template>
  <div class="wh-wrap">
    <div class="wh-inner">
      <div class="wh-titlerow">
        <b>Webhook 端点</b>
        <span class="sub">让外部系统凭令牌远程给节点发消息；目标会话与专家由端点配置决定</span>
        <button class="ghost new-btn" @click="showForm ? (showForm = false) : openAdd()">
          {{ showForm ? "收起" : "新建端点" }}
        </button>
      </div>

      <!-- 创建表单 -->
      <div v-if="showForm" class="wh-form">
        <div class="f-grid">
          <div class="f-field">
            <label>端点名称</label>
            <input v-model="fName" placeholder="例如「CI 失败通知」" maxlength="64" spellcheck="false" />
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
          <div class="f-field f-wide">
            <label>完成回调 URL（可选）</label>
            <input v-model="fCallback" placeholder="https://…，agent 处理完该轮后 POST 结果到此地址" spellcheck="false" />
          </div>
        </div>
        <p v-if="fPerm !== 'full'" class="f-hint warn">
          「{{ fPerm }}」模式下修改类工具会挂起等审批，无人值守的外部触发建议用「完全访问」。
        </p>
        <div class="f-actions">
          <span class="f-hint">
            专家 / 模型 / 思考级别随每次新建的会话绑定；专家自带技能与工具白名单，请确认授权范围。
          </span>
          <div class="f-btns">
            <button class="ghost" @click="showForm = false">取消</button>
            <button class="primary" :disabled="creating || !fName.trim() || !fMachine" @click="save">
              {{ creating ? (editingId ? "保存中…" : "创建中…") : editingId ? "保存修改" : "创建端点" }}
            </button>
          </div>
        </div>
      </div>

      <div v-if="error" class="wh-error">{{ error }}</div>

      <!-- 新端点令牌（仅一次） -->
      <div v-if="fresh" class="wh-fresh">
        <div class="fresh-tip">
          请立即复制「{{ fresh.name }}」的调用示例，关闭后将无法再次查看令牌：
        </div>
        <div class="fresh-row">
          <code class="mono">{{ fresh.token }}</code>
          <button class="ghost" @click="copy">{{ copied ? "已复制 ✓" : "复制令牌" }}</button>
          <button class="ghost" title="我已保存" @click="fresh = null">✕</button>
        </div>
        <pre class="fresh-curl mono">{{ fresh.curl }}</pre>
        <template v-if="fresh.callback_url">
          <div class="fresh-tip">
            配置了回调：agent 完成后 server 会 POST 到 <code class="mono">{{ fresh.callback_url }}</code
            >，签名头 <code class="mono">X-PiDock-Signature: sha256=HMAC(body)</code>，验签密钥（请保存）：
          </div>
          <div class="fresh-row">
            <code class="mono">{{ fresh.callback_secret }}</code>
          </div>
        </template>
      </div>

      <div class="wh-list">
        <div v-if="loading" class="wh-empty">加载中…</div>
        <div v-else-if="!endpoints.length" class="wh-empty">
          还没有 webhook 端点。创建一个，把 curl 示例贴进 CI 或告警系统即可远程驱动节点。
        </div>
        <template v-else>
          <template v-for="t in endpoints" :key="t.id">
            <div class="wh-item">
              <div class="wh-row" :class="{ expanded: expandedId === t.id }">
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
                  <span class="chip">{{ t.machine_hostname || "未知节点" }}</span>
                  <span class="chip">{{ POLICY_LABELS[t.session_policy] ?? t.session_policy }}</span>
                  <span v-if="t.expert_id" class="chip accent">专家</span>
                  <span v-if="t.callback_url" class="chip accent" :title="t.callback_url">回调</span>
                  <span v-if="!t.enabled" class="chip dead">已停用</span>
                </div>
                <span class="tdate">创建于 {{ fmtDate(t.created_at) }}</span>
                <button class="toggle" @click="openEdit(t)">编辑</button>
                <button class="toggle" :disabled="busyId === t.id" @click="toggle(t)">
                  {{ t.enabled ? "停用" : "启用" }}
                </button>
                <button class="revoke" :disabled="busyId === t.id" @click="remove(t)">删除</button>
              </div>
              <div v-if="expandedId === t.id" class="wh-detail">
                <div class="d-grid">
                  <span class="d-k">目标节点</span>
                  <span class="d-v">{{ t.machine_hostname || t.machine_id }}</span>
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
                  <span class="d-k">完成回调</span>
                  <span class="d-v">{{ t.callback_url || "未配置" }}</span>
                  <span class="d-k">回调签名密钥</span>
                  <span class="d-v mono secret">
                    {{ t.callback_secret || "—" }}
                    <button v-if="t.callback_secret" class="ghost" @click="copySecret(t)">
                      {{ copiedSecretId === t.id ? "已复制 ✓" : "复制" }}
                    </button>
                  </span>
                  <span class="d-k">端点 ID</span>
                  <span class="d-v mono">{{ t.id }}</span>
                </div>
                <div class="d-actions">
                  <span class="f-hint">调用令牌明文不保存；丢失或泄露时从这里换新，配置不变。</span>
                  <button class="ghost" :disabled="busyId === t.id" @click="regenerate(t)">重新生成令牌</button>
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
.wh-wrap {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 16px 28px;
}
.wh-inner {
  max-width: 720px;
  margin: 0 auto;
  display: flex;
  flex-direction: column;
  gap: 12px;
}
.wh-titlerow {
  display: flex;
  align-items: baseline;
  gap: 10px;
  flex-wrap: wrap;
}
.wh-titlerow b {
  color: var(--pd-text);
  font-size: calc(15px * var(--pd-font-scale));
  flex: none;
}
.wh-titlerow .sub {
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
.wh-form {
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
.f-hint.warn {
  color: var(--pd-yellow-text, #d8a253);
}
.f-actions {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
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
.wh-error {
  color: var(--pd-red-text, #e5484d);
  font-size: calc(12px * var(--pd-font-scale));
}
.wh-fresh {
  border: 1px solid var(--pd-accent);
  border-radius: 10px;
  padding: 12px 14px;
  background: var(--pd-bg-panel);
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.fresh-tip {
  color: var(--pd-text-2);
  font-size: calc(12px * var(--pd-font-scale));
}
.fresh-row {
  display: flex;
  align-items: center;
  gap: 8px;
}
.fresh-row code {
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--pd-accent);
  font-size: calc(12.5px * var(--pd-font-scale));
  letter-spacing: 0.5px;
}
.fresh-curl {
  margin: 0;
  padding: 10px 12px;
  background: var(--pd-bg-card);
  border: 1px solid var(--pd-border);
  border-radius: 8px;
  color: var(--pd-text-2);
  font-size: calc(11px * var(--pd-font-scale));
  overflow-x: auto;
  white-space: pre;
}
.mono {
  font-family: ui-monospace, monospace;
}
.wh-list {
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.wh-item {
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
.wh-row.expanded {
  border-radius: 8px 8px 0 0;
}
.wh-detail {
  border: 1px solid var(--pd-border);
  border-top: none;
  border-radius: 0 0 8px 8px;
  background: var(--pd-bg-card);
  padding: 12px 14px;
  display: flex;
  flex-direction: column;
  gap: 10px;
}
.d-grid {
  display: grid;
  grid-template-columns: 110px 1fr;
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
.d-v.mono,
.mono {
  font-family: ui-monospace, monospace;
}
.secret {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}
.d-actions {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  border-top: 1px solid var(--pd-border);
  padding-top: 10px;
}
.f-btns {
  display: flex;
  gap: 8px;
  flex: none;
}
.wh-empty {
  color: var(--pd-text-4);
  font-size: calc(12px * var(--pd-font-scale));
  padding: 24px 0;
  text-align: center;
}
.wh-row {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 9px 12px;
  background: var(--pd-bg-panel);
  border: 1px solid var(--pd-border);
  border-radius: 8px;
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
