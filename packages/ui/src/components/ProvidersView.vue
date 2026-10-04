<script setup lang="ts">
import { computed, nextTick, onMounted, ref, watch } from "vue";
import { NModal } from "naive-ui";
import type { DataBus } from "../databus.js";
import Icon from "./Icon.vue";

/**
 * 模型供应商管理页：
 * - 左栏：供应商列表（搜索 + 添加供应商）
 * - 右栏：内置供应商 = API 密钥管理 + 模型列表（设默认）；
 *         自定义供应商 = 图 2 表单（名称/API 类型/Base URL/API Key/获取模型列表/候选模型 ID），
 *         保存写入 ~/.pi/agent/models.json。
 */
const props = defineProps<{ bus: DataBus }>();


const API_TYPES = [
  { value: "openai-completions", label: "OpenAI Chat Completions (openai-completions)" },
  { value: "openai-responses", label: "OpenAI Responses API (openai-responses)" },
  { value: "anthropic-messages", label: "Anthropic Messages (anthropic-messages)" },
  { value: "google-generative-ai", label: "Google Generative AI (google-generative-ai)" },
];

interface ProviderRow {
  id: string;
  source: "builtin" | "custom";
  auth: string;
  modelsCount: number;
  baseUrl?: string;
  api?: string;
  customModels: string[];
  /** 自定义模型的原始行（含能力标记），供表单编辑 */
  customModelRows: Array<ModelRow>;
  hasEntryKey: boolean;
}

const loading = ref(true);
const notice = ref<string | null>(null);
const query = ref("");
const providers = ref<ProviderRow[]>([]);
const modelsJsonPath = ref("");
const allModels = ref<Array<{ provider: string; id: string; name?: string; reasoning: boolean; input?: string[]; contextWindow?: number; maxTokens?: number }>>([]);
const defaultProvider = ref("");
const defaultModel = ref("");
const selected = ref<string | null>(null);
const adding = ref(false);

// 密钥行内编辑
const keyDraft = ref("");
const keyEditing = ref(false);
const keyVisible = ref(false);

// 自定义供应商表单
const formId = ref("");
const formApi = ref("openai-completions");
const formBaseUrl = ref("");
const formApiKey = ref("");
const formModelsText = ref("");
const formKeyVisible = ref(false);
const formBusy = ref(false);
/** 逐模型配置行：能力（视觉/推理）+ 上下文窗口 / 最大输出 / 推理参数映射 */
interface ModelRow {
  id: string;
  image: boolean;
  reasoning: boolean;
  /** 扩展输入能力（pi schema 暂只认 text/image，video/pdf 存 extInput 透传） */
  video: boolean;
  pdf: boolean;
  contextWindow?: number;
  maxTokens?: number;
  thinkingLevelMap?: unknown;
}

const formModelRows = ref<Array<ModelRow>>([]);

// ---- 模型编辑弹窗 ----
const editingModel = ref<{
  /** row = 自定义供应商表单行；override = 内置模型覆盖（保存即写入 host） */
  mode: "row" | "override";
  providerId?: string;
  origId?: string;
  index?: number;
  id: string;
  contextWindow: string;
  maxTokens: string;
  image: boolean;
  reasoning: boolean;
  video: boolean;
  pdf: boolean;
  tlmText: string;
} | null>(null);
const advOpen = ref(false);
/** 弹窗保存同步候选文本时，抑制行重建（否则改 id 会丢能力标记） */
let suppressRowsSync = false;

function openModelEditor(i: number): void {
  const r = formModelRows.value[i];
  if (!r) return;
  editingModel.value = {
    mode: "row",
    index: i,
    id: r.id,
    contextWindow: r.contextWindow != null ? String(r.contextWindow) : "",
    maxTokens: r.maxTokens != null ? String(r.maxTokens) : "",
    image: r.image,
    reasoning: r.reasoning,
    video: r.video,
    pdf: r.pdf,
    tlmText: r.thinkingLevelMap ? JSON.stringify(r.thinkingLevelMap, null, 2) : "",
  };
  advOpen.value = false;
}

/** 内置模型：点击行打开覆盖配置弹窗（预填运行时当前值） */
function openModelOverrideEditor(m: {
  provider: string;
  id: string;
  input?: string[];
  reasoning?: boolean;
  contextWindow?: number;
  maxTokens?: number;
}): void {
  editingModel.value = {
    mode: "override",
    providerId: m.provider,
    origId: m.id,
    id: m.id,
    contextWindow: m.contextWindow != null ? String(m.contextWindow) : "",
    maxTokens: m.maxTokens != null ? String(m.maxTokens) : "",
    image: m.input?.includes("image") ?? false,
    reasoning: m.reasoning ?? false,
    video: false,
    pdf: false,
    tlmText: "",
  };
  advOpen.value = false;
}

