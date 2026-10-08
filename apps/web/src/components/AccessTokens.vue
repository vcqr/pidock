<script setup lang="ts">
import { onMounted, ref } from "vue";
import { appConfirm } from "@pidock/ui";
import { ApiError, type ApiClient } from "../auth.js";

interface PatInfo {
  id: string;
  name: string;
  days: number;
  expires_at: string;
  created_at: string;
}

const props = defineProps<{ client: ApiClient }>();

/** 有效期选项（与 server EXPIRY_CHOICES 对齐；0 = 不过期） */
const EXPIRY_OPTIONS = [
  { days: 0, label: "不过期" },
  { days: 3, label: "3 天" },
  { days: 15, label: "15 天" },
  { days: 30, label: "30 天" },
  { days: 90, label: "90 天" },
  { days: 180, label: "180 天" },
  { days: 360, label: "360 天" },
];

const tokens = ref<PatInfo[]>([]);
const loading = ref(true);
const error = ref<string | null>(null);
const busyId = ref("");
const newName = ref("");
const newDays = ref(0);
const creating = ref(false);
/** 新令牌明文只显示一次 */
const freshToken = ref<{ token: string; name: string; expires: string } | null>(null);
const copied = ref(false);
let copyTimer: ReturnType<typeof setTimeout> | undefined;

onMounted(load);

async function load(): Promise<void> {
  loading.value = true;
  error.value = null;
  try {
    const r = await props.client.request(`/me/tokens?token=${props.client.token}`);
    tokens.value = r.tokens ?? [];
  } catch (err) {
    error.value = err instanceof ApiError ? err.message : String(err);
  } finally {
    loading.value = false;
  }
}

async function create(): Promise<void> {
  creating.value = true;
  error.value = null;
  copied.value = false;
  try {
    const r = await props.client.request(`/me/tokens?token=${props.client.token}`, {
      method: "POST",
      body: JSON.stringify({ token: props.client.token, name: newName.value, days: newDays.value }),
    });
    freshToken.value = {
      token: r.token,
      name: r.name,
      expires: r.expires_at ? `有效期至 ${fmtDate(r.expires_at)}` : "有效期：不过期",
    };
    newName.value = "";
    await load();
  } catch (err) {
    error.value = err instanceof ApiError ? err.message : String(err);
  } finally {
    creating.value = false;
  }
}

function expired(t: PatInfo): boolean {
  return !!t.expires_at && new Date(t.expires_at).getTime() < Date.now();
}

async function revoke(t: PatInfo): Promise<void> {
  if (
    !(await appConfirm({
      title: `撤销令牌「${t.name}」？`,
      message: "使用该令牌的设备将立即失去同步能力，需要重新配置。",
      danger: true,
    }))
  )
    return;
  busyId.value = t.id;
  error.value = null;
  try {
    await props.client.request(`/me/tokens/${encodeURIComponent(t.id)}?token=${props.client.token}`, {
      method: "DELETE",
    });
    await load();
  } catch (err) {
    error.value = err instanceof ApiError ? err.message : String(err);
  } finally {
    busyId.value = "";
  }
}

async function copy(): Promise<void> {
  if (!freshToken.value) return;
  try {
    if (navigator.clipboard && window.isSecureContext) {
      await navigator.clipboard.writeText(freshToken.value.token);
    } else {
      const ta = document.createElement("textarea");
      ta.value = freshToken.value.token;
      ta.style.position = "fixed";
      ta.style.opacity = "0";
      document.body.appendChild(ta);
      ta.select();
      document.execCommand("copy");
      ta.remove();
    }
    copied.value = true;
    if (copyTimer) clearTimeout(copyTimer);
    copyTimer = setTimeout(() => (copied.value = false), 1500);
  } catch {
    /* 剪贴板不可用时静默 */
  }
}

function fmtDate(s: string): string {
  const d = new Date(s);
  return Number.isNaN(d.getTime()) ? s : d.toLocaleDateString();
}
</script>

