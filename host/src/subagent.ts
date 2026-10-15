import { defineTool } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

/**
 * Task 自定义工具：主 agent 派生池化子代理（host 内多开一个会话，非子进程）。
 * execute 阻塞到子代理回合结束，最终 assistant 文本作为 tool result 回传父会话。
 *
 * 配额与安全约束（pool 侧执行）：
 * - 深度 1：子代理会话不再注册本工具（也无法再派生）；
 * - 每父会话并发上限 MAX_SUBAGENTS_PER_PARENT，超限直接返回错误文本；
 * - 权限模式继承父会话当前模式（专家档案带 permission_mode 时优先），审批卡照常
 *   走 tool_approval 事件，在监控 dock / web 端裁决。
 */

/** 每父会话同时运行的子代理上限 */
export const MAX_SUBAGENTS_PER_PARENT = 5;

const MAX_PROMPT_CHARS = 20_000;
const MAX_LABEL_CHARS = 60;

export interface TaskSpawnParams {
  prompt: string;
  label?: string;
  expert_id?: string;
  model?: string;
  /** 回合预算（assistant 消息段数），超限自动中止；缺省走 settings 的全局默认 */
  maxTurns?: number;
}

export interface TaskSpawnResult {
  session_id: string;
  report: string;
}

/** LLM 参数防呆归一：截断超长文本、丢弃非字符串字段 */
export function sanitizeTaskParams(raw: Record<string, unknown>): TaskSpawnParams {
  const prompt = typeof raw.prompt === "string" ? raw.prompt.trim().slice(0, MAX_PROMPT_CHARS) : "";
  const label = typeof raw.label === "string" && raw.label.trim() ? raw.label.trim().slice(0, MAX_LABEL_CHARS) : undefined;
  const expertId = typeof raw.expert_id === "string" && raw.expert_id.trim() ? raw.expert_id.trim() : undefined;
  const model = typeof raw.model === "string" && raw.model.trim() ? raw.model.trim() : undefined;
  // 回合预算：1–500 防呆，非法值丢弃（走 settings 全局默认）
  const rawTurns = Number(raw.maxTurns);
  const maxTurns = Number.isFinite(rawTurns) && rawTurns >= 1 ? Math.min(Math.floor(rawTurns), 500) : undefined;
  return {
    prompt,
    ...(label ? { label } : {}),
    ...(expertId ? { expert_id: expertId } : {}),
    ...(model ? { model } : {}),
    ...(maxTurns ? { maxTurns } : {}),
  };
}

export function createTaskTool(
  spawn: (params: TaskSpawnParams) => Promise<TaskSpawnResult>,
  /** 可选：可用专家清单注入描述尾部，模型据此自主传 expert_id */
  expertsSuffix = "",
) {
  return defineTool({
    name: "Task",
    label: "Subagent",
    description:
      "Delegate a self-contained task to a subagent (a separate agent session running in parallel " +
      "in the background) and wait for its final report. " +
      "Use it for isolated sub-jobs: code review, research, long file analysis, bulk edits in a sandbox. " +
      "Give a complete, unambiguous prompt — the subagent shares your workspace and permissions but has " +
      "no access to this conversation. Subagents cannot spawn further subagents." + expertsSuffix,
    parameters: Type.Object({
      prompt: Type.String({ description: "Full task instructions for the subagent, including context it needs and the expected deliverable." }),
      label: Type.Optional(
        Type.String({ description: "Short display name for the subagent (shown in the monitor dock), e.g. 'Review pool.ts'." }),
      ),
      expert_id: Type.Optional(
        Type.String({ description: "Optional expert profile id (from the ExpertsView) to run the subagent as. Omit for a general-purpose subagent." }),
      ),
      model: Type.Optional(
        Type.String({ description: "Optional model override in 'provider/model-id' form. Omit to inherit the current model." }),
      ),
      maxTurns: Type.Optional(
        Type.Number({
          description:
            "Optional work budget: abort the subagent after this many assistant message segments (1-500). " +
            "Omit to use the global default.",
        }),
      ),
    }),
    // 允许同一回合多个 Task 调用并行执行（真正的子代理并发由这里产生）
    executionMode: "parallel",

    async execute(_toolCallId, params) {
      const clean = sanitizeTaskParams(params as Record<string, unknown>);
      if (!clean.prompt) {
        throw new Error("Task 参数缺少 prompt（需要完整的任务描述）");
      }
      const result = await spawn(clean);
      return {
        content: [{ type: "text" as const, text: result.report }],
        details: { session_id: result.session_id },
      };
    },
  });
}