async function saveModelEditor(): Promise<void> {
  const e = editingModel.value;
  if (!e) return;
  let tlm: unknown;
  if (e.tlmText.trim()) {
    try {
      tlm = JSON.parse(e.tlmText);
    } catch {
      flash("推理参数映射不是合法 JSON");
      return;
    }
  }
  if (e.mode === "override") {
    formBusy.value = true;
    try {
      const extInput = [e.video && "video", e.pdf && "pdf"].filter(Boolean);
      await props.bus.request("config.model_override.set", {
        provider: e.providerId,
        model: e.origId,
        override: {
          input: e.image ? ["text", "image"] : ["text"],
          ...(e.reasoning ? { reasoning: true } : {}),
          ...(Number(e.contextWindow) ? { contextWindow: Number(e.contextWindow) } : {}),
          ...(Number(e.maxTokens) ? { maxTokens: Number(e.maxTokens) } : {}),
          ...(tlm ? { thinkingLevelMap: tlm } : {}),
          ...(extInput.length ? { extInput } : {}),
        },
      });
      await load();
      flash(`已保存「${e.origId}」的模型配置`);
      editingModel.value = null;
    } catch (err) {
      flash(String(err));
    } finally {
      formBusy.value = false;
    }
    return;
  }
  const r = formModelRows.value[e.index!];
  if (r) {
    r.id = e.id.trim() || r.id;
    r.image = e.image;
    r.reasoning = e.reasoning;
    r.video = e.video;
    r.pdf = e.pdf;
    r.contextWindow = Number(e.contextWindow) || undefined;
    r.maxTokens = Number(e.maxTokens) || undefined;
    r.thinkingLevelMap = tlm;
  }
  suppressRowsSync = true;
  formModelsText.value = formModelRows.value.map((x) => x.id).join(", ");
  void nextTick(() => (suppressRowsSync = false));
  editingModel.value = null;
}

watch(formModelsText, (v) => {
  if (suppressRowsSync) return;
  const ids = v
    .split(/[,\n]/)
    .map((s) => s.trim())
    .filter(Boolean);
  const prev = new Map(formModelRows.value.map((r) => [r.id, r]));
  formModelRows.value = ids.map((id) => prev.get(id) ?? { id, image: false, reasoning: false, video: false, pdf: false });
});

function loadModelRows(): void {
  const p = selectedRow.value;
  formModelRows.value = (p?.customModelRows ?? []).map((r) => ({ ...r }));
}

/** 从能力行中移除模型（同时同步候选 ID 文本） */
function removeModelRow(i: number): void {
  const row = formModelRows.value[i];
  if (!row) return;
  formModelRows.value.splice(i, 1);
  const ids = formModelRows.value.map((r) => r.id);
  formModelsText.value = ids.join(", ");
}

function fmtInt(n: number): string {
  return n.toLocaleString("en-US");
}

let flashTimer: ReturnType<typeof setTimeout> | undefined;
function flash(msg: string): void {
  notice.value = msg;
  if (flashTimer) clearTimeout(flashTimer);
  flashTimer = setTimeout(() => (notice.value = null), 2500);
}

async function load(): Promise<void> {
  loading.value = true;
  try {
    const [pl, mj, ml, g] = await Promise.all([
      props.bus.request("config.providers.list"),
      props.bus.request("config.providers.custom.get"),
      props.bus.request("config.models.list"),
      props.bus.request("config.get"),
    ]);
    modelsJsonPath.value = mj.path ?? "";
    const entries: Record<string, any> = mj.config?.providers ?? {};
    const runtime: Record<string, any> = {};
    for (const p of pl.providers ?? []) runtime[p.id] = p;
    const ids = new Set<string>([...Object.keys(entries), ...Object.keys(runtime)]);
    providers.value = [...ids].map((id) => {
      const entry = entries[id] ?? null;
      const rt = runtime[id];
      // 仅含 modelOverrides 的条目 = 内置模型的覆盖配置，不算自定义供应商
      const overrideOnly =
        entry &&
        !entry.baseUrl &&
        !entry.api &&
        !entry.apiKey &&
        !entry.oauth &&
        !entry.headers &&
        !entry.compat &&
        !entry.authHeader &&
        !(Array.isArray(entry.models) && entry.models.length);
      const source = entry && !overrideOnly ? "custom" : "builtin";
      return {
        id,
        source,
        auth: rt?.auth ?? (entry?.apiKey ? "stored" : "missing"),
        modelsCount: rt?.models ?? (Array.isArray(entry?.models) ? entry.models.length : 0),
        baseUrl: entry?.baseUrl,
        api: entry?.api,
        customModels: Array.isArray(entry?.models) ? entry.models.map((m: any) => String(m.id ?? m)) : [],
        customModelRows: Array.isArray(entry?.models)
          ? entry.models.map((m: any) => ({
              id: String(m.id ?? m),
              image: Array.isArray(m.input) && m.input.includes("image"),
              reasoning: Boolean(m.reasoning),
              video: Array.isArray(m.extInput) && m.extInput.includes("video"),
              pdf: Array.isArray(m.extInput) && m.extInput.includes("pdf"),
              contextWindow: typeof m.contextWindow === "number" ? m.contextWindow : undefined,
              maxTokens: typeof m.maxTokens === "number" ? m.maxTokens : undefined,
            }))
          : [],
        hasEntryKey: !!entry?.apiKey,
      };
    });
    allModels.value = ml.models ?? [];
    defaultProvider.value = g.settings?.defaultProvider ?? "";
    defaultModel.value = g.settings?.defaultModel ?? "";
    if (selected.value && !providers.value.some((p) => p.id === selected.value)) selected.value = null;
  } catch (err) {
    flash(String(err));
  } finally {
    loading.value = false;
  }
}
onMounted(load);