<template>
  <div class="pat-wrap">
    <div class="pat-inner">
      <div class="pat-titlerow">
        <b>访问令牌</b>
        <span class="sub">给桌面端云同步等长期场景使用；明文只在创建时显示一次</span>
      </div>

      <div class="pat-create">
        <input
          v-model="newName"
          placeholder="令牌名称，如「办公室台式机」"
          maxlength="64"
          spellcheck="false"
          @keydown.enter="create"
        />
        <select v-model.number="newDays" class="expiry">
          <option v-for="o in EXPIRY_OPTIONS" :key="o.days" :value="o.days">{{ o.label }}</option>
        </select>
        <button class="primary" :disabled="creating || !newName.trim()" @click="create">
          {{ creating ? "创建中…" : "创建令牌" }}
        </button>
      </div>

      <div v-if="error" class="pat-error">{{ error }}</div>

      <!-- 新令牌明文（仅一次） -->
      <div v-if="freshToken" class="pat-fresh">
        <div class="fresh-tip">
          请立即复制「{{ freshToken.name }}」的令牌，关闭后将无法再次查看（{{ freshToken.expires }}）：
        </div>
        <div class="fresh-row">
          <code class="mono">{{ freshToken.token }}</code>
          <button class="ghost" @click="copy">{{ copied ? "已复制 ✓" : "复制" }}</button>
          <button class="ghost" title="我已保存" @click="freshToken = null">✕</button>
        </div>
      </div>

      <div class="pat-list">
        <div v-if="loading" class="pat-empty">加载中…</div>
        <div v-else-if="!tokens.length" class="pat-empty">
          还没有访问令牌。创建一个，粘贴到桌面端「云同步」即可免密登录。
        </div>
        <div v-for="t in tokens" v-else :key="t.id" class="pat-row">
          <span class="tname">{{ t.name }}</span>
          <span v-if="expired(t)" class="chip dead">已过期</span>
          <span class="tdate">到期：{{ t.expires_at ? fmtDate(t.expires_at) : "不过期" }}</span>
          <button class="revoke" :disabled="busyId === t.id" @click="revoke(t)">撤销</button>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.pat-wrap {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 16px 28px;
}
.pat-inner {
  max-width: 720px;
  margin: 0 auto;
  display: flex;
  flex-direction: column;
  gap: 12px;
}
.pat-titlerow {
  display: flex;
  align-items: baseline;
  gap: 10px;
}
.pat-titlerow b {
  color: var(--pd-text);
  font-size: calc(15px * var(--pd-font-scale));
}
.pat-titlerow .sub {
  color: var(--pd-text-4);
  font-size: calc(11px * var(--pd-font-scale));
}
.pat-create {
  display: flex;
  gap: 10px;
}
.pat-create input,
.pat-create .expiry {
  background: var(--pd-bg-panel);
  border: 1px solid var(--pd-border);
  border-radius: 8px;
  color: var(--pd-text);
  padding: 7px 10px;
  font-size: calc(12.5px * var(--pd-font-scale));
}
.pat-create input {
  flex: 1;
  min-width: 0;
}
.pat-error {
  color: var(--pd-red-text, #e5484d);
  font-size: calc(12px * var(--pd-font-scale));
}
.pat-fresh {
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
.mono {
  font-family: ui-monospace, monospace;
}
.pat-list {
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.pat-empty {
  color: var(--pd-text-4);
  font-size: calc(12px * var(--pd-font-scale));
  padding: 24px 0;
  text-align: center;
}
.pat-row {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 9px 12px;
  background: var(--pd-bg-panel);
  border: 1px solid var(--pd-border);
  border-radius: 8px;
}
.tname {
  flex: 1;
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
}
.chip.dead {
  color: var(--pd-red-text, #e5484d);
  border-color: var(--pd-red-text, #e5484d);
}
.revoke {
  flex: none;
  background: transparent;
  border: 1px solid var(--pd-border);
  color: var(--pd-red-text, #e5484d);
  border-radius: 6px;
  font-size: calc(11px * var(--pd-font-scale));
  padding: 2px 8px;
  cursor: pointer;
}
.revoke:disabled {
  color: var(--pd-text-4);
  cursor: not-allowed;
}
</style>
