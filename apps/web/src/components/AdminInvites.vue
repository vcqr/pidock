<script setup lang="ts">
import { onMounted, ref } from "vue";
import { ApiError, type ApiClient } from "../auth.js";

interface Invite {
  code: string;
  created_by: string;
  created_at: string;
  max_uses: number;
  used_count: number;
  expires_at: string;
  revoked: boolean;
}

const props = defineProps<{ client: ApiClient }>();
const emit = defineEmits<{ close: [] }>();

const invites = ref<Invite[]>([]);
const loading = ref(true);
const error = ref<string | null>(null);
const maxUses = ref(1);
const expiresDays = ref(0);
const creating = ref(false);
const copiedCode = ref("");
let copyTimer: ReturnType<typeof setTimeout> | undefined;

onMounted(load);

async function load(): Promise<void> {
  loading.value = true;
  error.value = null;
  try {
    const r = await props.client.request(`/admin/invites?token=${props.client.token}`);
    invites.value = r.invites ?? [];
  } catch (err) {
    error.value = err instanceof ApiError ? err.message : String(err);
  } finally {
    loading.value = false;
  }
}

async function create(): Promise<void> {
  creating.value = true;
  error.value = null;
  try {
    const r = await props.client.request(`/admin/invites?token=${props.client.token}`, {
      method: "POST",
      body: JSON.stringify({
        max_uses: Math.max(1, Math.floor(maxUses.value) || 1),
        expires_days: Math.max(0, Math.floor(expiresDays.value) || 0),
      }),
    });
    invites.value.unshift({
      code: r.code,
      created_by: props.client.userId,
      created_at: new Date().toISOString(),
      max_uses: r.max_uses,
      used_count: 0,
      expires_at: r.expires_at ?? "",
      revoked: false,
    });
    void copy(r.code);
  } catch (err) {
    error.value = err instanceof ApiError ? err.message : String(err);
  } finally {
    creating.value = false;
  }
}

async function revoke(code: string): Promise<void> {
  try {
    await props.client.request(`/admin/invites/${encodeURIComponent(code)}?token=${props.client.token}`, {
      method: "DELETE",
    });
    invites.value = invites.value.filter((i) => i.code !== code);
  } catch (err) {
    error.value = err instanceof ApiError ? err.message : String(err);
  }
}

function status(i: Invite): { label: string; cls: string } {
  if (i.revoked) return { label: "已撤销", cls: "dead" };
  if (i.expires_at && new Date(i.expires_at).getTime() < Date.now()) return { label: "已过期", cls: "dead" };
  if (i.used_count >= i.max_uses) return { label: "已用尽", cls: "usedup" };
  return { label: "有效", cls: "ok" };
}

function fmtDate(s: string): string {
  if (!s) return "永久";
  const d = new Date(s);
  return Number.isNaN(d.getTime()) ? s : d.toLocaleDateString();
}

async function copy(code: string): Promise<void> {
  try {
    if (navigator.clipboard && window.isSecureContext) {
      await navigator.clipboard.writeText(code);
    } else {
      const ta = document.createElement("textarea");
      ta.value = code;
      ta.style.position = "fixed";
      ta.style.opacity = "0";
      document.body.appendChild(ta);
      ta.select();
      document.execCommand("copy");
      ta.remove();
    }
    copiedCode.value = code;
    if (copyTimer) clearTimeout(copyTimer);
    copyTimer = setTimeout(() => (copiedCode.value = ""), 1500);
  } catch {
    /* 剪贴板不可用时静默 */
  }
}
</script>

