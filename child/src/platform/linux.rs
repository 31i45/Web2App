//! child/src/platform/linux.rs — Linux 平台外壳（child 侧）。
//!
//! 阶段 0b：按设计文档 §3.3 补齐与 Windows 侧**同名同签名**的项，使 Linux 目标
//! 可编译（门禁 G-0b）。具体行为在阶段 1 落地（T-1-1 GTK 外壳 / T-1-2 按键拦截 /
//! T-1-3 数据目录），本文件当前只提供**不使既有能力退化为「点了没反应」**的最小
//! 实现；每处未落地者以「阶段 1」标注，供 §7 T-0b-2 接缝笔记汇总（ADR-3 不预设计）。
//!
//! 与 Windows 侧各 `fn` 的签名逐字一致（缺一或签名不符即编译失败，这是刻意设计）。

use std::path::PathBuf;

use tao::event_loop::EventLoopProxy;
use wry::{NewWindowFeatures, NewWindowResponse, WebView};

use crate::webview::ChildEvent;

// ============================================================================
// 数据目录（XDG Base Directory）
// ============================================================================

/// `<数据基目录>/Web2App/<sub>`：Linux 遵循 XDG Base Directory 规范。
///
/// 探测链（与 Windows 的 `%LOCALAPPDATA%` 同构）：`$XDG_DATA_HOME`（须为绝对路径，
/// 相对路径按规范应被忽略）→ `$HOME/.local/share` → 兜底 temp（极端环境不 panic）。
/// 品牌目录名 `Web2App` 与 Windows 侧一致。隔离有效性（V-6）在阶段 1 T-1-3
/// 由 `WebContext.data_directory` 实测确认——此处仅保证「不落 exe 目录」。
pub(crate) fn data_dir(sub: &str) -> PathBuf {
  let base = std::env::var_os("XDG_DATA_HOME")
    .map(PathBuf::from)
    .filter(|p| p.is_absolute())
    .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
    .unwrap_or_else(std::env::temp_dir);
  base.join("Web2App").join(sub)
}

// ============================================================================
// 致命错误呈现（stderr + 非零退出码）
// ============================================================================

/// 致命错误的呈现后退出（返回 `!`）。
///
/// Unix 侧不存在 Windows「GUI 子系统下 stderr 无人看」的问题——从终端或桌面启动器
/// 启动时 stderr 可见，故按 UNIX 惯例写 stderr 并 `exit(1)`。阶段 1 若需 GUI 弹窗
/// （GTK dialog）再行接线。
pub(crate) fn fatal(msg: &str) -> ! {
  eprintln!("Web2App: {msg}");
  std::process::exit(1)
}

// ============================================================================
// 关闭确定性
// ============================================================================

/// no-op：看门狗是为绕开 Windows「WebView2 嵌套 COM 消息泵吞掉 WM_QUIT」而存在
/// （见 Windows 侧同名函数文档）。GTK/wry 无此结构性吞没，`ControlFlow::Exit`
/// 即确定退出，故 0b 不武装看门狗。阶段 1 T-1-1 实测若出现残留再按需补。
pub(crate) fn quit_watchdog() {}

/// no-op：关闭哨兵（WNDPROC 替换 + 看门狗）是纯 Win32 机制，GTK 侧无对应物。
/// 参数仅用于与 Windows 侧同签名（调用点在 `webview.rs`，平台无关）。
pub(crate) fn install_close_watchdog(_window: &tao::window::Window) {}

// ============================================================================
// 宿主层按键（阶段 1：T-1-2 / V-3 三候选实测后落定）
// ============================================================================

/// no-op（阶段 1 接线）：Windows 用 WebView2 `AcceleratorKeyPressed`（在按键送达
/// 内容层之前回调，覆盖跨源 iframe 内收不到键的边界）；GTK 侧需在 GTK 加速键
/// （`gtk::AccelGroup`，据 V-3 可能看不到 F5/Ctrl+R）与组件级 `key-press-event` 之间
/// 实测选定，属 T-1-2。
///
/// 0b 不接线的净影响为零：页面层 `keydown`（见 `webview::home_hotkey_script`）仍
/// 覆盖顶层帧内的 Alt+Home，回首页能力不缺；仅「焦点在跨源 iframe 内」这一边界
/// 暂未覆盖（与阶段 1 前的现状一致，非退化）。
pub(crate) fn install_keys(_webview: &WebView, _proxy: EventLoopProxy<ChildEvent>) {}

