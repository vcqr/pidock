<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { ApiError, type ApiClient } from "../auth.js";

interface LdapCfg {
  enabled: boolean;
  server_url: string;
  bind_dn: string;
  bind_password: string;
  base_dn: string;
  user_filter: string;
  starttls: boolean;
  email_attr: string;
  name_attr: string;
  allow_register: boolean;
}

interface OidcCfg {
  enabled: boolean;
  issuer: string;
  client_id: string;
  client_secret: string;
  scopes: string;
  label: string;
  redirect_base: string;
  allow_register: boolean;
}

const props = defineProps<{ client: ApiClient }>();

const loading = ref(true);
const saving = ref<"" | "ldap" | "oidc">("");
const testing = ref(false);
const error = ref<string | null>(null);
const saved = ref("");

const ldap = ref<LdapCfg>(emptyLdap());
const oidc = ref<OidcCfg>(emptyOidc());
const testUser = ref("");
const testPass = ref("");
const testMsg = ref("");
const testOk = ref(false);

function emptyLdap(): LdapCfg {
  return {
    enabled: false,
    server_url: "",
    bind_dn: "",
    bind_password: "",
    base_dn: "",
    user_filter: "(uid={username})",
    starttls: false,
    email_attr: "mail",
    name_attr: "cn",
    allow_register: true,
  };
}

function emptyOidc(): OidcCfg {
  return {
    enabled: false,
    issuer: "",
    client_id: "",
    client_secret: "",
    scopes: "",
    label: "",
    redirect_base: "",
    allow_register: true,
  };
}

onMounted(load);

async function load(): Promise<void> {
  loading.value = true;
  error.value = null;
  try {
    const [l, o] = await Promise.all([
      props.client.request(`/admin/auth/ldap?token=${props.client.token}`),
      props.client.request(`/admin/auth/oidc?token=${props.client.token}`),
    ]);
    ldap.value = { ...emptyLdap(), ...(l.config ?? {}) };
    oidc.value = { ...emptyOidc(), ...(o.config ?? {}) };
  } catch (err) {
    error.value = err instanceof ApiError ? err.message : String(err);
  } finally {
    loading.value = false;
  }
}

function flash(which: string): void {
  saved.value = which;
  setTimeout(() => (saved.value = ""), 2000);
}

async function saveLdap(): Promise<void> {
  saving.value = "ldap";
  error.value = null;
  try {
    await props.client.request(`/admin/auth/ldap?token=${props.client.token}`, {
      method: "PUT",
      body: JSON.stringify({ token: props.client.token, ...ldap.value }),
    });
    flash("ldap");
  } catch (err) {
    error.value = err instanceof ApiError ? err.message : String(err);
  } finally {
    saving.value = "";
  }
}

async function saveOidc(): Promise<void> {
  saving.value = "oidc";
  error.value = null;
  try {
    await props.client.request(`/admin/auth/oidc?token=${props.client.token}`, {
      method: "PUT",
      body: JSON.stringify({ token: props.client.token, ...oidc.value }),
    });
    flash("oidc");
  } catch (err) {
    error.value = err instanceof ApiError ? err.message : String(err);
  } finally {
    saving.value = "";
  }
}

async function testLdap(): Promise<void> {
  testing.value = true;
  error.value = null;
  testMsg.value = "";
  try {
    // 用当前表单值先存一份再测试，保证测的就是所见配置
    await props.client.request(`/admin/auth/ldap?token=${props.client.token}`, {
      method: "PUT",
      body: JSON.stringify({ token: props.client.token, ...ldap.value }),
    });
    const r = await props.client.request(`/admin/auth/ldap/test?token=${props.client.token}`, {
      method: "POST",
      body: JSON.stringify({
        token: props.client.token,
        username: testUser.value.trim(),
        password: testPass.value,
      }),
    });
    testMsg.value = r.message ?? "连接正常";
    testOk.value = true;
  } catch (err) {
    testMsg.value = err instanceof ApiError ? err.message : String(err);
    testOk.value = false;
  } finally {
    testing.value = false;
  }
}

/** 给管理员去 IdP 注册客户端用的回调地址 */
const oidcRedirect = computed(() => {
  const base = oidc.value.redirect_base.trim().replace(/\/+$/, "");
  return `${base || "（未配置 redirect_base 时由请求地址推断）"}/auth/sso/oidc/callback`;
});