<template>
  <div class="inv-overlay" @click.self="emit('close')">
    <div class="inv-panel">
      <div class="inv-head">
        <b>邀请码管理</b>
        <span class="sub">注册需凭邀请码；生成后立即复制</span>
        <button class="ghost" @click="emit('close')">✕</button>
      </div>

      <div class="inv-create">
        <label>可用次数<input v-model.number="maxUses" type="number" min="1" max="1000" /></label>
        <label>有效天数<input v-model.number="expiresDays" type="number" min="0" max="3650" placeholder="0=永久" /></label>
        <button class="primary" :disabled="creating" @click="create">
          {{ creating ? "生成中…" : "生成邀请码" }}
        </button>
      </div>

      <div v-if="error" class="inv-error">{{ error }}</div>

      <div class="inv-list">
        <div v-if="loading" class="inv-empty">加载中…</div>
        <div v-else-if="!invites.length" class="inv-empty">还没有邀请码，生成一个吧。</div>
        <div v-for="i in invites" v-else :key="i.code" class="inv-row">
          <button class="code mono" :title="copiedCode === i.code ? '已复制' : '点击复制'" @click="copy(i.code)">
            {{ copiedCode === i.code ? "已复制 ✓" : i.code }}
          </button>
          <span class="usage">{{ i.used_count }}/{{ i.max_uses }}</span>
          <span class="expire">{{ fmtDate(i.expires_at) }}</span>
          <span class="chip" :class="status(i).cls">{{ status(i).label }}</span>
          <button
            v-if="!i.revoked"
            class="revoke"
            :disabled="status(i).label !== '有效'"
            title="作废该邀请码"
            @click="revoke(i.code)"
          >
            撤销
          </button>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.inv-overlay {
  position: fixed;
  inset: 0;
  z-index: 100;
  background: rgba(0, 0, 0, 0.45);
  display: grid;
  place-items: center;
}
.inv-panel {
  width: min(560px, calc(100vw - 40px));
  max-height: 80vh;
  display: flex;
  flex-direction: column;
  background: var(--pd-bg-panel);
  border: 1px solid var(--pd-border);
  border-radius: 12px;
  box-shadow: var(--pd-shadow, 0 12px 40px rgba(0, 0, 0, 0.35));
  padding: 16px 18px;
}
.inv-head {
  display: flex;
  align-items: baseline;
  gap: 10px;
  padding-bottom: 12px;
  border-bottom: 1px solid var(--pd-border);
}
.inv-head b {
  color: var(--pd-text);
  font-size: calc(14px * var(--pd-font-scale));
}
.inv-head .sub {
  color: var(--pd-text-4);
  font-size: calc(11px * var(--pd-font-scale));
  flex: 1;
}
.inv-head .ghost {
  align-self: center;
}
.inv-create {
  display: flex;
  align-items: flex-end;
  gap: 10px;
  padding: 12px 0;
}
.inv-create label {
  display: flex;
  flex-direction: column;
  gap: 4px;
  font-size: calc(11px * var(--pd-font-scale));
  color: var(--pd-text-3);
}
.inv-create input {
  width: 92px;
  background: var(--pd-bg);
  border: 1px solid var(--pd-border);
  border-radius: 8px;
  color: var(--pd-text);
  padding: 5px 8px;
  font-size: calc(12px * var(--pd-font-scale));
}
.inv-create .primary {
  margin-left: auto;
}
.inv-error {
  color: var(--pd-red-text, #e5484d);
  font-size: calc(12px * var(--pd-font-scale));
  padding-bottom: 8px;
}
.inv-list {
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.inv-empty {
  color: var(--pd-text-4);
  font-size: calc(12px * var(--pd-font-scale));
  padding: 18px 0;
  text-align: center;
}
.inv-row {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 7px 10px;
  background: var(--pd-bg);
  border: 1px solid var(--pd-border);
  border-radius: 8px;
}
.mono {
  font-family: ui-monospace, monospace;
}
.code {
  background: transparent;
  border: none;
  color: var(--pd-text);
  font-size: calc(13px * var(--pd-font-scale));
  letter-spacing: 1px;
  cursor: pointer;
  flex: 1;
  text-align: left;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.code:hover {
  color: var(--pd-accent);
}
.usage,
.expire {
  color: var(--pd-text-3);
  font-size: calc(11.5px * var(--pd-font-scale));
  flex: none;
}
.chip {
  flex: none;
  font-size: calc(10.5px * var(--pd-font-scale));
  padding: 1px 8px;
  border-radius: 999px;
  border: 1px solid var(--pd-border);
}
.chip.ok {
  color: var(--pd-green);
  border-color: var(--pd-green);
}
.chip.usedup,
.chip.dead {
  color: var(--pd-text-4);
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
