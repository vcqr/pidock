import { describe, expect, test } from "bun:test";
import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";
import { defaultSessionDir, defaultWorkspaceRoot, isManagedSessionDir, sessionDirUnder } from "./workspace.js";

describe("defaultWorkspaceRoot", () => {
  test("根目录固定为 $HOME/.pidock", () => {
    expect(defaultWorkspaceRoot()).toBe(join(homedir(), ".pidock"));
  });
});

describe("sessionDirUnder", () => {
  test("路径为 root/<当日日期>/<会话id>（本地时区）", () => {
    const root = mkdtempSync(join(tmpdir(), "pidock-ws-"));
    try {
      const dir = sessionDirUnder(root, "sess-1", new Date(2026, 0, 2, 3, 4, 5));
      expect(dir).toBe(join(root, "2026-01-02", "sess-1"));
      expect(existsSync(dir)).toBe(false);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
});

describe("defaultSessionDir", () => {
  test("只计算路径不落盘（目录由首个获准执行的工具调用惰性创建）", () => {
    const root = mkdtempSync(join(tmpdir(), "pidock-ws-"));
    try {
      const dir = defaultSessionDir("sess-1", new Date(2026, 9, 13), root);
      expect(dir).toBe(join(root, "2026-10-13", "sess-1"));
      expect(existsSync(dir)).toBe(false);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  test("同会话 id 幂等（路径稳定）", () => {
    const now = new Date(2026, 10, 13);
    expect(defaultSessionDir("sess-1", now)).toBe(defaultSessionDir("sess-1", now));
  });
});

describe("isManagedSessionDir", () => {
  test("默认工作空间内部算托管，根目录本身与外部目录不算", () => {
    const root = defaultWorkspaceRoot();
    expect(isManagedSessionDir(join(root, "2026-10-13", "abc"))).toBe(true);
    expect(isManagedSessionDir(root)).toBe(false);
    expect(isManagedSessionDir(join(homedir(), "projects", "demo"))).toBe(false);
  });
});