/** LDAP / OIDC 两张配置卡用标签切换，避免单页内容过长 */
const activeTab = ref<"ldap" | "oidc">("ldap");
</script>

<template>
  <div class="cfg-pane">
    <div class="pane-head">
      <div class="pane-title">
        <span class="sub">由服务端动态生效；密码登录始终可用</span>
      </div>
    </div>

    <div v-if="loading" class="cfg-empty">加载中…</div>
    <template v-else>
      <div class="tabs">
        <button class="tab" :class="{ on: activeTab === 'ldap' }" @click="activeTab = 'ldap'">
          LDAP<span v-if="ldap.enabled" class="on-dot" title="已启用" />
        </button>
        <button class="tab" :class="{ on: activeTab === 'oidc' }" @click="activeTab = 'oidc'">
          SSO（OIDC）<span v-if="oidc.enabled" class="on-dot" title="已启用" />
        </button>
      </div>

      <div v-if="error" class="cfg-error">{{ error }}</div>

      <!-- LDAP -->
      <section v-if="activeTab === 'ldap'" class="card">
        <header class="card-head">
          <label class="switch"><input v-model="ldap.enabled" type="checkbox" /> 启用 LDAP 登录</label>
          <button class="primary" :disabled="saving === 'ldap'" @click="saveLdap">
            {{ saving === "ldap" ? "保存中…" : saved === "ldap" ? "已保存 ✓" : "保存" }}
          </button>
        </header>
        <div class="grid">
          <label class="span2">服务器地址<input v-model="ldap.server_url" placeholder="ldaps://ldap.example.com:636" spellcheck="false" /></label>
          <label class="switch inline"><input v-model="ldap.starttls" type="checkbox" /> StartTLS（ldap:// 地址时）</label>
          <label>服务账号 DN<input v-model="ldap.bind_dn" placeholder="cn=admin,dc=example,dc=com（空=匿名）" spellcheck="false" /></label>
          <label>服务账号密码<input v-model="ldap.bind_password" type="password" autocomplete="new-password" /></label>
          <label class="span2">搜索 Base DN<input v-model="ldap.base_dn" placeholder="dc=example,dc=com" spellcheck="false" /></label>
          <label>用户过滤器<input v-model="ldap.user_filter" placeholder="(uid={username})" spellcheck="false" /></label>
          <label>邮箱属性<input v-model="ldap.email_attr" placeholder="mail" spellcheck="false" /></label>
          <label>姓名属性<input v-model="ldap.name_attr" placeholder="cn" spellcheck="false" /></label>
          <label class="switch inline"><input v-model="ldap.allow_register" type="checkbox" /> 首次登录自动建号</label>
        </div>
        <div class="test">
          <input v-model="testUser" placeholder="测试用户名（可选）" spellcheck="false" />
          <input v-model="testPass" type="password" placeholder="测试密码（可选）" autocomplete="new-password" />
          <button class="ghost" :disabled="testing || !ldap.server_url" @click="testLdap">
            {{ testing ? "测试中…" : "测试连接" }}
          </button>
        </div>
        <div v-if="testMsg" class="test-msg" :class="testOk ? 'ok' : 'bad'">{{ testMsg }}</div>
      </section>

      <!-- OIDC -->
      <section v-else class="card">
        <header class="card-head">
          <label class="switch"><input v-model="oidc.enabled" type="checkbox" /> 启用 SSO（OIDC）登录</label>
          <button class="primary" :disabled="saving === 'oidc'" @click="saveOidc">
            {{ saving === "oidc" ? "保存中…" : saved === "oidc" ? "已保存 ✓" : "保存" }}
          </button>
        </header>
        <div class="grid">
          <label class="span2">Issuer<input v-model="oidc.issuer" placeholder="https://sso.example.com/realms/pidock" spellcheck="false" /></label>
          <label>Client ID<input v-model="oidc.client_id" placeholder="pidock" spellcheck="false" /></label>
          <label>Client Secret<input v-model="oidc.client_secret" type="password" autocomplete="new-password" /></label>
          <label>Scopes<input v-model="oidc.scopes" placeholder="openid email profile" spellcheck="false" /></label>
          <label>登录按钮文案<input v-model="oidc.label" placeholder="SSO 登录" /></label>
          <label class="span2">公开基址 redirect_base<input v-model="oidc.redirect_base" placeholder="https://pidock.example.com（反代后建议显式配置）" spellcheck="false" /></label>
          <label class="switch inline"><input v-model="oidc.allow_register" type="checkbox" /> 首次登录自动建号</label>
        </div>
        <div class="hint">
          在 IdP 注册客户端时，回调地址填：<code>{{ oidcRedirect }}</code>
        </div>
      </section>
    </template>
  </div>
