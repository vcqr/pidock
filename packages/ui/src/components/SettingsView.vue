<script setup lang="ts">
import { onMounted, ref } from "vue";
import type { DataBus } from "../databus.js";

const props = withDefaults(
  defineProps<{ bus: DataBus; initialTab?: "models" | "extensions" | "skills" | "mcp" }>(),
  { initialTab: "models" },
);
const emit = defineEmits<{ close: [] }>();

const tab = ref<"models" | "extensions" | "skills" | "mcp">(props.initialTab);

interface ProviderUi {
  id: string;
  auth: string;
  models: number;
  default: boolean;
}
interface ModelUi {
  provider: string;
  id: string;
  name: string;
  reasoning: boolean;
}
interface ExtensionUi {
  name: string;
  file: string;
  scope: string;
  enabled: boolean;
}
interface SkillUi {
  name: string;
  description: string;
  path: string;
  scope: string;
  enabled: boolean;
}

const providers = ref<ProviderUi[]>([]);
const models = ref<ModelUi[]>([]);
const modelFilter = ref("");
const extensions = ref<ExtensionUi[]>([]);
const skills = ref<SkillUi[]>([]);
const mcpText = ref("");
const mcpPath = ref("");
const settings = ref<any>(null);
const agentDir = ref("");
const notice = ref<string | null>(null);
const loading = ref(false);

const filteredModels = ref<ModelUi[]>([]);
function applyModelFilter(): void {
  const q = modelFilter.value.trim().toLowerCase();
  filteredModels.value = !q
    ? models.value.slice(0, 200)
    : models.value.filter((m) => `${m.provider}/${m.id}`.toLowerCase().includes(q)).slice(0, 200);
}

function flash(msg: string): void {
  notice.value = msg;
  setTimeout(() => (notice.value = null), 2500);
}

async function loadModels(): Promise<void> {
  const r = await props.bus.request("config.providers.list");
  providers.value = r.providers ?? [];
  const m = await props.bus.request("config.models.list");
  models.value = m.models ?? [];
  applyModelFilter();
}

async function loadExtensions(): Promise<void> {
  const r = await props.bus.request("config.extensions.list", {});
  extensions.value = r.extensions ?? [];
}

async function loadSkills(): Promise<void> {
  const r = await props.bus.request("config.skills.list", {});
  skills.value = r.skills ?? [];
}

async function loadMcp(): Promise<void> {
  const r = await props.bus.request("config.mcp.get");
  mcpText.value = r.config ? JSON.stringify(r.config, null, 2) : "{\n}";
  mcpPath.value = r.path;
}

async function loadAll(): Promise<void> {
  loading.value = true;
  try {
    const g = await props.bus.request("config.get");
    settings.value = g.settings;
    agentDir.value = g.agent_dir;
    await Promise.all([loadModels(), loadExtensions(), loadSkills(), loadMcp()]);
  } catch (err) {
    flash(String(err));
  } finally {
    loading.value = false;
  }
}
onMounted(loadAll);

async function setKey(p: ProviderUi): Promise<void> {
  const key = window.prompt(`为 ${p.id} 输入 API Key：`);
  if (!key) return;
  await props.bus.request("config.providers.set_key", { provider: p.id, key });
  flash(`${p.id} 密钥已保存`);
  await loadModels();
}

async function removeKey(p: ProviderUi): Promise<void> {
  if (!window.confirm(`删除 ${p.id} 的凭据？`)) return;
  await props.bus.request("config.providers.remove_key", { provider: p.id });
  flash(`${p.id} 凭据已删除`);
  await loadModels();
}

async function setDefaultModel(m: ModelUi): Promise<void> {
  await props.bus.request("config.models.set_default", { provider: m.provider, model: m.id });
  flash(`默认模型已设为 ${m.provider}/${m.id}`);
  await loadModels();
}

async function toggleExtension(e: ExtensionUi): Promise<void> {
  await props.bus.request("config.extensions.toggle", { file: e.file, enabled: !e.enabled });
  e.enabled = !e.enabled;
  flash("改动对新会话生效");
}

async function toggleSkill(s: SkillUi): Promise<void> {
  await props.bus.request("config.skills.toggle", { path: s.path, enabled: !s.enabled });
  s.enabled = !s.enabled;
  flash("改动对新会话生效");
}

async function saveMcp(): Promise<void> {
  try {
    const config = JSON.parse(mcpText.value);
    await props.bus.request("config.mcp.set", { config });
    flash("MCP 配置已保存");
  } catch (err) {
    flash("JSON 解析失败：" + String(err));
  }
}

