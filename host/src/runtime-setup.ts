// 编译版 host（bun build --compile）没有 pi CLI 那样的 Bun 运行时入口：pi 的
// dist/bun/cli.js 先加载 runtime-setup，静态 import quickjs-wasi/quickjs.wasm
// 让 Bun 把 wasm 内嵌进可执行文件，再调 setEmbeddedQuickJSWasmPath 注册路径。
// 缺了这步，codemode 执行时 getQuickJSWasmPath() 走 createRequire 回退，编译
// 产物里没有磁盘 node_modules，必然报 Cannot find module
// 'quickjs-wasi/quickjs.wasm'（dev 模式 bun src/main.ts 有磁盘依赖所以不暴露）。
// SDK 的 exports map 挡住 deep import，经 tsconfig paths 别名引入（同一模块
// 实例，bun 按 realpath 去重）；node --experimental-strip-types 直跑时跳过，
// 走磁盘回退可正常解析。
declare const Bun: unknown;

export {};

if (typeof Bun !== "undefined") {
  const { default: quickjsWasmPath } = await import("quickjs-wasi/quickjs.wasm");
  const { setEmbeddedQuickJSWasmPath } = await import(
    "@earendil-works/pi-coding-agent/dist/config.js"
  );
  setEmbeddedQuickJSWasmPath(quickjsWasmPath);
}
