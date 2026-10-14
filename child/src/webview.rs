//! child/src/webview.rs — 统一渲染层（系统原生 WebView 适配层）。
//!
//! v2 Windows-only：仅 WebView2 后端。上层（main.rs）只面向 `open_webview`
//! 一个入口。master 有自己的窗口构建逻辑（不依赖本模块）。

use tail_core::AppConfig;
use std::path::PathBuf;
use tao::{
  event::{Event, WindowEvent},
  event_loop::ControlFlow,
  window::{Icon, Theme, Window, WindowBuilder},
};
use wry::{WebContext, WebView};

/// 打开一个 WebView 窗口并阻塞至窗口关闭。
///
/// `config` 提供目标 URL、标题与可选用户图标（设为窗口图标）。
/// 用户选图 → 设为窗口图标；未选图 → 回落内嵌 w2a 默认图标
/// （Windows 窗口类图标不自动读 exe 资源节，须显式设置）。
/// 构建失败（窗口或 WebView）返回 Err——如 Windows 缺 WebView2 运行时、
/// 窗口创建失败等——由调用方决定如何呈现给用户（机制不预设死亡策略）。
pub fn open_webview(config: AppConfig) -> Result<(), String> {
  let event_loop: tao::event_loop::EventLoop<()> = tao::event_loop::EventLoop::new();
  let icon = config
    .icon_png
    .as_deref()
    .and_then(crate::icon::window_icon)
    .or_else(crate::icon::default_window_icon);
  let window = build_window(&event_loop, &config.title, Theme::Dark, icon)?;
  // 关闭哨兵必须在 WebView 构建（wry 挂接 subclass）之前安装，见 tail_core 文档。
  tail_core::arm_close_watchdog(tao::platform::windows::WindowExtWindows::hwnd(&window));
  // WebContext 需存活至事件循环结束（数据目录指向系统应用数据区，不污染 exe 目录）
  let mut context = WebContext::new(Some(webview_data_dir(&config.url)));
  let _webview = build_webview(&window, &config, &mut context)
    .map_err(|e| format!("创建 WebView 失败：{e}\n（Windows 请安装 WebView2 运行时：https://developer.microsoft.com/microsoft-edge/webview2/）"))?;

  event_loop.run(move |event, _, control_flow| {
    *control_flow = ControlFlow::Wait;
    if let Event::WindowEvent {
      event: WindowEvent::CloseRequested,
      ..
    } = event
    {
      // 生命周期与 WebView 绑定：窗口即应用，关闭即退出，无驻留。
      // Exit 之外加看门狗兜底：单条 WM_CLOSE 路径（Alt+F4/任务栏关闭/
      // taskkill/自动化）下 WebView2 消息泵可能吞掉退出信号，见 tail_core 文档。
      *control_flow = ControlFlow::Exit;
      tail_core::quit_watchdog();
    }
  })
  // tao 的 run() 返回 `!`（永不返回），`!` 直接收敛为本函数返回类型；
  // 正常路径到此即退出进程，Err 仅发生在上方构建阶段。
}

/// 构建原生窗口（深色标题栏）。
/// `icon`：窗口图标（None = 不设，Windows 下显示系统默认）。
/// 构建失败返回 Err——与 `build_webview` 一致，由调用方决定呈现方式；
/// 不在此 panic（release 为 GUI 子系统 + panic=abort，panic 即静默死亡）。
pub fn build_window(
  event_loop: &tao::event_loop::EventLoop<()>,
  title: &str,
  theme: Theme,
  icon: Option<Icon>,
) -> Result<Window, String> {
  let mut builder = WindowBuilder::new()
    .with_title(title)
    .with_theme(Some(theme))
    .with_inner_size(tao::dpi::LogicalSize::new(1100u32, 750u32))
    .with_min_inner_size(tao::dpi::LogicalSize::new(420u32, 320u32));
  if let Some(ic) = icon {
    builder = builder.with_window_icon(Some(ic));
  }
  builder.build(event_loop).map_err(|e| format!("创建窗口失败：{e}"))
}

/// 构建 WebView（产物形态：加载目标 URL）。devtools 仅 debug 构建启用（release 零开销）。
/// 失败原因典型为系统 WebView 运行时缺失（Windows WebView2），返回 Err 交由调用方呈现。
pub fn build_webview(window: &Window, config: &AppConfig, context: &mut WebContext) -> Result<WebView, wry::Error> {
  let mut builder = wry::WebViewBuilder::new_with_web_context(context).with_url(&config.url);

  if cfg!(debug_assertions) {
    builder = builder.with_devtools(true);
  }

  builder.build(window)
}

/// WebView 用户数据目录：系统应用数据区（按 host 分区），exe 旁不落盘。
pub fn webview_data_dir(url: &str) -> PathBuf {
  data_base_dir()
    .join("Web2App")
    .join("apps")
    .join(tail_core::sanitize_host(url))
}

/// 数据基目录探测（Windows-only）：%LOCALAPPDATA% → 兜底 temp。
///
/// v2 已转向 Windows-only，删除 Unix XDG/HOME 分支。Windows 上 LOCALAPPDATA
/// 几乎总存在；缺失（极端环境）回落 temp 目录保证不 panic。
fn data_base_dir() -> PathBuf {
  if let Some(p) = std::env::var_os("LOCALAPPDATA") {
    return PathBuf::from(p);
  }
  std::env::temp_dir()
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn data_dir_uses_sanitized_host() {
    let dir = webview_data_dir("https://example.com/x?y=1");
    let s = dir.to_string_lossy().replace('\\', "/");
    assert!(s.ends_with("apps/example.com"), "got: {s}");
    // 非 ASCII host 被替换为安全字符
    let dir2 = webview_data_dir("https://中文站.com");
    let s2 = dir2.to_string_lossy().replace('\\', "/");
    assert!(!s2.contains('中'), "got: {s2}");
  }

  #[test]
  fn data_base_dir_is_absolute() {
    // 探测链不 panic 且返回绝对路径（LOCALAPPDATA 或 temp 都是绝对路径）
    let d = data_base_dir();
    assert!(d.is_absolute());
  }
}