async function saveTrust(v: string): Promise<void> {
  await props.bus.request("config.settings.set", { patch: { defaultProjectTrust: v } });
  flash("已保存");
}
</script>

<template>
  <div class="settings">
    <header class="head">
      <button class="back" @click="emit('close')">← 返回</button>
      <h2>设置中心</h2>
      <span class="agent-dir" :title="agentDir">{{ agentDir }}</span>
    </header>
    <div v-if="notice" class="notice">{{ notice }}</div>

    <nav class="tabs">
      <button :class="{ on: tab === 'models' }" @click="tab = 'models'">模型与密钥</button>
      <button :class="{ on: tab === 'extensions' }" @click="tab = 'extensions'">
        扩展 ({{ extensions.length }})
      </button>
      <button :class="{ on: tab === 'skills' }" @click="tab = 'skills'">技能 ({{ skills.length }})</button>
      <button :class="{ on: tab === 'mcp' }" @click="tab = 'mcp'">MCP</button>
    </nav>

    <div class="body">
      <!-- models & keys -->
      <section v-if="tab === 'models'" class="pane">
        <h3>Providers（{{ providers.length }}）</h3>
        <div class="grid">
          <div v-for="p in providers" :key="p.id" class="card" :class="{ off: p.auth === 'missing' }">
            <div class="card-head">
              <b>{{ p.id }}</b>
              <span class="badge">{{ p.auth }}</span>
              <span v-if="p.default" class="badge blue">默认</span>
            </div>
            <div class="muted">{{ p.models }} 个模型</div>
            <div class="card-actions">
              <button @click="setKey(p)">设置密钥</button>
              <button v-if="p.auth !== 'missing'" class="danger" @click="removeKey(p)">删除</button>
            </div>
          </div>
        </div>

        <h3>
          模型（{{ models.length }}，显示前 {{ filteredModels.length }} 个）
          <input v-model="modelFilter" class="filter" placeholder="过滤，如 minimax / claude / kimi" @input="applyModelFilter" />
        </h3>
        <table class="tbl">
          <thead>
            <tr><th>provider/model</th><th>推理</th><th></th></tr>
          </thead>
          <tbody>
            <tr v-for="m in filteredModels" :key="m.provider + '/' + m.id">
              <td><code>{{ m.provider }}/{{ m.id }}</code></td>
              <td>{{ m.reasoning ? "✓" : "" }}</td>
              <td><button @click="setDefaultModel(m)">设为默认</button></td>
            </tr>
          </tbody>
        </table>
      </section>

      <!-- extensions -->
      <section v-else-if="tab === 'extensions'" class="pane">
        <p class="muted">来源：~/.pi/agent/extensions（全局）与项目 .pi/extensions。停用通过 settings.json 的排除规则实现，对新会话生效。</p>
        <div v-for="e in extensions" :key="e.file" class="row-item">
          <div>
            <b>{{ e.name }}</b>
            <span class="badge">{{ e.scope }}</span>
            <div class="muted small">{{ e.file }}</div>
          </div>
          <label class="switch">
            <input type="checkbox" :checked="e.enabled" @change="toggleExtension(e)" />
            {{ e.enabled ? "启用" : "停用" }}
          </label>
        </div>
        <div v-if="!extensions.length" class="empty">还没有安装扩展。把 .ts 文件放进 ~/.pi/agent/extensions 即可。</div>
      </section>

      <!-- skills -->
      <section v-else-if="tab === 'skills'" class="pane">
        <p class="muted">来源：~/.pi/agent/skills 与项目 .pi/skills。</p>
        <div v-for="s in skills" :key="s.path" class="row-item">
          <div>
            <b>{{ s.name }}</b>
            <span class="badge">{{ s.scope }}</span>
            <div class="muted small">{{ s.description }}</div>
          </div>
          <label class="switch">
            <input type="checkbox" :checked="s.enabled" @change="toggleSkill(s)" />
            {{ s.enabled ? "启用" : "停用" }}
          </label>
        </div>
        <div v-if="!skills.length" class="empty">还没有安装技能。把含 SKILL.md 的目录放进 ~/.pi/agent/skills 即可。</div>
      </section>

      <!-- mcp -->
      <section v-else class="pane">
        <p class="muted">
          pi 的 MCP 能力由扩展提供（如社区 MCP 扩展），本页编辑其配置文件
          <code>{{ mcpPath }}</code>。格式以所用 MCP 扩展的文档为准。
        </p>
        <textarea v-model="mcpText" class="json" rows="16" spellcheck="false" />
        <div class="pane-actions">
          <button class="primary" @click="saveMcp">保存 mcp.json</button>
        </div>
        <h3>项目信任</h3>
        <p class="muted">defaultProjectTrust：项目本地 .pi 资源（扩展/技能/设置）的默认信任策略。</p>
        <select :value="settings?.defaultProjectTrust ?? 'ask'" @change="saveTrust(($event.target as HTMLSelectElement).value)">
          <option value="ask">ask（每次询问）</option>
          <option value="always">always（总是信任）</option>
          <option value="never">never（忽略项目资源）</option>
        </select>
      </section>
    </div>
    <div v-if="loading" class="loading">加载中…</div>
  </div>