const filtered = computed(() => {
  const s = query.value.trim().toLowerCase();
  if (!s) return providers.value;
  return providers.value.filter((p) => p.id.toLowerCase().includes(s));
});

const selectedRow = computed(() => providers.value.find((p) => p.id === selected.value) ?? null);
const isCustom = computed(() => selectedRow.value?.source === "custom");

const providerModels = computed(() => {
  if (!selected.value) return [];
  return allModels.value
    .filter((m) => m.provider === selected.value)
    .slice(0, 200)
    .map((m) => ({ ...m, image: m.input?.includes("image") ?? false }));
});
const isDefaultModel = (m: { provider: string; id: string }): boolean =>
  defaultProvider.value === m.provider && defaultModel.value === m.id;

function select(id: string | null): void {
  selected.value = id;
  adding.value = false;
  keyDraft.value = "";
  keyEditing.value = false;
  keyVisible.value = false;
  formApiKey.value = "";
  formKeyVisible.value = false;
  loadForm();
}

function startAdd(): void {
  adding.value = true;
  selected.value = null;
  formId.value = "";
  formApi.value = "openai-completions";
  formBaseUrl.value = "";
  formApiKey.value = "";
  formModelsText.value = "";
  formModelRows.value = [];
  formKeyVisible.value = false;
}

function loadForm(): void {
  const p = selectedRow.value;
  formId.value = p?.id ?? "";
  formApi.value = p?.api ?? "openai-completions";
  formBaseUrl.value = p?.baseUrl ?? "";
  formModelsText.value = p?.customModels.join(", ") ?? "";
  loadModelRows();
  formApiKey.value = "";
}

async function saveCustom(): Promise<void> {
  const id = formId.value.trim();
  if (!id || /\s/.test(id)) return flash("供应商名称必填且不能包含空格");
  if (!formBaseUrl.value.trim()) return flash("Base URL 必填");
  const rows: ModelRow[] = formModelRows.value.length
    ? formModelRows.value
    : formModelsText.value
        .split(/[,\n]/)
        .map((s) => s.trim())
        .filter(Boolean)
        .map((mid) => ({ id: mid, image: false, reasoning: false, video: false, pdf: false }));
  if (!rows.length) return flash("至少填写一个候选模型 ID");
  formBusy.value = true;
  try {
    await props.bus.request("config.providers.custom.set", {
      id,
      entry: {
        baseUrl: formBaseUrl.value.trim(),
        api: formApi.value,
        models: rows.map((r) => {
          const extInput = [r.video && "video", r.pdf && "pdf"].filter(Boolean);
          return {
            id: r.id,
            input: r.image ? ["text", "image"] : ["text"],
            ...(r.reasoning ? { reasoning: true } : {}),
            ...(extInput.length ? { extInput } : {}),
            ...(r.contextWindow ? { contextWindow: Number(r.contextWindow) } : {}),
            ...(r.maxTokens ? { maxTokens: Number(r.maxTokens) } : {}),
            ...(r.thinkingLevelMap ? { thinkingLevelMap: r.thinkingLevelMap } : {}),
          };
        }),
      },
    });
    if (formApiKey.value.trim()) {
      await props.bus.request("config.providers.set_key", { provider: id, key: formApiKey.value.trim() });
    }
    flash(`已保存「${id}」`);
    adding.value = false;
    selected.value = null;
    await load();
    selected.value = id;
    loadForm();
  } catch (err) {
    flash(String(err));
  } finally {
    formBusy.value = false;
  }
}

async function removeCustom(): Promise<void> {
  const id = formId.value.trim();
  if (!id) return;
  if (!window.confirm(`删除自定义供应商「${id}」？`)) return;
  try {
    await props.bus.request("config.providers.custom.remove", { id });
    flash(`已删除「${id}」`);
    selected.value = null;
    await load();
  } catch (err) {
    flash(String(err));
  }
}

async function fetchModels(): Promise<void> {
  formBusy.value = true;
  try {
    const r = await props.bus.request("config.providers.fetch_models", {
      baseUrl: formBaseUrl.value,
      apiKey: formApiKey.value.trim() || undefined,
      api: formApi.value,
    });
    if (r.models?.length) {
      formModelsText.value = r.models.join(", ");
      flash(`获取到 ${r.models.length} 个模型`);
    } else {
      flash("接口未返回模型列表");
    }
  } catch (err) {
    flash(String(err));
  } finally {
    formBusy.value = false;
  }
}

async function saveKey(): Promise<void> {
  const id = selected.value;
  if (!id || !keyDraft.value.trim()) return;
  try {
    await props.bus.request("config.providers.set_key", { provider: id, key: keyDraft.value.trim() });
    flash(`「${id}」密钥已保存`);
    keyDraft.value = "";
    keyEditing.value = false;
    await load();
  } catch (err) {
    flash(String(err));
  }
}