</template>

<style scoped>
.cfg-pane {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  gap: 12px;
  overflow-y: auto;
}
.pane-head {
  padding-bottom: 12px;
  border-bottom: 1px solid var(--pd-border);
}
.pane-title {
  display: flex;
  align-items: baseline;
  gap: 10px;
}
.pane-title b {
  color: var(--pd-text);
  font-size: calc(15px * var(--pd-font-scale));
}
.pane-title .sub {
  color: var(--pd-text-4);
  font-size: calc(11px * var(--pd-font-scale));
}
.cfg-empty {
  color: var(--pd-text-4);
  font-size: calc(12px * var(--pd-font-scale));
  padding: 18px 0;
  text-align: center;
}
.cfg-error {
  color: var(--pd-red-text, #e5484d);
  font-size: calc(12px * var(--pd-font-scale));
}
.tabs {
  display: flex;
  gap: 4px;
  border-bottom: 1px solid var(--pd-border);
}
.tab {
  position: relative;
  background: transparent;
  border: none;
  color: var(--pd-text-3);
  font-size: calc(12.5px * var(--pd-font-scale));
  padding: 7px 14px 9px;
  cursor: pointer;
  border-radius: 8px 8px 0 0;
  display: flex;
  align-items: center;
  gap: 6px;
}
.tab:hover {
  color: var(--pd-text);
  background: var(--pd-bg-active, var(--pd-bg));
}
.tab.on {
  color: var(--pd-text);
  font-weight: 600;
  box-shadow: inset 0 -2px 0 var(--pd-accent);
}
.on-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--pd-green);
  flex: none;
}
.card {
  background: var(--pd-bg);
  border: 1px solid var(--pd-border);
  border-radius: 10px;
  padding: 12px 14px;
  display: flex;
  flex-direction: column;
  gap: 10px;
}
.card-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
}
.switch {
  display: flex;
  align-items: center;
  gap: 6px;
  color: var(--pd-text);
  font-size: calc(12.5px * var(--pd-font-scale));
  cursor: pointer;
}
.switch.inline {
  /* .grid label 是 column 布局，开关行必须横向排列并对齐输入框底边 */
  flex-direction: row;
  align-items: center;
  align-self: end;
  padding-bottom: 7px;
  color: var(--pd-text-3);
  font-size: calc(11.5px * var(--pd-font-scale));
}
.grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 8px 12px;
}
.grid label {
  display: flex;
  flex-direction: column;
  gap: 4px;
  font-size: calc(11px * var(--pd-font-scale));
  color: var(--pd-text-3);
}
.grid .span2 {
  grid-column: span 2;
}
.grid input {
  background: var(--pd-bg-panel, var(--pd-bg));
  border: 1px solid var(--pd-border);
  border-radius: 8px;
  color: var(--pd-text);
  padding: 6px 9px;
  font-size: calc(12px * var(--pd-font-scale));
}
.test {
  display: flex;
  gap: 8px;
  align-items: center;
  padding-top: 2px;
}
.test input {
  flex: 1;
  min-width: 0;
  background: var(--pd-bg-panel, var(--pd-bg));
  border: 1px solid var(--pd-border);
  border-radius: 8px;
  color: var(--pd-text);
  padding: 5px 9px;
  font-size: calc(12px * var(--pd-font-scale));
}
.test-msg {
  font-size: calc(11.5px * var(--pd-font-scale));
}
.test-msg.ok {
  color: var(--pd-green);
}
.test-msg.bad {
  color: var(--pd-red-text, #e5484d);
}
.hint {
  color: var(--pd-text-4);
  font-size: calc(11px * var(--pd-font-scale));
}
.hint code {
  font-family: ui-monospace, monospace;
  color: var(--pd-text-2);
  word-break: break-all;
}
/* 窄屏：双列 grid 挤成窄条、测试行放不下，改单列 */
@media (max-width: 640px) {
  .card-head {
    flex-wrap: wrap;
    row-gap: 6px;
  }
  .grid {
    grid-template-columns: 1fr;
  }
  .grid .span2 {
    grid-column: auto;
  }
  .test {
    flex-wrap: wrap;
  }
  .test input {
    flex: 1 1 100%;
  }
}
</style>
