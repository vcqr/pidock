import { defineTool } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import type { AskQuestion } from "@pidock/protocol";

/** 单题答案：labels = 命中的选项 label；text = 自由输入；两者可并存 */
export interface AskAnswer {
  labels: string[];
  text?: string;
}

/** 防呆上限：单题文本长度与选项数（LLM 输出不可信） */
const MAX_CHARS = 500;
const MAX_OPTIONS = 8;
/** 单次调用最多几道题（逐题弹出，太多会刷屏） */
const MAX_QUESTIONS = 4;

/** 归一化 LLM 参数：丢空问题/空选项，截断超长文本；无有效问题返回 null */
export function sanitizeQuestion(raw: unknown): AskQuestion | null {
  if (!raw || typeof raw !== "object") return null;
  const rec = raw as Record<string, unknown>;
  const header = typeof rec.header === "string" ? rec.header.trim().slice(0, 60) : "";
  const question = typeof rec.question === "string" ? rec.question.trim().slice(0, MAX_CHARS) : "";
  if (!question) return null;
  const options: AskQuestion["options"] = [];
  if (Array.isArray(rec.options)) {
    for (const opt of rec.options.slice(0, MAX_OPTIONS)) {
      if (!opt || typeof opt !== "object") continue;
      const o = opt as Record<string, unknown>;
      const label = typeof o.label === "string" ? o.label.trim().slice(0, MAX_CHARS) : "";
      if (!label) continue;
      const description =
        typeof o.description === "string" && o.description.trim()
          ? o.description.trim().slice(0, MAX_CHARS)
          : undefined;
      options.push(description ? { label, description } : { label });
    }
  }
  if (!options.length) return null;
  return {
    header: header || "提问",
    question,
    options,
    ...(rec.multiSelect === true ? { multiSelect: true } : {}),
  };
}

/**
 * AskUserQuestion 自定义工具：把选择题经 ask 回调交给 pool（发 ask_user_question
 * 事件弹出问题卡 + 等待 session.resolve_ask），逐题作答后把答案文本返回给模型。
 * ask 回调返回 null 表示用户取消/打断（abort/关闭会话时由 pool 兜底触发）；
 * 返回 "timeout" 表示等待超时（ask.timeoutSec 配置，默认 3 分钟），模型应自行继续。
 */
export function createAskUserQuestionTool(
  ask: (question: AskQuestion) => Promise<AskAnswer | "timeout" | null>,
) {
  return defineTool({
    name: "AskUserQuestion",
    label: "Ask User Question",
    description:
      "Ask the user a question and let them pick from options (or type their own answer). " +
      "Use when you need the user to make a decision or provide missing information to proceed.",
    promptSnippet: "AskUserQuestion: ask the user a question with selectable options",
    promptGuidelines: [
      "When a decision is genuinely the user's to make (approach, trade-offs, scope), call AskUserQuestion with 2-4 concrete options instead of guessing; do not use it for facts you can look up yourself.",
    ],
    parameters: Type.Object({
      questions: Type.Array(
        Type.Object({
          header: Type.String({ description: "Very short label (max 12 chars)" }),
          question: Type.String({ description: "The question to ask the user" }),
          options: Type.Array(
            Type.Object({
              label: Type.String({ description: "Display label for the option" }),
              description: Type.Optional(
                Type.String({ description: "Optional explanation shown below the label" }),
              ),
            }),
            { description: "Available options for the user to choose from (2-4 recommended)" },
          ),
          multiSelect: Type.Optional(
            Type.Boolean({ description: "Allow selecting multiple options" }),
          ),
        }),
        { description: "Questions to ask the user, answered one by one" },
      ),
    }),
    executionMode: "sequential",

    async execute(_toolCallId, params) {
      const questions = (Array.isArray(params.questions) ? params.questions : [])
        .map(sanitizeQuestion)
        .filter((q): q is AskQuestion => q !== null)
        .slice(0, MAX_QUESTIONS);
      // 各 return 分支共用同一 details 形状（SDK 会按 execute 返回值推断泛型）
      const details: {
        error?: string;
        interrupted?: boolean;
        timeout?: boolean;
        answers?: Record<string, string>;
      } = {};
      if (!questions.length) {
        details.error = "no_valid_question";
        return {
          content: [
            { type: "text" as const, text: "没有有效的问题（每题需要 question 和非空 options）" },
          ],
          details,
        };
      }

      const answers: Record<string, string> = {};
      let interrupted = false;
      let timedOut = false;
      for (const q of questions) {
        const answer = await ask(q);
        if (answer === "timeout") {
          interrupted = true;
          timedOut = true;
          break;
        }
        if (answer === null) {
          interrupted = true;
          break;
        }
        const parts: string[] = [];
        if (answer.labels.length) parts.push(answer.labels.join(", "));
        if (answer.text) parts.push(answer.text);
        answers[`${q.header}: ${q.question}`] = parts.join("；") || "(未回答)";
      }

      const answerText = Object.entries(answers)
        .map(([q, a]) => `${q} → ${a}`)
        .join("\n");
      details.answers = answers;
      if (interrupted) {
        details.interrupted = true;
        if (timedOut) details.timeout = true;
        const note = timedOut
          ? "等待回答超时，视为用户暂不在线。请基于已有信息自行决策继续任务，之后不要重复问同一个问题。"
          : "用户取消了回答。";
        const text = Object.keys(answers).length ? `${note}已回答：\n${answerText}` : note;
        return {
          content: [{ type: "text" as const, text }],
          details,
        };
      }
      return {
        content: [{ type: "text" as const, text: answerText }],
        details,
      };
    },
  });
}