async function removeKey(): Promise<void> {
  const id = selected.value;
  if (!id) return;
  if (!window.confirm(`删除「${id}」的 API 密钥？`)) return;
  try {
    await props.bus.request("config.providers.remove_key", { provider: id });
    flash(`「${id}」密钥已删除`);
    await load();
  } catch (err) {
    flash(String(err));
  }
}

async function setDefault(m: { provider: string; id: string }): Promise<void> {
  try {
    await props.bus.request("config.models.set_default", { provider: m.provider, model: m.id });
    defaultProvider.value = m.provider;
    defaultModel.value = m.id;
    flash(`默认模型已设为 ${m.provider}/${m.id}`);
  } catch (err) {
    flash(String(err));
  }
}
</script>

<template>
  <div class="prov-page">
    <!-- 左：供应商列表 -->
    <aside class="list-pane">
      <div class="search-box">
        <Icon name="search-line" :size="14" />
        <input v-model="query" placeholder="搜索模型平台…" />
      </div>
      <div class="provider-list">
        <div v-if="loading" class="state small">加载中…</div>
        <button
          v-for="p in filtered"
          :key="p.id"
          class="provider-row"
          :class="{ on: selected === p.id }"
          @click="select(p.id)"
        >
          <span class="tile">{{ p.id.slice(0, 1).toUpperCase() }}</span>
          <span class="p-name">{{ p.id }}</span>
          <span class="dot" :class="{ on: p.auth !== 'missing' }"></span>
        </button>
        <div v-if="!filtered.length && !loading" class="state small">没有匹配的供应商</div>
      </div>
      <button class="add-btn" @click="startAdd">
        <Icon name="add-line" :size="14" />添加供应商
      </button>
    </aside>

    <!-- 右：详情 / 表单 -->
    <section class="detail-pane">
      <!-- 新增自定义供应商 -->
      <template v-if="adding">
        <header class="det-head">
          <h1>添加供应商</h1>
        </header>
        <div class="form">
          <div class="field">
            <label>模型供应商名称 <i>*</i></label>
            <input v-model="formId" placeholder="如 my-proxy" />
          </div>
          <div class="field">
            <label>API 类型</label>
            <select v-model="formApi">
              <option v-for="t in API_TYPES" :key="t.value" :value="t.value">{{ t.label }}</option>
            </select>
          </div>
          <div class="field">
            <label>Base URL <i>*</i></label>
            <input v-model="formBaseUrl" placeholder="https://api.example.com/v1" />
          </div>
          <div class="field">
            <label>API Key</label>
            <div class="key-row">
              <input
                v-model="formApiKey"
                :type="formKeyVisible ? 'text' : 'password'"
                placeholder="留空可稍后在 auth.json 中配置"
              />
              <button class="eye" title="显示 / 隐藏" @click="formKeyVisible = !formKeyVisible">
                <Icon :name="formKeyVisible ? 'eye-off-line' : 'eye-line'" :size="14" />
              </button>
            </div>
          </div>
          <button class="fetch-btn" :disabled="formBusy" @click="fetchModels">
            <Icon name="download-cloud-2-line" :size="14" />获取模型列表
          </button>
          <div class="field">
            <label>候选模型 ID（逗号分隔，可点击上方「获取模型列表」自动填充）</label>
            <textarea v-model="formModelsText" rows="3" placeholder="model-a, model-b"></textarea>
          </div>
          <div class="field">
            <label>模型能力 · {{ formModelRows.length }} 个模型（视觉 = 支持图片输入，推理 = 支持思维链）</label>
            <div class="model-rows">
              <div v-for="(r, i) in formModelRows" :key="r.id" class="model-row" title="点击编辑模型配置" @click="openModelEditor(i)">
                <span class="mr-id">{{ r.id }}</span>
                <span v-if="r.image" class="mr-badge">视觉</span>
                <span v-if="r.reasoning" class="mr-badge">推理</span>
                <span v-if="r.video" class="mr-badge">视频</span>
                <span v-if="r.pdf" class="mr-badge">PDF</span>
                <span v-if="r.contextWindow" class="mr-badge">{{ fmtInt(r.contextWindow) }}</span>
                <button class="mr-del" title="移除该模型" @click.stop="removeModelRow(i)">
                  <Icon name="close-line" :size="12" />
                </button>
              </div>
              <div v-if="!formModelRows.length" class="mr-empty">填写或获取候选模型 ID 后自动生成</div>
            </div>
          </div>
          <div class="form-actions">
            <button class="primary" :disabled="formBusy" @click="saveCustom">保存</button>
          </div>
        </div>
      </template>

      <!-- 自定义供应商编辑 -->
      <template v-else-if="selectedRow && isCustom">
        <header class="det-head">
          <h1>{{ selectedRow.id }}</h1>
          <span class="badge">自定义</span>
          <span class="flex-sp"></span>
          <button class="danger-btn" @click="removeCustom">
            <Icon name="delete-bin-line" :size="14" />删除
          </button>
        </header>
        <p class="models-json-path" :title="modelsJsonPath">models.json · {{ selectedRow.baseUrl }}</p>
        <div class="form">
          <div class="field">
            <label>模型供应商名称</label>
            <input :value="selectedRow.id" disabled />
          </div>
          <div class="field">
            <label>API 类型</label>
            <select v-model="formApi">
              <option v-for="t in API_TYPES" :key="t.value" :value="t.value">{{ t.label }}</option>
            </select>
          </div>
          <div class="field">
            <label>Base URL</label>
            <input v-model="formBaseUrl" />
          </div>
          <div class="field">
            <label>API Key {{ selectedRow.auth !== 'missing' ? '（已配置）' : '（未配置）' }}</label>
            <div class="key-row">
              <input
                v-model="formApiKey"
                :type="formKeyVisible ? 'text' : 'password'"
                :placeholder="selectedRow.hasEntryKey ? '••••••••' : '留空则保持不变'"
              />
              <button class="eye" title="显示 / 隐藏" @click="formKeyVisible = !formKeyVisible">
                <Icon :name="formKeyVisible ? 'eye-off-line' : 'eye-line'" :size="14" />
              </button>
            </div>
          </div>
          <button class="fetch-btn" :disabled="formBusy" @click="fetchModels">
            <Icon name="download-cloud-2-line" :size="14" />获取模型列表
          </button>
          <div class="field">
            <label>候选模型 ID（逗号分隔，可点击上方「获取模型列表」自动填充）</label>
            <textarea v-model="formModelsText" rows="3"></textarea>
          </div>
          <div class="field">
            <label>模型能力 · {{ formModelRows.length }} 个模型（视觉 = 支持图片输入，推理 = 支持思维链）</label>
            <div class="model-rows">
              <div v-for="(r, i) in formModelRows" :key="r.id" class="model-row" title="点击编辑模型配置" @click="openModelEditor(i)">
                <span class="mr-id">{{ r.id }}</span>
                <span v-if="r.image" class="mr-badge">视觉</span>
                <span v-if="r.reasoning" class="mr-badge">推理</span>
                <span v-if="r.video" class="mr-badge">视频</span>
                <span v-if="r.pdf" class="mr-badge">PDF</span>
                <span v-if="r.contextWindow" class="mr-badge">{{ fmtInt(r.contextWindow) }}</span>
                <button class="mr-del" title="移除该模型" @click.stop="removeModelRow(i)">
                  <Icon name="close-line" :size="12" />
                </button>
              </div>
              <div v-if="!formModelRows.length" class="mr-empty">填写或获取候选模型 ID 后自动生成</div>
            </div>
          </div>
          <div class="form-actions">
            <button class="primary" :disabled="formBusy" @click="saveCustom">保存</button>
          </div>
        </div>
      </template>

      <!-- 内置供应商详情 -->
      <template v-else-if="selectedRow">
        <header class="det-head">
          <h1>{{ selectedRow.id }}</h1>
          <span class="badge">内置</span>
          <span class="flex-sp"></span>
          <button class="ghost-btn" title="刷新" @click="load">
            <Icon name="refresh-line" :size="15" />
          </button>
        </header>
        <p class="key-status">
          API 密钥：{{ selectedRow.auth === 'missing' ? '未配置' : `已配置（${selectedRow.auth}）` }}
          <span class="key-models">· {{ selectedRow.modelsCount }} 个模型可用</span>
        </p>
        <div class="key-edit">
          <div class="key-row">
            <input
              v-model="keyDraft"
              :type="keyVisible ? 'text' : 'password'"
              :placeholder="keyEditing ? '输入新的 API Key' : selectedRow.auth !== 'missing' ? '已配置 —— 点击「更换密钥」修改' : '未配置'"
              :disabled="!keyEditing"
            />
            <button class="eye" title="显示 / 隐藏" @click="keyVisible = !keyVisible">
              <Icon :name="keyVisible ? 'eye-off-line' : 'eye-line'" :size="14" />
            </button>
          </div>
          <div class="key-actions">
            <button v-if="!keyEditing" class="ghost-btn wide" @click="keyEditing = true">更换密钥</button>
            <template v-else>
              <button class="primary" @click="saveKey">保存密钥</button>
              <button class="ghost-btn wide" @click="keyEditing = false; keyDraft = ''">取消</button>
            </template>
            <button v-if="selectedRow.auth !== 'missing' && !keyEditing" class="ghost-btn wide danger" @click="removeKey">
              <Icon name="delete-bin-line" :size="13" />删除密钥
            </button>
          </div>
        </div>

        <div class="models-head">
          <h2>模型 <span class="count">{{ providerModels.length }}</span></h2>
        </div>
        <div class="model-list">
          <div
            v-for="m in providerModels"
            :key="m.id"
            class="model-row"
            title="点击编辑模型配置"
            @click="openModelOverrideEditor(m)"
          >
            <span class="m-id">{{ m.id }}</span>
            <span v-if="m.image" class="type-badge">视觉</span>
            <span v-if="m.reasoning" class="type-badge">推理</span>
            <span v-if="isDefaultModel(m)" class="badge">默认</span>
            <template v-else>
              <button class="edit-btn" title="编辑模型配置" @click.stop="openModelOverrideEditor(m)">
                <Icon name="edit-2-line" :size="13" />编辑
              </button>
              <button class="ghost-btn wide" @click.stop="setDefault(m)">设为默认</button>
            </template>
          </div>
          <div v-if="!providerModels.length" class="state small">该供应商暂无可用模型（可能缺少密钥）</div>
        </div>
      </template>

      <div v-else class="state">← 从左侧选择一个供应商，或点击「添加供应商」</div>

      <div v-if="notice" class="notice">{{ notice }}</div>

      <!-- 模型配置编辑弹窗 -->
      <n-modal :show="!!editingModel" @update:show="(v: boolean) => { if (!v) editingModel = null; }">
        <div v-if="editingModel" class="me-card">
          <header class="me-head">
            <b>编辑模型配置</b>
            <button class="me-x" @click="editingModel = null">
              <Icon name="close-line" :size="14" />
            </button>
          </header>
          <div class="me-body">
            <div class="field">
              <label>模型 ID</label>
              <input v-model="editingModel.id" />
            </div>
            <div class="field">
              <label>上下文窗口</label>
              <input v-model="editingModel.contextWindow" inputmode="numeric" placeholder="如 128000" />
            </div>
            <div class="field">
              <label>最大输出 Token</label>
              <input v-model="editingModel.maxTokens" inputmode="numeric" placeholder="如 16384" />
            </div>
            <button class="adv-toggle" @click="advOpen = !advOpen">
              <Icon name="arrow-down-s-line" :size="13" :class="{ fold: !advOpen }" />
              高级配置
            </button>
            <div v-if="advOpen" class="adv-body">
              <div class="field">
                <label>输入类型（文本默认开启）</label>
                <div class="cap-chips">
                  <span class="cap-chip locked"><Icon name="check-line" :size="12" />文本</span>
                  <label class="cap-chip" :class="{ on: editingModel.image }">
                    <input v-model="editingModel.image" type="checkbox" />图片
                  </label>
                  <label class="cap-chip" :class="{ on: editingModel.video }">
                    <input v-model="editingModel.video" type="checkbox" />视频
                  </label>
                  <label class="cap-chip" :class="{ on: editingModel.pdf }">
                    <input v-model="editingModel.pdf" type="checkbox" />PDF
                  </label>
                </div>
              </div>
              <div class="field">
                <label>推理（支持思维链）</label>
                <label class="cap-chip" :class="{ on: editingModel.reasoning }">
                  <input v-model="editingModel.reasoning" type="checkbox" />推理
                </label>
              </div>
              <div class="field">
                <label>推理参数映射（可选 JSON）</label>
                <textarea v-model="editingModel.tlmText" rows="4" class="mono" placeholder='{"thinking": {"low": "low"}}'></textarea>
              </div>
            </div>
          </div>
          <footer class="me-foot">
            <button class="me-cancel" @click="editingModel = null">取消</button>
            <button class="primary" @click="saveModelEditor">保存</button>
          </footer>
        </div>
      </n-modal>
    </section>
  </div>
