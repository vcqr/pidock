// quickjs-wasi 的 ./quickjs.wasm 子路径是纯资源（无类型声明），Bun 会把静态/
// 动态 import 的 wasm 内嵌进编译产物并 evaluate 成可读路径
declare module "*.wasm" {
  const path: string;
  export default path;
}

// pi SDK 的 exports map 只放出 .、./rpc-entry 等子路径，dist/config.js 只能经
// tsconfig paths 别名引入；这里补最小类型面
declare module "@earendil-works/pi-coding-agent/dist/config.js" {
  export function setEmbeddedQuickJSWasmPath(path: string): void;
  export function getQuickJSWasmPath(): string;
}
