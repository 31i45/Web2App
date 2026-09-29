//! webview.rs — 统一渲染层（系统原生 WebView 适配层）。
//!
//! 所有平台差异收敛于此：Windows/WebView2、macOS/WKWebView、Linux/WebKitGTK。
//! 上层（main.rs / ui.rs）只面向 `open_webview` 一个入口。

use crate::tail::AppConfig;
use std::path::PathBuf;
use tao::{
  event::{Event, WindowEvent},
  event_loop::ControlFlow,
  window::{Icon, Theme, Window, WindowBuilder},
};
use wry::{WebContext, WebView};

/// 打开一个 WebView 窗口并阻塞至窗口关闭。
///
/// `config` 提供目标 URL、标题与可选用户图标（设为窗口图标，跨平台）。
/// 未选图 → 不设置 → 窗口自动回落 exe 资源图标（母版 w2a）。
/// 本函数是全项目唯一的「运行窗口」入口（深模块）。
pub fn open_webview(config: AppConfig) {
  let event_loop: tao::event_loop::EventLoop<()> = tao::event_loop::EventLoop::new();
  let icon = config.icon_png.as_deref().and_then(crate::icon::window_icon);
  let window = build_window(&event_loop, &config.title, Theme::Dark, icon);
  // WebContext 需存活至事件循环结束（数据目录指向系统应用数据区，不污染 exe 目录）
  let mut context = WebContext::new(Some(webview_data_dir(&config.url)));
  let _webview = build_webview(&window, &config, &mut context);

  event_loop.run(move |event, _, control_flow| {
    *control_flow = ControlFlow::Wait;
    if let Event::WindowEvent {
      event: WindowEvent::CloseRequested,
      ..
    } = event
    {
      // 生命周期与 WebView 绑定：窗口即应用，关闭即退出，无驻留。
      *control_flow = ControlFlow::Exit;
    }
  });
}

/// 构建原生窗口（深色标题栏，与 UI 主题一致）。
/// 泛型 T：事件循环自定义事件类型（母版用 FormEvent，产物用 ()）。
/// `icon`：用户图标（None = 回落 exe 资源图标）。
pub fn build_window<T>(
  event_loop: &tao::event_loop::EventLoop<T>,
  title: &str,
  theme: Theme,
  icon: Option<Icon>,
) -> Window {
  let mut builder = WindowBuilder::new()
    .with_title(title)
    .with_theme(Some(theme))
    .with_inner_size(tao::dpi::LogicalSize::new(1100u32, 750u32))
    .with_min_inner_size(tao::dpi::LogicalSize::new(420u32, 320u32));
  if let Some(ic) = icon {
    builder = builder.with_window_icon(Some(ic));
  }
  builder.build(event_loop).expect("create window")
}

/// 构建 WebView（产物形态：加载目标 URL）。devtools 仅 debug 构建启用（release 零开销）。
pub fn build_webview(window: &Window, config: &AppConfig, context: &mut WebContext) -> WebView {
  let mut builder = wry::WebViewBuilder::new_with_web_context(context).with_url(&config.url);

  if cfg!(debug_assertions) {
    builder = builder.with_devtools(true);
  }

  builder
    .build(window)
    .unwrap_or_else(|e| panic!("create webview: {e}"))
}

/// WebView 用户数据目录：系统应用数据区（按 host 分区），exe 旁不落盘。
pub fn webview_data_dir(url: &str) -> PathBuf {
  data_base_dir()
    .join("Web2App")
    .join("apps")
    .join(crate::builder::sanitize_host(url))
}

/// 数据基目录的跨平台探测链（无属性级 cfg 分裂，一屏读全）：
/// Windows `%LOCALAPPDATA%` → Unix `$XDG_DATA_HOME` →
/// macOS `~/Library/Application Support` / Linux `~/.local/share` → 兜底 temp。
fn data_base_dir() -> PathBuf {
  if let Some(p) = std::env::var_os("LOCALAPPDATA") {
    return PathBuf::from(p);
  }
  if let Some(p) = std::env::var_os("XDG_DATA_HOME") {
    return PathBuf::from(p);
  }
  if let Some(home) = std::env::var_os("HOME") {
    let rel = if cfg!(target_os = "macos") { "Library/Application Support" } else { ".local/share" };
    return PathBuf::from(home).join(rel);
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
    // 各平台环境变量至少其一存在；验证探测链不 panic 且返回绝对路径
    let d = data_base_dir();
    assert!(d.is_absolute());
  }
}