</template>

<style scoped>
.prov-page {
  flex: 1;
  display: flex;
  gap: 18px;
  padding: 22px 26px;
  overflow: hidden;
  background: var(--pd-bg);
  min-height: 0;
}

/* 左栏 */
.list-pane {
  width: 240px;
  flex: none;
  display: flex;
  flex-direction: column;
  background: var(--pd-bg-card);
  border: 1px solid var(--pd-border-soft);
  border-radius: 12px;
  padding: 10px;
  overflow: hidden;
}
.search-box {
  display: flex;
  align-items: center;
  gap: 8px;
  background: var(--pd-bg);
  border: 1px solid var(--pd-border);
  border-radius: 9px;
  padding: 7px 10px;
  color: var(--pd-text-4);
  flex: none;
}
.search-box input {
  flex: 1;
  min-width: 0;
  background: none;
  border: none;
  outline: none;
  color: var(--pd-text);
  font-size: 12.5px;
}
.search-box input::placeholder { color: var(--pd-text-4); }
.provider-list {
  flex: 1;
  overflow-y: auto;
  margin-top: 8px;
  min-height: 0;
}
.provider-row {
  display: flex;
  align-items: center;
  gap: 9px;
  width: 100%;
  background: none;
  border: none;
  border-radius: 8px;
  padding: 7px 8px;
  color: var(--pd-text-2);
  font-size: 13px;
  cursor: pointer;
  text-align: left;
}
.provider-row:hover { background: var(--pd-bg-hover); color: var(--pd-text); }
.provider-row.on { background: var(--pd-bg-active); color: var(--pd-text); }
.tile {
  width: 26px;
  height: 26px;
  flex: none;
  border-radius: 7px;
  display: grid;
  place-items: center;
  background: var(--pd-accent-soft);
  color: var(--pd-accent);
  font-size: 12.5px;
  font-weight: 700;
}
.p-name {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  flex: none;
  background: var(--pd-text-4);
  opacity: 0.5;
}
.dot.on { background: var(--pd-green); opacity: 1; }
.add-btn {
  flex: none;
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  margin-top: 8px;
  background: var(--pd-bg);
  border: 1px solid var(--pd-border);
  border-radius: 9px;
  padding: 8px;
  color: var(--pd-text-2);
  font-size: 12.5px;
  cursor: pointer;
}
.add-btn:hover { color: var(--pd-text); border-color: var(--pd-accent); }

