//! child/src/main.rs — 子应用运行时入口。
//!
//! 分发逻辑（精简，无母版模式）：
//! 1. 尾部有配置 → 打开目标网页（普通 Web2App 应用）。
//! 2. 尾部无配置 → 运行时组件不能直接运行（child 是产物模板，无打包能力，
//!    结构性不可繁殖——打包器代码在 master crate，child 不含）。
//!
//! Release 构建为 Windows GUI 子系统：双击运行不出现后台控制台。

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod icon;
mod platform;
mod popup;
mod webview;

fn main() {
  match tail_core::read_tail() {
    Ok(Some(c)) => {
      if let Err(e) = webview::open_webview(c) {
        platform::fatal(&e);
      }
    }
    Ok(None) => platform::fatal(
      "本程序是 Web2App 运行时组件，不能直接运行。\n请使用 Web2App 打包器生成应用。",
    ),
    Err(e) => platform::fatal(&format!(
      "读取运行配置失败：{e}\n（文件可能已损坏，请重新获取本程序）"
    )),
  }
}
