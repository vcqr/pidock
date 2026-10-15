import { describe, expect, test } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

// 隔离 agent 目录（无 auth/models → 子代理 prompt 快速失败，不花真实 token）
// 必须先于 pool.ts 导入设置（trustStore 等在构造期读取 getAgentDir()）
const agentDir = mkdtempSync(join(tmpdir(), "pidock-pool-subagent-"));
process.env.PI_CODING_AGENT_DIR = agentDir;

const { SessionPool } = await import("./pool.js");

describe("SessionPool subagent plumbing", () => {
  test(
    "runTask：派生登记/报告回退/深度守卫/并发上限/收尾清理",
    async () => {
      const pool = new SessionPool();
      const cwd = mkdtempSync(join(tmpdir(), "pidock-pool-subagent-cwd-"));
      try {
        const parent = await pool.createSession({ cwd });
        const run = pool as unknown as {
          runTask: (p: string, t: { prompt: string; label?: string }) => Promise<{ session_id: string; report: string }>;
        };
        const maps = pool as unknown as {
          subagentParent: Map<string, string>;
          subagentChildren: Map<string, Set<string>>;
        };

        // 派生：无模型环境 prompt 快速失败 → 回退报告；登记与注册表正确
        const first = await run.runTask(parent.session_id, { prompt: "任意任务", label: "测试子代理" });
        expect(first.session_id).toBeTruthy();
        expect(first.report).toContain("没有产出文本报告");
        const child = pool.listSessions().sessions.find((s) => s.session_id === first.session_id);
        expect(child?.kind).toBe("subagent");
        expect(child?.parent_session_id).toBe(parent.session_id);
        expect(child?.name).toBe("测试子代理");
        expect(child?.source).toBe("subagent");
        // 收尾登记已清理（子代理回合结束即关闭）
        expect(maps.subagentParent.size).toBe(0);
        expect(maps.subagentChildren.size).toBe(0);
        expect(pool.listSessions().sessions.find((s) => s.session_id === first.session_id)?.open).toBe(false);

        // 深度 1：运行中的子代理（登记在册）不能再派生——用打开中的会话播种验证
        maps.subagentParent.set(parent.session_id, "fake-grandparent");
        await expect(run.runTask(parent.session_id, { prompt: "x" })).rejects.toThrow("深度");
        maps.subagentParent.delete(parent.session_id);

        // 并发上限：每父 MAX=5，第 6 个拒绝
        maps.subagentChildren.set(parent.session_id, new Set(["c1", "c2", "c3", "c4", "c5"]));
        await expect(run.runTask(parent.session_id, { prompt: "x" })).rejects.toThrow("上限");
        maps.subagentChildren.delete(parent.session_id);
      } finally {
        pool.disposeAll();
        rmSync(cwd, { recursive: true, force: true });
      }
    },
    120_000,
  );
});
