<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import type { DataBus } from "../databus.js";
import { basename } from "../utils/time.js";
import Composer from "./Composer.vue";
import Icon from "./Icon.vue";

/**
 * 自动化（定时任务）：主区右栏页面（侧栏保留，左右分栏）。
 * 列表 + 新建/编辑弹窗 + 运行记录；提示词编辑复用主界面的 Composer
 * （draft 模式：工作空间 chip / 权限 / 模型 / 思考级别 / @ 文件 / / 技能 / 图片
 * 全部与主界面一致，send 上报给表单保存而不是发消息）。
 * 数据走 DataBus 的 automation.* 方法（桌面端由 Rust 调度器处理）。
 */

const props = defineProps<{ bus: DataBus }>();
const emit = defineEmits<{ close: []; "open-providers": [] }>();

interface RunRecord {
  run_id: string;
  trigger: string;
  started_at: number;
  finished_at?: number | null;
  status: string;
  session_id?: string | null;
  error?: string | null;
}
interface Job {
  id: string;
  name: string;
  prompt: string;
  cron: string;
  workspace?: string | null;
  model?: string | null;
  thinking_level?: string | null;
  images?: Array<{ data: string; mime_type: string }> | null;
  permission_mode: string;
  starts_at?: number | null;
  ends_at?: number | null;
  enabled: boolean;
  created_at: number;
  updated_at: number;
  last_run?: RunRecord | null;
  runs: RunRecord[];
  next_run_at?: number | null;
}

const jobs = ref<Job[]>([]);
const loading = ref(true);
const notice = ref("");
let noticeTimer: ReturnType<typeof setTimeout> | null = null;

function errText(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : String(e);
}
function flash(msg: string): void {
  notice.value = msg;
  if (noticeTimer) clearTimeout(noticeTimer);
  noticeTimer = setTimeout(() => (notice.value = ""), 2600);
}

