import { describe, expect, test } from "bun:test";
import { buildExpertPromptBlock, matchExpertCommand } from "./experts.js";
import type { Expert } from "@pidock/protocol";

function fakeExpert(over: Partial<Expert> = {}): Expert {
  return {
    id: "e1",
    name: "code-reviewer",
    prompt: "你是资深代码评审专家。",
    skills: [],
    extensions: [],
    knowledge_dirs: [],
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    ...over,
  };
}

describe("matchExpertCommand", () => {
  test("解析 /expert:name 与带参形式", () => {
    expect(matchExpertCommand("/expert:code-reviewer")).toEqual({ name: "code-reviewer", args: undefined });
    expect(matchExpertCommand("/expert:code-reviewer 帮我审查这段代码")).toEqual({
      name: "code-reviewer",
      args: "帮我审查这段代码",
    });
    // 名称里允许点/横线，参数可含换行
    expect(matchExpertCommand("/expert:db-migrate\n生成 users 表")).toEqual({
      name: "db-migrate",
      args: "生成 users 表",
    });
  });

  test("普通消息 / 行中提及不匹配", () => {
    expect(matchExpertCommand("帮我看看 /expert: 是什么")).toBeNull();
    expect(matchExpertCommand("普通提问")).toBeNull();
    expect(matchExpertCommand("/skill:not-expert")).toBeNull();
  });
});

describe("buildExpertPromptBlock", () => {
  test("生成 expert 块 + 默认指令（无参数时）", () => {
    const out = buildExpertPromptBlock(fakeExpert(), undefined);
    expect(out).toContain('<expert name="code-reviewer" location="pidock-experts">');
    expect(out).toContain("你是资深代码评审专家。");
    expect(out.endsWith("请以该专家的身份与视角处理本次对话。")).toBe(true);
  });

  test("带参数时保留用户指令", () => {
    const out = buildExpertPromptBlock(fakeExpert(), "重点看内存泄漏");
    expect(out).toContain("重点看内存泄漏");
    expect(out).not.toContain("请以该专家的身份与视角处理本次对话。");
  });

  test("空提示词给出占位文本", () => {
    const out = buildExpertPromptBlock(fakeExpert({ prompt: "" }), undefined);
    expect(out).toContain("（该专家未填写角色提示词）");
  });
});
