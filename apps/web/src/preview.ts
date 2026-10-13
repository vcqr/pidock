/**
 * 手机端适配自查预览页（仅开发用：vite dev 访问 /preview.html，不进构建产物）。
 * 用桩 DataBus 挂载修过的三个视图/组件，配合浏览器 375×812 视口截图核对。
 */
import { createApp, defineComponent, h, ref, computed } from "vue";
import "@pidock/ui/src/styles/tokens.css";
import "./style.css";
import Composer from "@pidock/ui/src/components/Composer.vue";
import AutomationView from "@pidock/ui/src/components/AutomationView.vue";
import ExpertsView from "@pidock/ui/src/components/ExpertsView.vue";
import Icon from "@pidock/ui/src/components/Icon.vue";
import type { DataBus } from "@pidock/ui/src/databus.js";

const bus: DataBus = {
  connected: true,
  async request(method: string, _params?: unknown): Promise<any> {
    switch (method) {
      case "automation.list":
        return {
          jobs: [
            {
              id: "j1",
              name: "每日站会摘要",
              prompt: "汇总昨日提交与今日计划",
              cron: "0 30 9 * * *",
              workspace: "/home/user/proj/demo",
              permission_mode: "full",
              enabled: true,
              created_at: 0,
              updated_at: 0,
              last_run: { run_id: "r1", trigger: "cron", started_at: Date.now() - 3600e3, finished_at: Date.now() - 3500e3, status: "ok" },
              runs: [],
              next_run_at: Date.now() + 86400e3,
            },
            {
              id: "j2",
              name: "每周依赖巡检",
              prompt: "检查依赖更新",
              cron: "0 0 10 * * 1",
              permission_mode: "full",
              enabled: false,
              created_at: 0,
              updated_at: 0,
              runs: [],
            },
          ],
        };
      case "session.list":
        return { home: "/home/user", sessions: [{ cwd: "/home/user/proj/demo" }] };
      case "config.models.list":
        return { models: ["anthropic/claude-sonnet-4-5", "openai/gpt-5.2", "deepseek/deepseek-chat"] };
      case "config.skills.list":
        return { skills: [{ name: "commit", description: "生成规范的提交信息" }] };
      case "config.extensions.list":
        return { extensions: [{ name: "fetch", file: "fetch.ts" }] };
      case "experts.list":
        return {
          experts: [
            {
              id: "e1",
              name: "code-reviewer",
              description: "严格但建设性的代码评审专家",
              icon: "code-line",
              prompt: "你是资深评审，关注正确性与可维护性。",
              skills: ["commit"],
              extensions: ["fetch"],
              knowledge_dirs: [],
              created_at: "",
              updated_at: "",
            },
          ],
        };
      default:
        return {};
    }
  },
  async onEvent(_handler: (event: any) => void): Promise<() => void> {
    return () => {};
  },
} as any;

const VIEWS = ["composer", "auto", "experts"] as const;
type View = (typeof VIEWS)[number];
const current = ref<View>((location.hash.slice(1) as View) || "composer");
const pick = (v: View): void => {
  current.value = v;
  location.hash = v;
};

const HomePage = defineComponent({
  setup() {
    const perm = ref("plan");
    const model = ref("anthropic/claude-sonnet-4-5");
    const think = ref("medium");
    return () =>
      // 模拟真实外壳：顶栏（节点胶囊）+ overflow:hidden 的主区（home-wrap 同款裁剪）
      h("div", { style: "height:100%;display:flex;flex-direction:column;background:var(--pd-bg)" }, [
        h("div", {
          style:
            "flex:none;height:44px;display:flex;align-items:center;gap:8px;padding:0 12px;border-bottom:1px solid var(--pd-border);background:var(--pd-bg-panel);font-size:12.5px;color:var(--pd-text)",
        }, [
          h("span", { style: "width:20px;height:20px;border-radius:6px;background:var(--pd-accent);display:grid;place-items:center;color:#1a1a1a;font-weight:700;font-size:12px" }, "P"),
          h("b", "vcyan-mac"),
          h("span", { style: "width:7px;height:7px;border-radius:50%;background:var(--pd-green)" }),
          h("span", { style: "margin-left:auto;color:var(--pd-text-3)" }, "空阅"),
        ]),
        h("div", { style: "flex:1;min-height:0;overflow:hidden;position:relative;display:grid;place-items:center;padding:24px 20px 48px" }, [
          h("div", { style: "width:min(560px,100%);display:flex;flex-direction:column" }, [
            h("p", { style: "text-align:center;font-size:21px;font-weight:600;margin:0 0 26px" }, "下午好"),
            h(Composer, {
              busy: false,
              centered: true,
              permissionMode: perm.value,
              thinkingLevel: think.value,
              thinkingAvailable: ["off", "minimal", "low", "medium", "high", "xhigh", "max"],
              model: model.value,
              models: ["anthropic/claude-sonnet-4-5", "openai/gpt-5.2", "deepseek/deepseek-chat"],
              "onSetPermissionMode": (m: string) => (perm.value = m),
              "onSetThinkingLevel": (l: string) => (think.value = l),
              "onSetModel": (m: string) => (model.value = m),
            }),
            h("div", { style: "display:flex;gap:12px;flex-wrap:wrap;justify-content:center;margin-top:30px" }, [
              ["帮我看看这个项目的结构", "修复当前项目里的报错", "给项目写一份 README", "代码评审最近的改动"].map((t) =>
                h("button", {
                  style:
                    "display:flex;align-items:center;gap:8px;padding:8px 16px;border-radius:10px;border:1px solid var(--pd-border);background:var(--pd-bg-card);color:var(--pd-text-2);font-size:13px",
                }, t),
              ),
            ]),
          ]),
        ]),
      ]);
  },
});

const App = defineComponent({
  setup() {
    const body = computed(() => {
      switch (current.value) {
        case "auto":
          return () => h(AutomationView, { bus, onClose: () => {}, onOpenProviders: () => {} });
        case "experts":
          return () => h(ExpertsView, { bus, onClose: () => {}, onOpenProviders: () => {} });
        default:
          return () => h(HomePage);
      }
    });
    return () => [
      h("div", { style: "height:100%;box-sizing:border-box" }, [h(body.value, { key: current.value })]),
      // 调试切换栏置于底部：不遮挡弹层展开的顶部区域
      h("div", { style: "position:fixed;bottom:0;left:0;right:0;z-index:999;display:flex;gap:8px;padding:6px 10px;background:#111;border-top:1px solid #333;font-size:12px;color:#eee" }, [
        ...VIEWS.map((v) =>
          h("button", { onClick: () => pick(v), style: `padding:4px 10px;border-radius:6px;border:1px solid #444;background:${current.value === v ? "#e8a33d" : "#222"};color:${current.value === v ? "#111" : "#eee"}` }, v),
        ),
        h("span", { style: "margin-left:auto;display:flex;align-items:center;gap:4px;color:#888" }, [
          h(Icon, { name: "smartphone-line", size: 14 }),
          "375×812 视口自查",
        ]),
      ]),
    ];
  },
});

createApp(App).mount("#app");