/* 右栏 */
.detail-pane {
  flex: 1;
  min-width: 0;
  overflow-y: auto;
  background: var(--pd-bg-card);
  border: 1px solid var(--pd-border-soft);
  border-radius: 12px;
  padding: 22px 26px;
}
.det-head {
  display: flex;
  align-items: center;
  gap: 10px;
}
.det-head h1 {
  margin: 0;
  font-size: 20px;
  font-weight: 700;
  color: var(--pd-text);
}
.badge {
  font-size: 10.5px;
  color: var(--pd-accent-text);
  background: var(--pd-accent-soft);
  border-radius: 5px;
  padding: 1.5px 7px;
  flex: none;
}
.flex-sp { flex: 1; }
.ghost-btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  background: none;
  border: 1px solid var(--pd-border);
  border-radius: 8px;
  padding: 6px 10px;
  color: var(--pd-text-3);
  cursor: pointer;
  font-size: 12.5px;
}
.ghost-btn:hover { color: var(--pd-text); background: var(--pd-bg-hover); }
.ghost-btn.danger:hover { background: var(--pd-red-soft); color: var(--pd-red-text); border-color: var(--pd-red-soft); }
.danger-btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  background: none;
  border: 1px solid var(--pd-border);
  border-radius: 8px;
  padding: 6px 10px;
  color: var(--pd-text-3);
  cursor: pointer;
  font-size: 12.5px;
}
.danger-btn:hover { background: var(--pd-red-soft); color: var(--pd-red-text); border-color: var(--pd-red-soft); }
.key-status { margin: 10px 0 0; font-size: 13px; color: var(--pd-text-2); }
.key-models { color: var(--pd-text-4); font-size: 12px; }

