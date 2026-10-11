// quickjs-wasi 的 ./quickjs.wasm 子路径是纯资源（无类型声明），Bun 会把静态/
// 动态 import 的 wasm 内嵌进编译产物并 evaluate 成可读路径
declare module "*.wasm" {
  const path: string;
  export default path;
}

// pi SDK 的 exports map 只放出 .、./rpc-entry 等子路径，dist/config.js 只能经
// tsconfig paths 别名引入；这里补最小类型面
declare module "@earendil-works/pi-coding-agent/dist/config.js" {
  export const isBunBinary: boolean;
  export const isBunRuntime: boolean;
  export function setEmbeddedQuickJSWasmPath(path: string): void;
  export function getQuickJSWasmPath(): string;
  /** bun-binary 运行时固定返回 "./src/extensions/codemode/worker.ts"（编译版 worker 约定） */
  export function getCodemodeWorkerSpecifier(): string | undefined;
}

// pi SDK 的 exports map 不放出 dist/core/output-guard.js，同经 tsconfig paths 别名引入
declare module "@earendil-works/pi-coding-agent/dist/core/output-guard.js" {
  /** 接管 stdout：此后 process.stdout.write 改道 stderr，包管理器等子进程不再污染协议通道 */
  export function takeOverStdout(): void;
  /** 写真实 stdout（接管后协议帧唯一合法出口；未接管时等价直写） */
  export function writeRawStdout(text: string): void;
  export function isStdoutTakenOver(): boolean;
}
