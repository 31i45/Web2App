//! build.rs — 母版图标编译（Windows）。
//!
//! 把 `assets/w2a.ico` 经 .rc 资源脚本编译进 exe，由链接器布局 `.rsrc` 节
//! —— 文件图标（Explorer/任务栏）由此而来，产物字节级继承，无需任何注入。
//! 非 Windows 目标为空操作（窗口图标由运行时 tao API 设置，见 webview.rs）。

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winres::WindowsResource::new();
    res.set_icon("assets/w2a.ico");
    if let Err(e) = res.compile() {
        println!("cargo:warning=winres failed: {e}");
    }
    println!("cargo:rerun-if-changed=assets/w2a.ico");
}
