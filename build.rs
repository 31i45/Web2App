//! build.rs — 母版图标编译（Windows 原生构建）。
//!
//! 把 `assets/w2a.ico` 经 `.rc` 资源脚本编译进 exe，由链接器布局 `.rsrc` 节
//! —— 文件图标（Explorer/任务栏）由此而来，产物字节级继承，无需任何注入。
//!
//! 跨平台正确性（CI 三平台矩阵曾在此翻车）：winres 是 `cfg(windows)` 限定的
//! build-dependency——非 Windows 构建机上该 crate 不在依赖图内，引用它的代码
//! 必须同样被 `cfg(target_os = "windows")` 在编译期消除（按构建机平台裁剪），
//! 否则 Unix 上 build.rs 直接编译失败（undeclared crate `winres`）。
//! 运行时再以 `CARGO_CFG_TARGET_OS` 区分目标平台，使「Windows 主机交叉编译
//! Unix 目标」也正确空操作。

fn main() {
    // 目标平台非 Windows：不嵌入任何资源（Unix 产物无 .rsrc 概念）
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // 构建机为 Windows 时才编译 winres 代码块（交叉编译 Unix 主机时跳过图标）
        #[cfg(target_os = "windows")]
        {
            let mut res = winres::WindowsResource::new();
            res.set_icon("assets/w2a.ico");
            if let Err(e) = res.compile() {
                println!("cargo:warning=winres failed: {e}");
            }
        }
    }
    println!("cargo:rerun-if-changed=assets/w2a.ico");
}