// ---- 时间格式化 ----
function pad(n: number): string {
  return n < 10 ? `0${n}` : String(n);
}
function fmtMs(ms?: number | null): string {
  if (!ms) return "—";
  const d = new Date(ms);
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}
function fmtShort(ms?: number | null): string {
  if (!ms) return "—";
  const d = new Date(ms);
  return `${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}
function toLocalInput(ms?: number | null): string {
  if (!ms) return "";
  const d = new Date(ms);
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
}
function clampN(v: number, lo: number, hi: number): number {
  return Math.min(hi, Math.max(lo, Math.floor(v)));
}

// ---- cron 组装 / 解析 / 人话 ----
type FreqMode = "hourly" | "daily" | "weekly" | "everyN" | "custom";
const WEEK_CN = ["日", "一", "二", "三", "四", "五", "六"];

function buildCron(): string {
  const parts = (fTime.value || "09:00").split(":");
  const hh = clampN(parseInt(parts[0] ?? "0", 10) || 0, 0, 23);
  const mm = clampN(parseInt(parts[1] ?? "0", 10) || 0, 0, 59);
  switch (freqMode.value) {
    case "hourly":
      return `0 ${clampN(fMinute.value, 0, 59)} * * * *`;
    case "daily":
      return `0 ${mm} ${hh} * * *`;
    case "weekly":
      return `0 ${mm} ${hh} * * ${clampN(fDow.value, 0, 6)}`;
    case "everyN":
      return `0 */${clampN(fEveryN.value, 1, 1440)} * * * *`;
    default:
      return fCronExpr.value.trim();
  }
}
function parseCronToForm(cron: string): void {
  const p = cron.trim().split(/\s+/);
  const num = (x?: string) => (x && /^\d+$/.test(x) ? parseInt(x, 10) : null);
  const fits = p.length === 6 && p[0] === "0" && p[3] === "*" && p[4] === "*";
  if (fits) {
    const [, m, h, , , dow] = p;
    const mi = num(m);
    const hi = num(h);
    if (m?.startsWith("*/") && h === "*" && dow === "*" && /^\d+$/.test(m.slice(2))) {
      freqMode.value = "everyN";
      fEveryN.value = parseInt(m.slice(2), 10);
      return;
    }
    if (h === "*" && dow === "*" && mi !== null && hi === null) {
      freqMode.value = "hourly";
      fMinute.value = mi;
      return;
    }
    if (mi !== null && hi !== null && dow === "*") {
      freqMode.value = "daily";
      fTime.value = `${pad(hi)}:${pad(mi)}`;
      return;
    }
    if (mi !== null && hi !== null && dow !== "*" && num(dow) !== null) {
      freqMode.value = "weekly";
      fTime.value = `${pad(hi)}:${pad(mi)}`;
      fDow.value = num(dow)!;
      return;
    }
  }
  freqMode.value = "custom";
  fCronExpr.value = cron;
}
function describeCron(cron: string): string {
  const p = cron.trim().split(/\s+/);
  const num = (x?: string) => (x && /^\d+$/.test(x) ? parseInt(x, 10) : null);
  if (p.length !== 6 || p[0] !== "0" || p[3] !== "*" || p[4] !== "*") return `cron ${cron}`;
  const [, m, h, , , dow] = p;
  const mi = num(m);
  const hi = num(h);
  if (m?.startsWith("*/") && h === "*" && dow === "*" && /^\d+$/.test(m.slice(2))) return `每 ${m.slice(2)} 分钟`;
  if (h === "*" && dow === "*" && mi !== null && hi === null) return `每小时第 ${m} 分`;
  if (mi !== null && hi !== null && dow === "*") return `每天 ${pad(hi)}:${pad(mi)}`;
  if (mi !== null && hi !== null && num(dow) !== null) return `每周${WEEK_CN[num(dow)!]} ${pad(hi)}:${pad(mi)}`;
  return `cron ${cron}`;
}

// ---- 列表 ----
async function load(): Promise<void> {
  try {
    const r = await props.bus.request("automation.list", {});
    jobs.value = r?.jobs ?? [];
  } catch (e) {
    flash(`加载失败：${errText(e)}`);
  } finally {
    loading.value = false;
  }
}
let reloadTimer: ReturnType<typeof setTimeout> | null = null;
function scheduleReload(): void {
  if (reloadTimer) clearTimeout(reloadTimer);
  reloadTimer = setTimeout(() => void load(), 250);
}
let unlisten: (() => void) | null = null;
onMounted(async () => {
  await load();
  unlisten = await props.bus.onEvent((e) => {
    if (typeof e?.kind === "string" && e.kind.startsWith("automation.")) scheduleReload();
  });
});
onBeforeUnmount(() => {
  unlisten?.();
  if (reloadTimer) clearTimeout(reloadTimer);
  if (noticeTimer) clearTimeout(noticeTimer);
});

function jobState(j: Job): { label: string; cls: string } {
  const now = Date.now();
  if (j.last_run?.status === "running") return { label: "运行中", cls: "running" };
  if (!j.enabled) return { label: "已停用", cls: "off" };
  if (j.ends_at && now >= j.ends_at) return { label: "已过期", cls: "expired" };
  if (j.starts_at && now < j.starts_at) return { label: "未开始", cls: "pending" };
  return { label: "已启用", cls: "on" };
}
function lastRunChip(j: Job): { text: string; cls: string; title: string } | null {
  const r = j.last_run;
  if (!r) return null;
  if (r.status === "running") return { text: "⟳ 运行中", cls: "running", title: `开始于 ${fmtMs(r.started_at)}` };
  if (r.status === "ok") return { text: `✓ ${fmtShort(r.finished_at ?? r.started_at)}`, cls: "ok", title: "" };
  if (r.status === "skipped") return { text: "⤼ 跳过", cls: "skip", title: r.error ?? "" };
  return { text: "✗ 失败", cls: "failed", title: r.error ?? "" };
}
function wsLabel(j: Job): string {
  return j.workspace ? basename(j.workspace) : "主目录";
}
function jobDesc(j: Job): string {
  const next = j.next_run_at ? `下次 ${fmtShort(j.next_run_at)}` : jobState(j).label;
  return `${describeCron(j.cron)} · ${wsLabel(j)} · ${next}`;
}

async function toggle(j: Job): Promise<void> {
  const next = !j.enabled;
  j.enabled = next;
  try {
    await props.bus.request("automation.set_enabled", { id: j.id, enabled: next });
    flash(next ? "已启用" : "已停用");
  } catch (e) {
    j.enabled = !next;
    flash(errText(e));
  }
}
async function runNow(j: Job): Promise<void> {
  try {
    await props.bus.request("automation.run_now", { id: j.id });
    flash(`已触发「${j.name}」，稍后可在会话列表查看`);
  } catch (e) {
    flash(errText(e));
  }
}
async function removeJob(j: Job): Promise<void> {
  if (!window.confirm(`删除定时任务「${j.name}」？`)) return;
  try {
    await props.bus.request("automation.delete", { id: j.id });
    flash("已删除");
    await load();
  } catch (e) {
    flash(errText(e));
  }
}

// ---- Composer 上下文（工作空间 / 模型 / @ 文件 / 技能） ----
const projects = ref<string[]>([]);
const homeDir = ref<string | null>(null);
const modelOptions = ref<string[]>([]);
async function loadComposerContext(): Promise<void> {
  try {
    const r = await props.bus.request("session.list");
    const home: string = r?.home ?? "";
    homeDir.value = home || null;
    const seen = new Set<string>();
    const list: string[] = [];
    for (const s of r?.sessions ?? []) {
      const cwd: string | undefined = s?.cwd;
      if (!cwd || cwd === home || seen.has(cwd)) continue;
      seen.add(cwd);
      list.push(cwd);
    }
    projects.value = list;
  } catch {
    projects.value = [];
  }
  try {
    const r = await props.bus.request("config.models.list");
    modelOptions.value = (r?.models ?? [])
      .map((m: unknown) =>
        typeof m === "string" ? m : `${(m as any)?.provider}/${(m as any)?.id}`,
      )
      .filter((x: string) => x && x !== "undefined/undefined");
  } catch {
    modelOptions.value = [];
  }
}
async function mentionLoader(cwd: string): Promise<Array<{ path: string; name: string; dir: boolean }>> {
  try {
    const r = await props.bus.request("workspace.files", { cwd });
    return r?.files ?? [];
  } catch {
    return [];
  }
}
async function skillsLoader(): Promise<Array<{ name: string; description: string }>> {
  try {
    const r = await props.bus.request("config.skills.list", {});
    return (r?.skills ?? [])
      .filter((s: any) => s.enabled !== false)
      .map((s: any) => ({ name: String(s.name ?? ""), description: String(s.description ?? "") }));
  } catch {
    return [];
  }
}

// ---- 新建 / 编辑弹窗 ----
const showForm = ref(false);
const formMode = ref<"add" | "edit">("add");
const editingId = ref<string | null>(null);
/** 每次打开弹窗自增，作为 Composer 的 key 强制重挂载以应用 preset */
const dialogSeq = ref(0);
const saving = ref(false);
const fName = ref("");
const fPrompt = ref("");
const freqMode = ref<FreqMode>("daily");
const fTime = ref("09:00");
const fDow = ref(1);
const fMinute = ref(0);
const fEveryN = ref(30);
const fCronExpr = ref("");
const effMode = ref<"forever" | "range">("forever");
const fStart = ref("");
const fEnd = ref("");
/** 由 Composer 的工作空间 chip / 权限 / 模型 / 思考级别菜单回写 */
const fPerm = ref("full");
const fModel = ref("");
const fThink = ref("");

const peekTimes = ref<number[]>([]);
const peekErr = ref("");

const dialogCron = computed(() => (showForm.value ? buildCron() : ""));
let peekTimer: ReturnType<typeof setTimeout> | null = null;
watch(dialogCron, (cron) => {
  if (!showForm.value || !cron) return;
  if (peekTimer) clearTimeout(peekTimer);
  peekTimer = setTimeout(() => void doPeek(cron), 350);
});
async function doPeek(cron: string): Promise<void> {
  peekErr.value = "";
  peekTimes.value = [];
  try {
    const r = await props.bus.request("automation.peek", { cron, count: 3 });
    peekTimes.value = r?.times ?? [];
    if (!peekTimes.value.length) peekErr.value = "无法计算下次触发时间";
  } catch (e) {
    peekErr.value = errText(e);
  }
}

function openAdd(): void {
  editingId.value = null;
  formMode.value = "add";
  fName.value = "";
  fPrompt.value = "";
  freqMode.value = "daily";
  fTime.value = "09:00";
  fDow.value = 1;
  fMinute.value = 0;
  fEveryN.value = 30;
  fCronExpr.value = "";
  effMode.value = "forever";
  fStart.value = "";
  fEnd.value = "";
  fPerm.value = "full";
  fModel.value = "";
  fThink.value = "";
  peekTimes.value = [];
  peekErr.value = "";
  dialogSeq.value += 1;
  showForm.value = true;
  void loadComposerContext();
}
function openEdit(j: Job): void {
  editingId.value = j.id;
  formMode.value = "edit";
  fName.value = j.name;
  fPrompt.value = j.prompt;
  parseCronToForm(j.cron);
  effMode.value = j.starts_at || j.ends_at ? "range" : "forever";
  fStart.value = toLocalInput(j.starts_at);
  fEnd.value = toLocalInput(j.ends_at);
  fPerm.value = j.permission_mode || "full";
  fModel.value = j.model ?? "";
  fThink.value = j.thinking_level ?? "";
  peekTimes.value = [];
  peekErr.value = "";
  dialogSeq.value += 1;
  showForm.value = true;
  void loadComposerContext();
}

/** Composer 的 send（Enter / ↑ 按钮）= 保存任务 */
async function applyDraft(
  text: string,
  cwd?: string | null,
  images?: Array<{ data: string; mime_type: string }>,
): Promise<void> {
  if (!fName.value.trim()) return flash("请先填写任务名称");
  if (!buildCron()) return flash("cron 表达式不能为空");
  if (effMode.value === "range") {
    if (!fStart.value || !fEnd.value) return flash("请填写开始和结束时间");
    if (new Date(fStart.value).getTime() >= new Date(fEnd.value).getTime()) return flash("开始时间必须早于结束时间");
  }
  saving.value = true;
  try {
    await props.bus.request("automation.save", {
      id: editingId.value ?? "",
      name: fName.value,
      prompt: text,
      cron: buildCron(),
      workspace: cwd ?? null,
      model: fModel.value || null,
      thinking_level: fThink.value || null,
      images: images?.length ? images : null,
      permission_mode: fPerm.value,
      starts_at: effMode.value === "range" && fStart.value ? new Date(fStart.value).getTime() : null,
      ends_at: effMode.value === "range" && fEnd.value ? new Date(fEnd.value).getTime() : null,
      enabled: editingId.value ? (jobs.value.find((j) => j.id === editingId.value)?.enabled ?? true) : true,
    });
    showForm.value = false;
    flash(formMode.value === "add" ? "已创建定时任务" : "已保存修改");
    await load();
  } catch (e) {
    flash(errText(e));
  } finally {
    saving.value = false;
  }
}

// ---- 运行记录弹窗 ----
const histJob = ref<Job | null>(null);
function statusIcon(s: string): string {
  if (s === "ok") return "✓";
  if (s === "failed") return "✗";
  if (s === "running") return "⟳";
  return "⤼";
}
function statusLabel(s: string): string {
  if (s === "ok") return "成功";
  if (s === "failed") return "失败";
  if (s === "running") return "运行中";
  return "跳过";
}
function durText(r: RunRecord): string {
  if (!r.finished_at) return "";
  const sec = Math.max(0, Math.round((r.finished_at - r.started_at) / 1000));
  if (sec < 60) return `${sec}s`;
  return `${Math.floor(sec / 60)}m${sec % 60}s`;
}
function copyRunId(sid: string): void {
  void navigator.clipboard?.writeText(sid);
  flash("已复制会话 ID");
}
</script>

<template>
  <div class="auto-page">
    <div class="page-body">
      <div class="wrap">
        <header class="head">
          <div class="title-block">
            <h2>
              自动化
              <span v-if="!loading" class="count">{{ jobs.length }}</span>
            </h2>
            <p class="sub">按计划自动创建新会话并执行提示词；应用运行期间调度，错过的时点不补跑。</p>
          </div>
          <div class="head-actions">
            <button class="ghost-btn" title="刷新" @click="load">
              <Icon name="refresh-line" :size="15" />
            </button>
            <button class="ghost-btn" title="返回会话" @click="emit('close')">
              <Icon name="close-line" :size="15" />
            </button>
            <button class="dark-btn" @click="openAdd">
              <Icon name="add-line" :size="14" />新建任务
            </button>
          </div>
        </header>

        <div class="list">
          <div v-if="loading" class="state">加载中…</div>
          <template v-else>
            <div v-for="j in jobs" :key="j.id" class="row" :class="{ dim: !j.enabled }">
              <span class="tile"><Icon name="time-line" :size="18" /></span>
              <div class="info">
                <div class="name-row">
                  <b>{{ j.name }}</b>
                  <span class="state-badge" :class="jobState(j).cls">{{ jobState(j).label }}</span>
                  <span
                    v-if="lastRunChip(j)"
                    class="last-run"
                    :class="lastRunChip(j)!.cls"
                    :title="lastRunChip(j)!.title"
                  >{{ lastRunChip(j)!.text }}</span>
                </div>
                <div class="sub-text" :title="jobDesc(j)">{{ jobDesc(j) }}</div>
              </div>
              <label class="switch" :title="j.enabled ? '点击停用' : '点击启用'" @click.stop>
                <input type="checkbox" :checked="j.enabled" @change="toggle(j)" />
                <span class="slider"></span>
              </label>
              <button class="row-btn" title="立即运行" @click.stop="runNow(j)">
                <Icon name="play-circle-line" :size="14" />
              </button>
              <button class="row-btn" title="运行记录" @click.stop="histJob = j">
                <Icon name="history-line" :size="14" />
              </button>
              <button class="row-btn" title="编辑" @click.stop="openEdit(j)">
                <Icon name="edit-2-line" :size="14" />
              </button>
              <button class="row-btn danger" title="删除" @click.stop="removeJob(j)">
                <Icon name="delete-bin-line" :size="14" />
              </button>
            </div>
            <div v-if="!jobs.length" class="state">还没有定时任务，点右上角「新建任务」创建一个。</div>
          </template>
        </div>

        <div v-if="notice" class="notice">{{ notice }}</div>
      </div>
    </div>

    <!-- 新建 / 编辑弹窗：名称 + 频率/时效 + 主界面同款 Composer -->
    <div v-if="showForm" class="dialog-mask" @click.self="showForm = false">
      <div class="dialog">
        <header class="d-head">
          <h2>{{ formMode === "add" ? "新建定时任务" : "编辑定时任务" }}</h2>
          <button class="d-close" title="关闭" @click="showForm = false">
            <Icon name="close-line" :size="15" />
          </button>
        </header>
        <p class="d-sub">到点后自动创建新会话执行这里的提示词，会话与普通会话一样出现在左侧列表。</p>

        <div class="field">
          <label>名称 <i>*</i></label>
          <input v-model="fName" placeholder="例如：每日站会摘要" />
        </div>

        <div class="field">
          <label>执行频率 <i>*</i></label>
          <select v-model="freqMode">
            <option value="daily">每天</option>
            <option value="weekly">每周</option>
            <option value="hourly">每小时</option>
            <option value="everyN">每隔 N 分钟</option>
            <option value="custom">自定义 Cron</option>
          </select>
        </div>

        <div class="field freq-row">
          <template v-if="freqMode === 'daily'">
            <span class="lbl">每天</span>
            <input v-model="fTime" type="time" />
          </template>
          <template v-else-if="freqMode === 'weekly'">
            <span class="lbl">每</span>
            <select v-model="fDow" class="dow">
              <option v-for="(w, i) in WEEK_CN" :key="i" :value="i">周{{ w }}</option>
            </select>
            <input v-model="fTime" type="time" />
          </template>
          <template v-else-if="freqMode === 'hourly'">
            <span class="lbl">每小时第</span>
            <input v-model.number="fMinute" type="number" min="0" max="59" />
            <span class="lbl">分</span>
          </template>
          <template v-else-if="freqMode === 'everyN'">
            <span class="lbl">每隔</span>
            <input v-model.number="fEveryN" type="number" min="1" max="1440" />
            <span class="lbl">分钟</span>
          </template>
          <template v-else>
            <input v-model="fCronExpr" class="cron-input" placeholder="0 30 9 * * *（秒 分 时 日 月 周）" spellcheck="false" />
          </template>
        </div>

        <div v-if="peekErr" class="peek err">{{ peekErr }}</div>
        <div v-else-if="peekTimes.length" class="peek">
          接下来：{{ peekTimes.map((t) => fmtShort(t)).join("、") }}
        </div>

        <div class="field">
          <label>任务时效</label>
          <div class="seg">
            <button :class="{ on: effMode === 'forever' }" @click="effMode = 'forever'">长期</button>
            <button :class="{ on: effMode === 'range' }" @click="effMode = 'range'">限定区间</button>
          </div>
        </div>
        <div v-if="effMode === 'range'" class="range-row">
          <div class="field half">
            <label>开始</label>
            <input v-model="fStart" type="datetime-local" />
          </div>
          <div class="field half">
            <label>结束</label>
            <input v-model="fEnd" type="datetime-local" />
          </div>
        </div>

        <div class="composer-slot">
          <Composer
            :key="dialogSeq"
            draft
            centered
            :busy="false"
            :model="fModel || undefined"
            :models="modelOptions"
            :permission-mode="fPerm"
            :thinking-level="fThink || undefined"
            :mention-cwd="homeDir ?? undefined"
            :mention-loader="mentionLoader"
            :skills-loader="skillsLoader"
            :projects="projects"
            placeholder="添加提示词，Enter 保存"
            :preset="fPrompt"
            @send="applyDraft"
            @set-permission-mode="(m: string) => (fPerm = m)"
            @set-thinking-level="(l: string) => (fThink = l)"
            @set-model="(m: string) => (fModel = m)"
            @open-providers="emit('open-providers')"
          />
          <p v-if="fPerm !== 'full'" class="field-hint">
            无人值守运行建议在下方权限菜单选「完全访问」，否则修改类工具会挂起等待审批。
          </p>
        </div>

        <div v-if="notice" class="notice">{{ notice }}</div>

        <footer class="d-foot">
          <span class="foot-hint">Enter 或 ↑ 保存任务</span>
          <button class="cancel" @click="showForm = false">取消</button>
        </footer>
      </div>
    </div>

    <!-- 运行记录弹窗 -->
    <div v-if="histJob" class="dialog-mask" @click.self="histJob = null">
      <div class="dialog dialog-hist">
        <header class="d-head">
          <h2>运行记录 · {{ histJob.name }}</h2>
          <button class="d-close" title="关闭" @click="histJob = null">
            <Icon name="close-line" :size="15" />
          </button>
        </header>
        <p class="d-sub">最多保留最近 20 次记录。</p>
        <div v-if="!histJob.runs.length" class="state">还没有运行记录</div>
        <div v-else class="hist-list">
          <div v-for="r in [...histJob.runs].reverse()" :key="r.run_id" class="hist-row">
            <span class="hist-ic" :class="r.status">{{ statusIcon(r.status) }}</span>
            <div class="hist-main">
              <div class="hist-line">
                {{ fmtMs(r.started_at) }} · {{ r.trigger === "manual" ? "手动" : "定时" }}<template v-if="durText(r)"> · {{ durText(r) }}</template>
                <a v-if="r.session_id" class="hist-sid" title="复制会话 ID" @click="copyRunId(r.session_id)">会话</a>
              </div>
              <div v-if="r.error" class="hist-err">{{ r.error }}</div>
            </div>
            <span class="hist-st" :class="r.status">{{ statusLabel(r.status) }}</span>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.auto-page {
  flex: 1;
  min-width: 0;
  min-height: 0;
  display: flex;
  flex-direction: column;
  background: var(--pd-bg);
}
.page-body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
}
.wrap {
  max-width: 860px;
  margin: 0 auto;
  padding: 24px 32px 48px;
}
.head {
  display: flex;
  align-items: flex-start;
  gap: 16px;
}
.title-block { flex: 1; min-width: 0; }
h2 {
  margin: 0;
  font-size: 19px;
  font-weight: 700;
  color: var(--pd-text);
  display: flex;
  align-items: center;
  gap: 10px;
}
.count {
  font-size: 12px;
  font-weight: 600;
  color: var(--pd-text-3);
  background: var(--pd-bg-card);
  border: 1px solid var(--pd-border);
  border-radius: 999px;
  padding: 1px 9px;
}
.sub {
  margin: 5px 0 0;
  font-size: 12.5px;
  color: var(--pd-text-3);
  line-height: 1.5;
}
.head-actions { display: flex; align-items: center; gap: 8px; flex: none; }
.ghost-btn {
  width: 30px;
  height: 30px;
  display: grid;
  place-items: center;
  background: none;
  border: 1px solid var(--pd-border);
  border-radius: 8px;
  color: var(--pd-text-3);
  cursor: pointer;
}
.ghost-btn:hover { color: var(--pd-text); background: var(--pd-bg-hover); }
.dark-btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  background: var(--pd-text);
  color: var(--pd-bg);
  border: none;
  border-radius: 9px;
  padding: 7px 14px;
  font-size: 13px;
  font-weight: 600;
  cursor: pointer;
}
.dark-btn:hover { background: var(--pd-accent); color: #1a1a1a; }

.list { margin-top: 14px; }
.row {
  display: flex;
  align-items: center;
  gap: 14px;
  background: var(--pd-bg-card);
  border: 1px solid var(--pd-border-soft);
  border-radius: 12px;
  padding: 13px 16px;
  margin-bottom: 10px;
  transition: border-color 0.15s;
}
.row:hover { border-color: var(--pd-border); }
.row.dim .name-row b { color: var(--pd-text-3); }
.row.dim .sub-text { color: var(--pd-text-4); }
.row.dim .tile { opacity: 0.45; }
.tile {
  width: 38px;
  height: 38px;
  flex: none;
  border-radius: 10px;
  display: grid;
  place-items: center;
  background: var(--pd-accent-soft);
  color: var(--pd-accent);
}
.info { flex: 1; min-width: 0; }
.name-row {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
}
.name-row b {
  color: var(--pd-text);
  font-size: 14px;
  font-weight: 600;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.state-badge {
  font-size: 10.5px;
  border-radius: 5px;
  padding: 1.5px 8px;
  flex: none;
}
.state-badge.on { color: var(--pd-green-text); background: var(--pd-green-soft); }
.state-badge.running { color: var(--pd-accent-text); background: var(--pd-accent-soft); }
.state-badge.off { color: var(--pd-text-3); background: var(--pd-bg-hover); }
.state-badge.pending { color: var(--pd-yellow-text); background: var(--pd-yellow-soft); }
.state-badge.expired { color: var(--pd-red-text); background: var(--pd-red-soft); }
.last-run {
  font-size: 10.5px;
  border-radius: 5px;
  padding: 1.5px 7px;
  flex: none;
  cursor: default;
}
.last-run.ok { color: var(--pd-green-text); background: var(--pd-green-soft); }
.last-run.failed { color: var(--pd-red-text); background: var(--pd-red-soft); }
.last-run.running { color: var(--pd-accent-text); background: var(--pd-accent-soft); }
.last-run.skip { color: var(--pd-text-3); background: var(--pd-bg-hover); }
.sub-text {
  margin-top: 4px;
  font-size: 12.5px;
  color: var(--pd-text-3);
  line-height: 1.55;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.row-btn {
  width: 28px;
  height: 28px;
  flex: none;
  display: grid;
  place-items: center;
  background: none;
  border: none;
  border-radius: 7px;
  color: var(--pd-text-4);
  cursor: pointer;
}
.row-btn:hover { background: var(--pd-bg-hover); color: var(--pd-text); }
.row-btn.danger:hover { background: var(--pd-red-soft); color: var(--pd-red-text); }

.switch {
  position: relative;
  flex: none;
  width: 42px;
  height: 23px;
  cursor: pointer;
}
.switch input {
  position: absolute;
  opacity: 0;
  width: 0;
  height: 0;
}
.slider {
  position: absolute;
  inset: 0;
  border-radius: 999px;
  background: var(--pd-bg-hover);
  border: 1px solid var(--pd-border);
  transition: background 0.15s, border-color 0.15s;
}
.slider::before {
  content: "";
  position: absolute;
  width: 17px;
  height: 17px;
  border-radius: 50%;
  top: 2px;
  left: 2px;
  background: var(--pd-text-3);
  transition: transform 0.15s, background 0.15s;
}
.switch input:checked + .slider {
  background: var(--pd-green);
  border-color: var(--pd-green);
}
.switch input:checked + .slider::before {
  transform: translateX(19px);
  background: #fff;
}

.state {
  color: var(--pd-text-4);
  font-size: 13px;
  padding: 28px 4px;
  line-height: 1.7;
}

.notice {
  margin-top: 14px;
  font-size: 12.5px;
  color: var(--pd-accent-text);
  background: var(--pd-accent-soft);
  border-radius: 8px;
  padding: 8px 12px;
}

/* ---- dialog ---- */
.dialog-mask {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.55);
  display: grid;
  place-items: center;
  z-index: 120;
}
.dialog {
  width: min(640px, calc(100vw - 48px));
  max-height: calc(100vh - 80px);
  overflow-y: auto;
  background: var(--pd-bg-raised);
  border: 1px solid var(--pd-border);
  border-radius: 14px;
  box-shadow: var(--pd-shadow);
  padding: 20px 22px 18px;
}
.d-head {
  display: flex;
  align-items: center;
  gap: 12px;
}
.d-head h2 {
  margin: 0;
  font-size: 17px;
  font-weight: 700;
  color: var(--pd-text);
  flex: 1;
}
.d-close {
  width: 28px;
  height: 28px;
  display: grid;
  place-items: center;
  background: none;
  border: none;
  border-radius: 7px;
  color: var(--pd-text-3);
  cursor: pointer;
}
.d-close:hover { background: var(--pd-bg-hover); color: var(--pd-text); }
.d-sub {
  margin: 6px 0 0;
  font-size: 12.5px;
  color: var(--pd-text-3);
  line-height: 1.55;
}
.field { margin-top: 16px; }
.field label {
  display: block;
  font-size: 13px;
  color: var(--pd-text-2);
  margin-bottom: 7px;
}
.field label i {
  color: var(--pd-red);
  font-style: normal;
}
.field input,
.field select {
  width: 100%;
  box-sizing: border-box;
  background: var(--pd-bg-card);
  border: 1px solid var(--pd-border);
  border-radius: 9px;
  color: var(--pd-text);
  font-size: 13px;
  font-family: inherit;
  padding: 9px 12px;
}
.field select option { background: var(--pd-bg-raised); }
.field input:focus,
.field select:focus {
  outline: none;
  border-color: var(--pd-accent);
}
.field input::placeholder { color: var(--pd-text-4); }
:global([data-theme="dark"]) .field input[type="time"],
:global([data-theme="dark"]) .field input[type="datetime-local"],
:global([data-theme="dark"]) .field input[type="number"] {
  color-scheme: dark;
}

.freq-row { display: flex; align-items: center; gap: 8px; }
.freq-row .lbl { font-size: 12.5px; color: var(--pd-text-3); flex: none; }
.freq-row input,
.freq-row select {
  width: auto;
  flex: none;
}
.freq-row input[type="time"] { width: 120px; }
.freq-row input[type="number"] { width: 84px; }
.freq-row select.dow { width: 100px; }
.freq-row .cron-input { flex: 1; min-width: 0; font-family: Consolas, monospace; }

.peek {
  margin-top: 10px;
  font-size: 12px;
  color: var(--pd-text-3);
  background: var(--pd-bg-card);
  border: 1px solid var(--pd-border-soft);
  border-radius: 8px;
  padding: 7px 10px;
  line-height: 1.6;
}
.peek.err {
  color: var(--pd-red-text);
  background: var(--pd-red-soft);
  border-color: transparent;
}

.seg {
  display: inline-flex;
  background: var(--pd-bg-raised);
  border: 1px solid var(--pd-border);
  border-radius: 9px;
  padding: 2px;
  gap: 2px;
}
.seg button {
  background: none;
  border: none;
  border-radius: 7px;
  color: var(--pd-text-3);
  font-size: 12.5px;
  padding: 5px 12px;
  cursor: pointer;
}
.seg button:hover { color: var(--pd-text); }
.seg button.on { background: var(--pd-bg-active); color: var(--pd-text); font-weight: 600; }

.range-row { display: flex; gap: 12px; }
.range-row .half { flex: 1; min-width: 0; }

.composer-slot { margin-top: 16px; }
.field-hint {
  margin: 7px 0 0;
  font-size: 12px;
  color: var(--pd-text-4);
  line-height: 1.5;
}

.d-foot {
  display: flex;
  justify-content: flex-end;
  align-items: center;
  gap: 10px;
  margin-top: 18px;
}
.foot-hint {
  flex: 1;
  font-size: 12px;
  color: var(--pd-text-4);
}
.cancel {
  background: none;
  border: 1px solid var(--pd-border);
  color: var(--pd-text-2);
  border-radius: 9px;
  padding: 7px 16px;
  font-size: 13px;
  cursor: pointer;
}
.cancel:hover { background: var(--pd-bg-hover); color: var(--pd-text); }

/* ---- 运行记录 ---- */
.dialog-hist { width: min(560px, calc(100vw - 48px)); }
.hist-list {
  margin-top: 14px;
  max-height: 50vh;
  overflow-y: auto;
}
.hist-row {
  display: flex;
  align-items: flex-start;
  gap: 10px;
  padding: 9px 4px;
  border-bottom: 1px solid var(--pd-border-soft);
}
.hist-row:last-child { border-bottom: none; }
.hist-ic {
  width: 20px;
  text-align: center;
  flex: none;
  font-size: 13px;
  padding-top: 1px;
}
.hist-ic.ok { color: var(--pd-green); }
.hist-ic.failed { color: var(--pd-red-text); }
.hist-ic.running { color: var(--pd-accent); }
.hist-ic.skipped { color: var(--pd-text-4); }
.hist-main { flex: 1; min-width: 0; }
.hist-line {
  font-size: 12.5px;
  color: var(--pd-text-2);
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}
.hist-sid {
  font-size: 11px;
  color: var(--pd-accent-text);
  cursor: pointer;
}
.hist-sid:hover { text-decoration: underline; }
.hist-err {
  margin-top: 3px;
  font-size: 12px;
  color: var(--pd-red-text);
  word-break: break-all;
}
.hist-st {
  font-size: 11px;
  color: var(--pd-text-4);
  flex: none;
  padding-top: 2px;
}
.hist-st.ok { color: var(--pd-green-text); }
.hist-st.failed { color: var(--pd-red-text); }
.hist-st.running { color: var(--pd-accent-text); }
</style>