</template>

<style scoped>
.settings {
  position: absolute;
  inset: 0;
  background: #0a0f1a;
  display: flex;
  flex-direction: column;
  z-index: 10;
}
.head {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 12px 16px;
  border-bottom: 1px solid #1f2937;
}
.head h2 { margin: 0; font-size: 16px; color: #f3f4f6; }
.agent-dir { margin-left: auto; font-size: 11px; color: #4b5563; }
.back {
  background: #1f2937;
  color: #e5e7eb;
  border: none;
  border-radius: 6px;
  padding: 6px 12px;
  cursor: pointer;
}
.notice {
  margin: 8px 16px 0;
  padding: 8px 12px;
  background: #064e3b;
  color: #a7f3d0;
  border-radius: 6px;
  font-size: 12.5px;
}
.tabs {
  display: flex;
  gap: 4px;
  padding: 10px 16px 0;
}
.tabs button {
  background: transparent;
  color: #9ca3af;
  border: none;
  border-bottom: 2px solid transparent;
  padding: 8px 14px;
  font-size: 13px;
  cursor: pointer;
}
.tabs button.on { color: #60a5fa; border-bottom-color: #3b82f6; }
.body { flex: 1; overflow-y: auto; padding: 16px; }
.pane h3 { color: #e5e7eb; font-size: 14px; display: flex; align-items: center; gap: 10px; }
.grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(240px, 1fr));
  gap: 8px;
  margin: 10px 0 24px;
}
.card {
  border: 1px solid #1f2937;
  border-radius: 8px;
  padding: 10px;
  background: #0b1220;
}
.card.off { opacity: 0.55; }
.card-head { display: flex; align-items: center; gap: 8px; }
.badge {
  font-size: 10px;
  padding: 2px 6px;
  border-radius: 4px;
  background: #1f2937;
  color: #9ca3af;
}
.badge.blue { background: #1e3a8a; color: #bfdbfe; }
.muted { color: #6b7280; font-size: 12px; }
.small { font-size: 11px; }
.card-actions { margin-top: 8px; display: flex; gap: 6px; }
button {
  background: #1f2937;
  color: #e5e7eb;
  border: none;
  border-radius: 6px;
  padding: 5px 10px;
  font-size: 12px;
  cursor: pointer;
}
button.primary { background: #2563eb; }
button.danger { background: #7f1d1d; color: #fecaca; }
.filter {
  flex: 1;
  max-width: 300px;
  background: #111827;
  border: 1px solid #1f2937;
  color: #e5e7eb;
  border-radius: 6px;
  padding: 5px 8px;
  font-size: 12px;
}
.tbl { width: 100%; border-collapse: collapse; font-size: 12.5px; }
.tbl th, .tbl td { text-align: left; padding: 5px 8px; border-bottom: 1px solid #141c2c; color: #d1d5db; }
.tbl code { color: #93c5fd; }
.row-item {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 10px;
  border: 1px solid #1f2937;
  border-radius: 8px;
  margin-bottom: 6px;
  background: #0b1220;
}
.switch { display: flex; align-items: center; gap: 6px; color: #9ca3af; font-size: 12px; white-space: nowrap; }
.json {
  width: 100%;
  background: #0b1220;
  color: #d1d5db;
  border: 1px solid #1f2937;
  border-radius: 8px;
  font-family: Consolas, monospace;
  font-size: 12px;
  padding: 10px;
}
.pane-actions { margin-top: 10px; }
.empty { color: #4b5563; font-size: 12.5px; padding: 20px 0; }
.loading { position: absolute; bottom: 12px; right: 16px; color: #4b5563; font-size: 12px; }
select {
  background: #111827;
  color: #e5e7eb;
  border: 1px solid #1f2937;
  border-radius: 6px;
  padding: 6px 10px;
  font-size: 12.5px;
}
</style>