/* 表单（图 2） */
.form { margin-top: 18px; display: flex; flex-direction: column; gap: 14px; }
.field label {
  display: block;
  font-size: 12.5px;
  color: var(--pd-text-2);
  margin-bottom: 6px;
}
.field label i { color: var(--pd-red); font-style: normal; }
.field input,
.field select,
.field textarea {
  width: 100%;
  box-sizing: border-box;
  background: var(--pd-bg);
  border: 1px solid var(--pd-border);
  border-radius: 9px;
  color: var(--pd-text);
  font-size: 13px;
  font-family: inherit;
  padding: 9px 12px;
}
.field select option { background: var(--pd-bg-raised); }
.field textarea { font-family: Consolas, monospace; font-size: 12.5px; resize: vertical; }
.field input:focus,
.field select:focus,
.field textarea:focus { outline: none; border-color: var(--pd-accent); }
.field input::placeholder,
.field textarea::placeholder { color: var(--pd-text-4); }
.field input:disabled { color: var(--pd-text-3); }
.key-row { display: flex; gap: 6px; align-items: center; }
.key-row input { flex: 1; }
.eye {
  flex: none;
  width: 32px;
  height: 32px;
  display: grid;
  place-items: center;
  background: none;
  border: none;
  border-radius: 7px;
  color: var(--pd-text-4);
  cursor: pointer;
}
.eye:hover { background: var(--pd-bg-hover); color: var(--pd-text); }
/* 逐模型能力行 */
.model-rows {
  display: flex;
  flex-direction: column;
  gap: 4px;
  max-height: 220px;
  overflow-y: auto;
}
.model-row {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 5px 8px;
  border: 1px solid var(--pd-border);
  border-radius: 7px;
  font-size: 12.5px;
}
/* 覆盖 .field label 的块级样式，能力标签保持行内紧凑 */
.model-row label.mr-cap,
.model-row .mr-cap {
  display: inline-flex;
  margin-bottom: 0;
}
.mr-id {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--pd-text);
  font-family: Consolas, "JetBrains Mono", monospace;
}
.mr-cap {
  display: inline-flex;
  align-items: center;
  gap: 3px;
  color: var(--pd-text-3);
  cursor: pointer;
  flex: none;
  user-select: none;
}
.model-row input[type="checkbox"] {
  width: auto;
  padding: 0;
  margin: 0;
  accent-color: var(--pd-accent);
}
.mr-del {
  background: none;
  border: none;
  color: var(--pd-text-4);
  cursor: pointer;
  padding: 2px;
  border-radius: 4px;
  display: grid;
  place-items: center;
  flex: none;
}
.mr-del:hover { color: var(--pd-text); background: var(--pd-bg-hover); }
.mr-empty { padding: 8px; font-size: 12px; color: var(--pd-text-4); }

.fetch-btn {
  align-self: flex-start;
  display: inline-flex;
  align-items: center;
  gap: 7px;
  background: none;
  border: 1px solid var(--pd-border);
  border-radius: 9px;
  padding: 8px 14px;
  color: var(--pd-text-2);
  font-size: 13px;
  cursor: pointer;
}
.fetch-btn:hover { color: var(--pd-text); border-color: var(--pd-accent); }
.fetch-btn:disabled { opacity: 0.5; cursor: default; }
.form-actions { display: flex; justify-content: flex-end; gap: 8px; }
.primary {
  background: var(--pd-accent);
  color: #1a1a1a;
  border: none;
  border-radius: 9px;
  padding: 8px 18px;
  font-size: 13px;
  font-weight: 600;
  cursor: pointer;
}
.primary:hover { background: var(--pd-accent-hover); }
.primary:disabled { opacity: 0.5; cursor: default; }

