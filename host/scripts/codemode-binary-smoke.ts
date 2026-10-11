// 编译版（bun build --compile）codemode 沙箱最小复现：
// 按 execute.js 的方式组装 CodemodeSandbox（内嵌 wasm 路径 + getCodemodeWorkerSpecifier），
// 执行一段调用工具的脚本。worker.ts 用法对齐 runtime-setup.ts。
declare const Bun: unknown;

async function setupEmbeddedWasm() {
  if (typeof Bun !== "undefined") {
    const { default: quickjsWasmPath } = await import("quickjs-wasi/quickjs.wasm");
    const { setEmbeddedQuickJSWasmPath } = await import(
      "@earendil-works/pi-coding-agent/dist/config.js"
    );
    setEmbeddedQuickJSWasmPath(quickjsWasmPath);
  }
}

async function main() {
  await setupEmbeddedWasm();
  const config = await import("@earendil-works/pi-coding-agent/dist/config.js");
  console.log("[diag] isBunBinary:", config.isBunBinary, "isBunRuntime:", config.isBunRuntime);
  console.log("[diag] wasm path:", config.getQuickJSWasmPath());
  console.log("[diag] worker specifier:", config.getCodemodeWorkerSpecifier());

  // 按 execute.js 的方式组装：内嵌 wasm 编译结果 + 编译版 worker specifier
  const { CodemodeSandbox, loadQuickJSWasm } = await import("@earendil-works/pi-codemode");
  const sandbox = new CodemodeSandbox({
    tools: [
      {
        name: "echo",
        description: "echo back",
        execute: async (args: unknown) => JSON.stringify(args),
      },
    ],
    wasm: loadQuickJSWasm(config.getQuickJSWasmPath()),
    workerUrl: config.getCodemodeWorkerSpecifier(),
  });
  const result = await sandbox.execute(`const v = await tools.echo({ hello: "world" });\nreturn v;`);
  console.log("[result]", JSON.stringify(result, null, 2));
  await sandbox.close();
  if (!result.ok) process.exit(1);
}

main().catch((error) => {
  console.error("[fatal]", error);
  process.exit(1);
});
