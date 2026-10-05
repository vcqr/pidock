import { describe, expect, test } from "bun:test";
import { createAskUserQuestionTool, sanitizeQuestion } from "./ask.js";
import type { AskAnswer } from "./ask.js";
import type { AskQuestion } from "@pidock/protocol";

/** 直接调用 defineTool 产物的 execute（测试不关心 signal/onUpdate/ctx） */
async function runExecute(
  tool: ReturnType<typeof createAskUserQuestionTool>,
  params: { questions: unknown[] },
) {
  return (tool.execute as unknown as (id: string, p: unknown) => Promise<any>)("call1", params);
}

describe("sanitizeQuestion", () => {
  test("丢空问题/空选项，保留 header/question/选项", () => {
    const q = sanitizeQuestion({
      header: "模式",
      question: "选一个",
      options: [{ label: "A", description: "说明" }, { label: "  " }, null],
    });
    expect(q).toEqual({
      header: "模式",
      question: "选一个",
      options: [{ label: "A", description: "说明" }],
    });
  });

  test("无问题或无有效选项返回 null", () => {
    expect(sanitizeQuestion(null)).toBe(null);
    expect(sanitizeQuestion({ question: "  ", options: [{ label: "A" }] })).toBe(null);
    expect(sanitizeQuestion({ question: "Q", options: [] })).toBe(null);
  });

  test("超长文本截断，multiSelect=true 保留", () => {
    const q = sanitizeQuestion({
      header: "h",
      question: "x".repeat(600),
      options: [{ label: "y".repeat(600) }],
      multiSelect: true,
    });
    expect(q?.question.length).toBe(500);
    expect(q?.options[0]?.label.length).toBe(500);
    expect(q?.multiSelect).toBe(true);
  });
});

describe("AskUserQuestion 工具", () => {
  const q1: AskQuestion = { header: "模式", question: "选哪个", options: [{ label: "Alpha" }, { label: "Beta" }] };

  test("逐题作答并把 label/自由文本聚合为答案", async () => {
    const seen: AskQuestion[] = [];
    const queue: (AskAnswer | null)[] = [
      { labels: ["Beta"] },
      { labels: ["A", "B"], text: "另注" },
    ];
    const tool = createAskUserQuestionTool((q) => {
      seen.push(q);
      return Promise.resolve(queue.shift() ?? null);
    });
    const result = await runExecute(tool, {
      questions: [q1, { header: "H", question: "多选", options: [{ label: "A" }, { label: "B" }] }],
    });
    expect(seen.length).toBe(2);
    expect(result.content[0].text).toBe("模式: 选哪个 → Beta\nH: 多选 → A, B；另注");
    expect(result.details).toEqual({
      answers: { "模式: 选哪个": "Beta", "H: 多选": "A, B；另注" },
    });
  });

  test("取消时短路并标记 interrupted", async () => {
    let calls = 0;
    const tool = createAskUserQuestionTool(() => {
      calls++;
      return Promise.resolve(calls === 1 ? { labels: ["Alpha"] } : null);
    });
    const result = await runExecute(tool, {
      questions: [q1, { header: "H", question: "Q2", options: [{ label: "X" }] }],
    });
    expect(calls).toBe(2);
    expect(result.details.interrupted).toBe(true);
    expect(result.content[0].text).toContain("用户取消");
    expect(result.content[0].text).toContain("Alpha");
  });

  test("全部问题无效时返回错误提示", async () => {
    const tool = createAskUserQuestionTool(() => Promise.resolve(null));
    const result = await runExecute(tool, { questions: [{ question: "无选项" }] });
    expect(result.details.error).toBe("no_valid_question");
  });
});
