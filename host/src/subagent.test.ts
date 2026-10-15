import { describe, expect, test } from "bun:test";
import { sanitizeTaskParams } from "./subagent.js";

describe("sanitizeTaskParams", () => {
  test("保留合法字段并原样传递", () => {
    const p = sanitizeTaskParams({
      prompt: "审查 pool.ts 的会话生命周期",
      label: "Review pool",
      expert_id: "e123",
      model: "anthropic/claude-opus",
    });
    expect(p.prompt).toBe("审查 pool.ts 的会话生命周期");
    expect(p.label).toBe("Review pool");
    expect(p.expert_id).toBe("e123");
    expect(p.model).toBe("anthropic/claude-opus");
  });

  test("超长 prompt/label 截断（LLM 输出不可信）", () => {
    const p = sanitizeTaskParams({ prompt: "a".repeat(30_000), label: "l".repeat(200) });
    expect(p.prompt.length).toBe(20_000);
    expect(p.label!.length).toBe(60);
  });

  test("非字符串/空白字段被丢弃", () => {
    const p = sanitizeTaskParams({ prompt: "  ", label: 42, expert_id: null, model: true });
    expect(p.prompt).toBe("");
    expect(p.label).toBeUndefined();
    expect(p.expert_id).toBeUndefined();
    expect(p.model).toBeUndefined();
  });

  test("空对象返回空 prompt", () => {
    expect(sanitizeTaskParams({}).prompt).toBe("");
  });

  test("maxTurns 防呆：合法取整、越界钳制、非法丢弃", () => {
    expect(sanitizeTaskParams({ prompt: "x", maxTurns: 50 }).maxTurns).toBe(50);
    expect(sanitizeTaskParams({ prompt: "x", maxTurns: 9999 }).maxTurns).toBe(500);
    expect(sanitizeTaskParams({ prompt: "x", maxTurns: 3.7 }).maxTurns).toBe(3);
    expect(sanitizeTaskParams({ prompt: "x", maxTurns: 0 }).maxTurns).toBeUndefined();
    expect(sanitizeTaskParams({ prompt: "x", maxTurns: -5 }).maxTurns).toBeUndefined();
    expect(sanitizeTaskParams({ prompt: "x", maxTurns: "abc" }).maxTurns).toBeUndefined();
    expect(sanitizeTaskParams({ prompt: "x" }).maxTurns).toBeUndefined();
  });
});
