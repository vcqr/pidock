//! 确保 rust-embed 的嵌入目录在编译期存在：从未构建过 web 前端时放一个占位
//! index.html，否则 `cargo build/check -p pidock-server` 会因 apps/web/dist
//! 缺失直接编译失败。dist 本身被 gitignore，生成它不污染工作区。
use std::path::Path;

fn main() {
    let dist = Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../web/dist");
    println!("cargo:rerun-if-changed={}", dist.display());
    println!(
        "cargo:rerun-if-changed={}",
        dist.join("index.html").display()
    );
    if !dist.join("index.html").exists() {
        std::fs::create_dir_all(&dist).expect("create apps/web/dist");
        std::fs::write(
            dist.join("index.html"),
            "<!doctype html><meta charset=\"utf-8\"><title>PiDock</title>\
             <p>web console not built — run <code>pnpm --filter @pidock/web build</code></p>",
        )
        .expect("write placeholder index.html");
    }
}
