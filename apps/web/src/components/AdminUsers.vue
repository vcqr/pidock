<script setup lang="ts">
import { onMounted, ref } from "vue";
import { appConfirm, Icon } from "@pidock/ui";
import { ApiError, type ApiClient } from "../auth.js";

interface AdminUser {
  user_id: string;
  email: string;
  role: string;
  auth_source: string;
  disabled: boolean;
  display_name: string;
  created_at: string;
  last_login_at: string;
}

const props = defineProps<{ client: ApiClient }>();

const users = ref<AdminUser[]>([]);
const loading = ref(true);
const error = ref<string | null>(null);
const busyId = ref("");

onMounted(load);

async function load(): Promise<void> {
  loading.value = true;
  error.value = null;
  try {
    const r = await props.client.request(`/admin/users?token=${props.client.token}`);
    users.value = r.users ?? [];
  } catch (err) {
    error.value = err instanceof ApiError ? err.message : String(err);
  } finally {
    loading.value = false;
  }
}

async function patch(id: string, body: Record<string, unknown>): Promise<void> {
  busyId.value = id;
  error.value = null;
  try {
    await props.client.request(`/admin/users/${encodeURIComponent(id)}?token=${props.client.token}`, {
      method: "PATCH",
      body: JSON.stringify({ token: props.client.token, ...body }),
    });
    await load();
  } catch (err) {
    error.value = err instanceof ApiError ? err.message : String(err);
  } finally {
    busyId.value = "";
  }
}

async function toggleRole(u: AdminUser): Promise<void> {
  await patch(u.user_id, { role: u.role === "admin" ? "user" : "admin" });
}

async function toggleDisabled(u: AdminUser): Promise<void> {
  await patch(u.user_id, { disabled: !u.disabled });
}

async function remove(u: AdminUser): Promise<void> {
  if (
    !(await appConfirm({
      title: `删除用户「${u.email}」？`,
      message: "仅删除账号记录；该用户的节点与会话数据保留。此操作不可恢复。",
      danger: true,
    }))
  )
    return;
  busyId.value = u.user_id;
  error.value = null;
  try {
    await props.client.request(`/admin/users/${encodeURIComponent(u.user_id)}?token=${props.client.token}`, {
      method: "DELETE",
    });
    await load();
  } catch (err) {
    error.value = err instanceof ApiError ? err.message : String(err);
  } finally {
    busyId.value = "";
  }
}

function sourceLabel(u: AdminUser): string {
  if (u.auth_source === "ldap") return "LDAP";
  if (u.auth_source === "oidc") return "SSO";
  return "本地";
}

function fmtDate(s: string): string {
  if (!s) return "—";
  const d = new Date(s);
  return Number.isNaN(d.getTime()) ? s : d.toLocaleDateString();
}
</script>

<template>
  <div class="usr-pane">
    <div class="pane-head">
      <div class="pane-title">
        <span class="sub">共 {{ users.length }} 个账号；至少保留一个管理员</span>
      </div>
      <button class="ghost refresh" title="刷新" @click="load">
        <Icon name="refresh-line" :size="14" />
      </button>
    </div>

    <div v-if="error" class="usr-error">{{ error }}</div>

    <div class="usr-list">
      <div v-if="loading" class="usr-empty">加载中…</div>
      <div v-else-if="!users.length" class="usr-empty">还没有用户。</div>
      <div v-for="u in users" v-else :key="u.user_id" class="usr-row" :class="{ off: u.disabled }">
        <div class="who">
          <span class="email">{{ u.email }}<span v-if="u.user_id === props.client.userId" class="me">（我）</span></span>
          <span v-if="u.display_name" class="dname">{{ u.display_name }}</span>
        </div>
        <span class="chip" :class="u.role === 'admin' ? 'adm' : 'norm'">
          {{ u.role === "admin" ? "管理员" : "用户" }}
        </span>
        <span class="chip src">{{ sourceLabel(u) }}</span>
        <span v-if="u.disabled" class="chip dead">已禁用</span>
        <span class="dates" :title="`注册 ${u.created_at || '—'}`">
          {{ fmtDate(u.created_at) }} 注册 · 上次登录 {{ fmtDate(u.last_login_at) }}
        </span>
        <div class="ops">
          <button class="op" :disabled="busyId === u.user_id" @click="toggleRole(u)">
            {{ u.role === "admin" ? "撤销管理员" : "设为管理员" }}
          </button>
          <button class="op" :disabled="busyId === u.user_id" @click="toggleDisabled(u)">
            {{ u.disabled ? "启用" : "禁用" }}
          </button>
          <button class="op danger" :disabled="busyId === u.user_id" @click="remove(u)">
            <Icon name="delete-bin-line" :size="13" />
          </button>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.usr-pane {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
}
.pane-head {
  display: flex;
  align-items: center;
  gap: 10px;
  padding-bottom: 12px;
  border-bottom: 1px solid var(--pd-border);
}
.pane-title {
  display: flex;
  align-items: baseline;
  gap: 10px;
  flex: 1;
}
.pane-title b {
  color: var(--pd-text);
  font-size: calc(15px * var(--pd-font-scale));
}
.pane-title .sub {
  color: var(--pd-text-4);
  font-size: calc(11px * var(--pd-font-scale));
}
.refresh {
  align-self: center;
}
.usr-error {
  color: var(--pd-red-text, #e5484d);
  font-size: calc(12px * var(--pd-font-scale));
  padding: 8px 0;
}
.usr-list {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding-top: 10px;
}
.usr-empty {
  color: var(--pd-text-4);
  font-size: calc(12px * var(--pd-font-scale));
  padding: 18px 0;
  text-align: center;
}
.usr-row {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px 10px;
  background: var(--pd-bg);
  border: 1px solid var(--pd-border);
  border-radius: 8px;
}
.usr-row.off .who .email {
  color: var(--pd-text-4);
  text-decoration: line-through;
}
.who {
  display: flex;
  flex-direction: column;
  gap: 1px;
  flex: 1;
  min-width: 0;
}
.email {
  color: var(--pd-text);
  font-size: calc(12.5px * var(--pd-font-scale));
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.me {
  color: var(--pd-text-4);
  font-size: calc(10.5px * var(--pd-font-scale));
}
.dname {
  color: var(--pd-text-4);
  font-size: calc(10.5px * var(--pd-font-scale));
}
.chip {
  flex: none;
  font-size: calc(10.5px * var(--pd-font-scale));
  padding: 1px 8px;
  border-radius: 999px;
  border: 1px solid var(--pd-border);
  color: var(--pd-text-3);
}
.chip.adm {
  color: var(--pd-accent);
  border-color: var(--pd-accent);
}
.chip.dead {
  color: var(--pd-red-text, #e5484d);
  border-color: var(--pd-red-text, #e5484d);
}
.dates {
  flex: none;
  color: var(--pd-text-4);
  font-size: calc(10.5px * var(--pd-font-scale));
}
.ops {
  flex: none;
  display: flex;
  gap: 6px;
}
.op {
  background: transparent;
  border: 1px solid var(--pd-border);
  color: var(--pd-text-2);
  border-radius: 6px;
  font-size: calc(11px * var(--pd-font-scale));
  padding: 2px 8px;
  cursor: pointer;
}
.op:hover {
  border-color: var(--pd-accent);
  color: var(--pd-accent);
}
.op.danger:hover {
  border-color: var(--pd-red-text, #e5484d);
  color: var(--pd-red-text, #e5484d);
}
.op:disabled {
  color: var(--pd-text-4);
  cursor: not-allowed;
}
</style>
