import { defineTool } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import type { TodoItem } from "@pidock/protocol";

/**
 * TodoWrite 自定义工具：LLM 全量提交任务清单，经 onUpdate 回调交给 pool
 * 存储/推送 todo_updated 事件，UI 悬浮进度卡渲染。
 */
const TODO_STATUSES = ["pending", "in_progress", "completed"] as const;

/** 防呆上限：条数与单条文本长度（LLM 输出不可信） */
const MAX_TODOS = 50;
const MAX_CONTENT_CHARS = 500;

/** 归一化 LLM 参数或 JSONL 回放的 todos：非法状态归 pending、截断超长文本 */
export function sanitizeTodos(raw: unknown): TodoItem[] {
  if (!Array.isArray(raw)) return [];
  const items: TodoItem[] = [];
  for (const entry of raw.slice(0, MAX_TODOS)) {
    if (!entry || typeof entry !== "object") continue;
    const rec = entry as Record<string, unknown>;
    const content = typeof rec.content === "string" ? rec.content.trim().slice(0, MAX_CONTENT_CHARS) : "";
    if (!content) continue;
    const status = TODO_STATUSES.includes(rec.status as never) ? (rec.status as TodoItem["status"]) : "pending";
    const activeForm =
      typeof rec.activeForm === "string" && rec.activeForm.trim()
        ? rec.activeForm.trim().slice(0, MAX_CONTENT_CHARS)
        : typeof rec.active_form === "string" && rec.active_form.trim()
          ? rec.active_form.trim().slice(0, MAX_CONTENT_CHARS)
          : undefined;
    items.push(activeForm ? { content, status, active_form: activeForm } : { content, status });
  }
  return items;
}

export function createTodoWriteTool(onUpdate: (todos: TodoItem[]) => void) {
  return defineTool({
    name: "TodoWrite",
    label: "Todo Write",
    description:
      "Create or update a structured task list to track progress on complex multi-step work. " +
      "Each item has a status (pending, in_progress, completed). " +
      "Use this proactively for tasks with 3+ steps.",
    promptSnippet: "TodoWrite: manage a structured todo list to track task progress",
    promptGuidelines: [
      "For any task with 3 or more steps, first call TodoWrite with the full task list, then keep it up to date: mark exactly one item in_progress while working (set activeForm to the present-tense action) and completed as soon as it is done.",
    ],
    parameters: Type.Object({
      todos: Type.Array(
        Type.Object({
          content: Type.String({ description: "Description of the task" }),
          status: Type.Union(
            [Type.Literal("pending"), Type.Literal("in_progress"), Type.Literal("completed")],
            { description: "Status of the todo item" },
          ),
          activeForm: Type.Optional(
            Type.String({
              description:
                "Present-tense description of what you're currently doing (for in_progress items)",
            }),
          ),
        }),
        { description: "The full todo list (replaces previous list)" },
      ),
    }),
    executionMode: "sequential",

    async execute(_toolCallId, params) {
      const todos = sanitizeTodos(params.todos);
      onUpdate(todos);
      const completed = todos.filter((t) => t.status === "completed").length;
      return {
        content: [
          {
            type: "text" as const,
            text: `Todos updated. ${completed}/${todos.length} completed.`,
          },
        ],
        details: { count: todos.length, completed },
      };
    },
  });
}