// ============================================================================
// External：交给系统默认程序
// ============================================================================

/// 用系统默认程序打开 URL（XDG `xdg-open`，与 Windows `ShellExecuteW` 同语义）。
///
/// 异步派发（`spawn` 而非 `status`），不阻塞 UI 线程——与 `ShellExecuteW` 的非阻塞
/// 行为一致。真协议（`http(s)` / `mailto:` / 自定义协议…）交给系统协议处理器是
/// 浏览器的标准行为（Phase 1 可考虑按桌面环境细化为 `gio open`）。
pub(crate) fn shell_open(url: &str) {
  let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

/// 建窗失败时的降级：仅对 http(s) 交系统打开（空白窗外发只会弹系统浏览器空白页）。
pub(crate) fn open_external_if_http(url: &str) {
  if crate::popup::is_http_url(url) {
    shell_open(url);
  }
}

// ============================================================================
// Popup：独立原生窗口 + WebView（阶段 1：T-1-1）
// ============================================================================

/// 阶段 1 接线：GTK 侧需先建 GTK 顶层窗再建 WebView，并以
/// `NewWindowResponse::Create { webview }` 交给 webkit2gtk（其载荷类型为
/// `webkit2gtk::WebView`，与 Windows 的 `ICoreWebView2` 异质 → 故设计文档取
/// 同签名 `#[cfg]` 模块而非 trait，见 ADR-1）。
///
/// 0b 暂返回 `None`：调用方（`webview.rs`）据此降级为 [`open_external_if_http`]，
/// 保证跨站外链仍可打开，不退化到「点了没反应」。代价：空白窗（`about:blank`）与
/// 带尺寸特征的 OAuth 弹窗在 Linux 0b 上会被外发或静默放弃——这是**阶段 1 的
/// 待办**，不是最终行为。
///
/// [`open_external_if_http`]: open_external_if_http
pub(crate) fn create_popup(
  _url: &str,
  _features: NewWindowFeatures,
  _home: &str,
) -> Option<NewWindowResponse> {
  None
}

/// no-op（阶段 1 接线）：仅 `create_popup` 建成 GTK 弹窗后才有调用方
/// （Windows 侧由弹窗自身的「站内就地导航」路由回调调用）。0b 无弹窗 → 暂无人调用。
#[allow(dead_code)]
pub(crate) fn navigate_popup(_handle: isize, _url: &str) {}

/// no-op：0b 不建弹窗，无存活弹窗可关。调用点在 `webview.rs` 的关闭路径
/// （与 Windows 侧同一位置，平台无关）；阶段 1 建成弹窗后在此回收全部 GTK 窗口。
pub(crate) fn close_all_popups() {}

/// no-op（阶段 1 接线）：仅 `create_popup` 建成弹窗后才有调用方（弹窗 IPC 全屏上报）。
/// 主窗口全屏由 `webview.rs` 直接调 tao `Window::set_fullscreen`，不经过本函数。
/// 阶段 1 另需处理 WebKitGTK 的 `enter-fullscreen` 信号（V-4）。
#[allow(dead_code)]
pub(crate) fn set_fullscreen(_handle: isize, _on: bool) {}

#[cfg(test)]
mod tests {
  use super::*;

  /// 与 Windows 侧同名测试同构：品牌目录拼接 + 绝对路径（探测链不 panic）。
  #[test]
  fn data_dir_lives_under_brand_and_is_absolute() {
    let s = data_dir("apps").to_string_lossy().replace('\\', "/");
    assert!(s.ends_with("/Web2App/apps"), "got: {s}");
    assert!(data_dir("apps").is_absolute());
  }
}
