<script setup lang="ts">
import { onMounted, ref } from "vue";
import { appConfirm, Icon } from "@pidock/ui";
import { ApiError, validEmail, type ApiClient } from "../auth.js";

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

// 注册策略：全局开关（关闭后仅管理员在此建号）
const allowRegister = ref(true);
const savingPolicy = ref(false);
const savedPolicy = ref(false);

// 后台建号
const showAdd = ref(false);
const newEmail = ref("");
const newPassword = ref("");
const newRole = ref<"user" | "admin">("user");
const creating = ref(false);
const notice = ref<string | null>(null);
let noticeTimer: ReturnType<typeof setTimeout> | undefined;

onMounted(load);

async function load(): Promise<void> {
  loading.value = true;
  error.value = null;
  try {
    const [r, p] = await Promise.all([
      props.client.request(`/admin/users`),
      props.client.request(`/admin/auth/policy`),
    ]);
    users.value = r.users ?? [];
    allowRegister.value = p.config?.allow_register ?? true;
  } catch (err) {
    error.value = err instanceof ApiError ? err.message : String(err);
  } finally {
    loading.value = false;
  }
}

function flash(msg: string): void {
  notice.value = msg;
  if (noticeTimer) clearTimeout(noticeTimer);
  noticeTimer = setTimeout(() => (notice.value = ""), 2500);
}

async function savePolicy(): Promise<void> {
  savingPolicy.value = true;
  error.value = null;
  try {
    await props.client.request(`/admin/auth/policy`, {
      method: "PUT",
      body: JSON.stringify({ token: props.client.token, allow_register: allowRegister.value }),
    });
    savedPolicy.value = true;
    setTimeout(() => (savedPolicy.value = false), 2000);
  } catch (err) {
    error.value = err instanceof ApiError ? err.message : String(err);
  } finally {
    savingPolicy.value = false;
  }
}

async function create(): Promise<void> {
  creating.value = true;
  error.value = null;
  try {
    const r = await props.client.request(`/admin/users`, {
      method: "POST",
      body: JSON.stringify({
        token: props.client.token,
        email: newEmail.value,
        password: newPassword.value,
        role: newRole.value,
      }),
    });
    showAdd.value = false;
    newEmail.value = "";
    newPassword.value = "";
    newRole.value = "user";
    flash(`已创建账号「${r.email}」（${r.role === "admin" ? "管理员" : "用户"}）`);
    await load();
  } catch (err) {
    error.value = err instanceof ApiError ? err.message : String(err);
  } finally {
    creating.value = false;
  }
}

async function patch(id: string, body: Record<string, unknown>): Promise<void> {
  busyId.value = id;
  error.value = null;
  try {
    await props.client.request(`/admin/users/${encodeURIComponent(id)}`, {
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
    await props.client.request(`/admin/users/${encodeURIComponent(u.user_id)}`, {
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
    <div v-if="notice" class="usr-notice">{{ notice }}</div>

    <!-- 注册策略：全局开关 -->
    <div class="policy-row">
      <label class="switch">
        <input v-model="allowRegister" type="checkbox" />
        开放注册（凭邀请码）
      </label>
      <button class="op" :disabled="savingPolicy" @click="savePolicy">
        {{ savingPolicy ? "保存中…" : savedPolicy ? "已保存 ✓" : "保存" }}
      </button>
      <span class="policy-hint">关闭后注册入口隐藏，账号由管理员在此创建</span>
    </div>

    <!-- 后台建号 -->
    <div class="add-row">
      <button v-if="!showAdd" class="op" @click="showAdd = true">添加账号</button>
      <div v-else class="add-form">
        <input v-model="newEmail" placeholder="邮箱" spellcheck="false" />
        <input
          v-model="newPassword"
          type="password"
          placeholder="初始密码（8~128 位，含字母和数字）"
          autocomplete="new-password"
        />
        <select v-model="newRole">
          <option value="user">用户</option>
          <option value="admin">管理员</option>
        </select>
        <button
          class="op"
          :disabled="
            creating ||
            !newEmail.trim() ||
            !validEmail(newEmail) ||
            newPassword.length < 8 ||
            newPassword.length > 128 ||
            !/[a-zA-Z]/.test(newPassword) ||
            !/\d/.test(newPassword)
          "
          @click="create"
        >
          {{ creating ? "创建中…" : "创建" }}
        </button>
        <button class="op" @click="showAdd = false">取消</button>
      </div>
    </div>

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
.usr-notice {
  color: var(--pd-green);
  font-size: calc(12px * var(--pd-font-scale));
  padding: 8px 0;
}
.policy-row {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 9px 12px;
  background: var(--pd-bg);
  border: 1px solid var(--pd-border);
  border-radius: 8px;
  margin-top: 10px;
}
.policy-row .switch {
  display: flex;
  align-items: center;
  gap: 6px;
  color: var(--pd-text);
  font-size: calc(12px * var(--pd-font-scale));
  cursor: pointer;
}
.policy-hint {
  color: var(--pd-text-4);
  font-size: calc(10.5px * var(--pd-font-scale));
  margin-left: auto;
}
.add-row {
  margin-top: 8px;
}
.add-form {
  display: flex;
  gap: 8px;
  align-items: center;
}
.add-form input,
.add-form select {
  background: var(--pd-bg);
  border: 1px solid var(--pd-border);
  border-radius: 8px;
  color: var(--pd-text);
  padding: 5px 9px;
  font-size: calc(12px * var(--pd-font-scale));
}
.add-form input {
  flex: 1;
  min-width: 0;
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
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
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
  white-space: nowrap;
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
/* 窄屏：单行 flex 装不下（邮箱被压成一字一行、操作按钮溢出），改为两行布局 */
@media (max-width: 640px) {
  .policy-row {
    flex-wrap: wrap;
    row-gap: 6px;
  }
  .policy-row .op {
    margin-left: auto;
  }
  .policy-hint {
    flex-basis: 100%;
    margin-left: 0;
  }
  .add-form {
    flex-wrap: wrap;
  }
  .add-form input {
    flex: 1 1 100%;
  }
  .add-form select {
    flex: 1;
  }
  .usr-row {
    flex-wrap: wrap;
    row-gap: 6px;
  }
  .who {
    flex: 1 1 100%;
  }
  .dates {
    flex: 1 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .ops {
    margin-left: auto;
  }
}
</style>
