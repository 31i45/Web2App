//! child/build.rs — 子应用文件图标编译（Windows 原生构建）。
//!
//! 把 `assets/w2a.ico` 经 `.rc` 资源脚本编译进 exe，由链接器布局 `.rsrc` 节
//! —— 文件图标（Explorer/任务栏）由此而来。child 产物字节级继承此图标，
//! master 用 UpdateResource 在打包时注入用户自定义图标覆盖此位置。
//!
//! 目标平台门控：winres 仅对 Windows 目标生效。build script 在 **host** 上运行，
//! 故判据必须取 `CARGO_CFG_TARGET_OS`（而非 `cfg!(windows)`，后者在
//! Windows host 交叉编译 Unix 目标时误判为真）。Linux/macOS 目标的文件图标
//! 由 master 的对应打包器另行处理。

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    // assets 在 workspace 根目录，child 子 crate 经 CARGO_MANIFEST_DIR 回溯上一级。
    let icon = format!("{}/../assets/w2a.ico", env!("CARGO_MANIFEST_DIR"));
    let mut res = winres::WindowsResource::new();
    res.set_icon(&icon);
    // 产物身份：winres 默认用包名（"child"）填充 FileDescription/ProductName，
    // 会经产物 exe 的版本资源泄漏到 Task Manager 等用户可见处。显式统一为
    // 产品名 "Web2App"，切断「内部架构角色名 → 用户可见身份」的泄漏。
    res.set("FileDescription", "Web2App");
    res.set("ProductName", "Web2App");
    if let Err(e) = res.compile() {
        println!("cargo:warning=winres failed: {e}");
    }
    println!("cargo:rerun-if-changed={icon}");
}
