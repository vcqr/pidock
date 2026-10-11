// codemode 沙箱 worker 入口（仅编译版使用）。SDK 的 getCodemodeWorkerSpecifier() 在
// bun-binary 运行时固定返回 "./src/extensions/codemode/worker.ts"，这是 pi 官方
// release 构建的约定：把 worker 作为 bun build --compile 的额外入口点内嵌进二进制。
// host 的构建命令必须满足两个条件（见 package.json build 与 release.yml）：
// 1. 把本文件与 src/main.ts 一起传入，否则 codemode 执行报 Cannot find module
//    './src/extensions/codemode/worker.ts'（dev 模式走 pi-codemode 的磁盘
//    defaultWorkerUrl，不受影响）。worker.js 顶层在 parentPort 存在时自启动。
// 2. 必须加 `--root .`（以 host/ 为 cwd）：bun 按「所有入口点的公共祖先目录」
//    生成内嵌清单键，main.ts 与本文件同在 src/ 下时键会丢掉 src/ 前缀
//    （extensions/codemode/worker.js），与 SDK 期望的 specifier 匹配不上，worker
//    退化为磁盘加载并在运行时报 Cannot find package 'quickjs-wasi'。--root .
//    强制键为 src/extensions/codemode/worker.js，恰好匹配。
import "@earendil-works/pi-codemode/worker";