/* 模型列表 */
.models-head { margin-top: 22px; }
.models-head h2 {
  margin: 0;
  font-size: 15px;
  font-weight: 700;
  color: var(--pd-text);
  display: flex;
  align-items: center;
  gap: 8px;
}
.count {
  font-size: 11px;
  font-weight: 600;
  color: var(--pd-text-3);
  background: var(--pd-bg);
  border: 1px solid var(--pd-border);
  border-radius: 999px;
  padding: 0 8px;
}
.model-list { margin-top: 10px; }
.model-row {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 9px 12px;
  border-bottom: 1px solid var(--pd-border-soft);
}
.m-id {
  flex: 1;
  min-width: 0;
  font-family: Consolas, monospace;
  font-size: 12.5px;
  color: var(--pd-text-2);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.type-badge {
  font-size: 10px;
  color: var(--pd-text-3);
  background: var(--pd-bg-hover);
  border-radius: 5px;
  padding: 1.5px 7px;
  flex: none;
}

.state {
  color: var(--pd-text-4);
  font-size: 13px;
  padding: 28px 4px;
  line-height: 1.7;
}
.state.small { padding: 8px 4px; font-size: 12px; }
.notice {
  margin-top: 14px;
  font-size: 12.5px;
  color: var(--pd-accent-text);
  background: var(--pd-accent-soft);
  border-radius: 8px;
  padding: 8px 12px;
}

/* ---- 模型能力徽标与编辑弹窗 ---- */
.edit-btn {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 5px 10px;
  border-radius: 7px;
  border: 1px solid var(--pd-border);
  background: none;
  color: var(--pd-text-2);
  font-size: 12px;
  cursor: pointer;
  flex: none;
}
.edit-btn:hover { background: var(--pd-bg-hover); color: var(--pd-text); border-color: var(--pd-accent); }
.mr-badge {
  flex: none;
  font-size: 10.5px;
  padding: 1px 7px;
  border-radius: 99px;
  background: var(--pd-bg-hover);
  color: var(--pd-text-3);
}
.model-row { cursor: pointer; }
.me-card {
  width: 470px;
  max-width: 92vw;
  max-height: 86vh;
  overflow-y: auto;
  background: var(--pd-bg-raised);
  border: 1px solid var(--pd-border);
  border-radius: 14px;
  box-shadow: var(--pd-shadow);
  padding: 16px 18px;
  display: flex;
  flex-direction: column;
}
.me-head { display: flex; align-items: center; justify-content: space-between; margin-bottom: 10px; }
.me-head b { font-size: 14.5px; color: var(--pd-text); }
.me-x {
  background: none;
  border: none;
  color: var(--pd-text-3);
  cursor: pointer;
  padding: 4px;
  border-radius: 6px;
  display: grid;
  place-items: center;
}
.me-x:hover { background: var(--pd-bg-hover); color: var(--pd-text); }
.adv-toggle {
  display: flex;
  align-items: center;
  gap: 6px;
  background: none;
  border: none;
  color: var(--pd-text-2);
  font-size: 13px;
  cursor: pointer;
  padding: 6px 2px;
}
.adv-toggle svg { transition: transform 0.12s; }
.adv-toggle svg.fold { transform: rotate(-90deg); }
.adv-body { display: flex; flex-direction: column; gap: 8px; padding-left: 2px; }
.cap-chips { display: flex; gap: 8px; flex-wrap: wrap; }
/* .me-card 前缀提高特异性：压过 .field label/input 的块级与全宽样式 */
.me-card .cap-chips { display: flex; gap: 8px; flex-wrap: wrap; }
.me-card .cap-chip {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 5px 12px;
  border-radius: 99px;
  border: 1px solid var(--pd-border);
  background: var(--pd-bg);
  color: var(--pd-text-3);
  font-size: 12.5px;
  cursor: pointer;
  user-select: none;
  margin-bottom: 0;
}
.me-card .cap-chip.on { border-color: var(--pd-accent); color: var(--pd-accent-text); background: var(--pd-accent-soft); }
.me-card .cap-chip input {
  width: auto;
  padding: 0;
  margin: 0;
  accent-color: var(--pd-accent);
}
.me-card .cap-chip.locked { color: var(--pd-text-4); cursor: default; }
.mono { font-family: Consolas, monospace; font-size: 12px; }
.me-foot { display: flex; justify-content: flex-end; gap: 8px; margin-top: 12px; }
.me-cancel {
  background: none;
  border: none;
  color: var(--pd-text-2);
  font-size: 13px;
  cursor: pointer;
  padding: 7px 12px;
  border-radius: 8px;
}
.me-cancel:hover { background: var(--pd-bg-hover); color: var(--pd-text); }
</style>
