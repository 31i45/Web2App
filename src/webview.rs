//! webview.rs — 统一渲染层（系统原生 WebView 适配层）。
//!
//! 所有平台差异收敛于此：Windows/WebView2、macOS/WKWebView、Linux/WebKitGTK。
//! 上层（main.rs / ui.rs）只面向 `open_webview` 一个入口。

use crate::tail::AppConfig;
use std::path::PathBuf;
use tao::{
  event::{Event, WindowEvent},
  event_loop::ControlFlow,
  window::{Theme, Window, WindowBuilder},
};
use wry::{WebContext, WebView};

/// 打开一个 WebView 窗口并阻塞至窗口关闭。
///
/// `config` 提供目标 URL 与窗口标题；`on_close` 在窗口关闭后回调一次。
/// 本函数是全项目唯一的「运行窗口」入口（深模块）。
pub fn open_webview(config: AppConfig, mut on_close: Option<Box<dyn FnOnce()>>) {
  let event_loop: tao::event_loop::EventLoop<()> = tao::event_loop::EventLoop::new();
  let window = build_window(&event_loop, &config.title, Theme::Dark);
  // WebContext 需存活至事件循环结束（数据目录指向系统应用数据区，不污染 exe 目录）
  let mut context = WebContext::new(Some(webview_data_dir(&config.url)));
  let _webview = build_webview(&window, &config, &mut context);

  let mut taken = false;
  event_loop.run(move |event, _, control_flow| {
    *control_flow = ControlFlow::Wait;
    if let Event::WindowEvent {
      event: WindowEvent::CloseRequested,
      ..
    } = event
    {
      // 生命周期与 WebView 绑定：窗口即应用，关闭即退出，无驻留。
      if !taken {
        taken = true;
        if let Some(cb) = on_close.take() {
          cb();
        }
      }
      *control_flow = ControlFlow::Exit;
    }
  });
}

/// 构建原生窗口（深色标题栏，与 UI 主题一致）。
/// 泛型 T：事件循环自定义事件类型（母版用 FormEvent，产物用 ()）。
pub fn build_window<T>(event_loop: &tao::event_loop::EventLoop<T>, title: &str, theme: Theme) -> Window {
  WindowBuilder::new()
    .with_title(title)
    .with_theme(Some(theme))
    .with_inner_size(tao::dpi::LogicalSize::new(1100u32, 750u32))
    .with_min_inner_size(tao::dpi::LogicalSize::new(420u32, 320u32))
    .build(event_loop)
    .expect("create window")
}

/// 构建 WebView。devtools 仅 debug 构建启用（release 自动失效，零开销）。
pub fn build_webview(window: &Window, config: &AppConfig, context: &mut WebContext) -> WebView {
  let mut builder = wry::WebViewBuilder::new_with_web_context(context)
    .with_url(&config.url)
    .with_initialization_script(native_close_bridge());

  if cfg!(debug_assertions) {
    builder = builder.with_devtools(true);
  }

  builder
    .build(window)
    .unwrap_or_else(|e| panic!("create webview: {e}"))
}

/// WebView 用户数据目录：系统应用数据区（`%LOCALAPPDATA%\Web2App\apps\<host>`），
/// 不在 exe 旁边落任何文件。host 缺失时回退临时目录。
/// 跨平台取目录：Windows LOCALAPPDATA / Unix XDG_DATA_HOME / 兜底 temp。
pub fn webview_data_dir(url: &str) -> PathBuf {
  let host = crate::builder::default_title(url)
    .chars()
    .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' })
    .collect::<String>();
  let host = if host.is_empty() { "_default".to_string() } else { host };

  let base = std::env::var_os("LOCALAPPDATA")
    .map(PathBuf::from)
    .or_else(|| std::env::var_os("XDG_DATA_HOME").map(PathBuf::from))
    .unwrap_or_else(std::env::temp_dir);
  base.join("Web2App").join("apps").join(host)
}

/// 注入页面关闭桥：JS 调用 `window.__web2app.close()` 原生关闭窗口。
fn native_close_bridge() -> &'static str {
  r#"window.__web2app = { close: () => { try { window.ipc.postMessage("__web2app_close__"); } catch (e) { console.warn(e); } } };"#
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn close_bridge_uses_ipc_post_message() {
    let js = native_close_bridge();
    assert!(js.contains("window.ipc.postMessage(\"__web2app_close__\")"));
    assert!(js.contains("window.__web2app"));
  }

  #[test]
  fn data_dir_sanitizes_host() {
    let dir = webview_data_dir("https://example.com/x?y=1");
    let s = dir.to_string_lossy().replace('\\', "/");
    assert!(s.ends_with("apps/example.com"), "got: {s}");
    // 非 ASCII host 被替换为安全字符
    let dir2 = webview_data_dir("https://中文站.com");
    let s2 = dir2.to_string_lossy().replace('\\', "/");
    assert!(!s2.contains('中'), "got: {s2}");
  }
}
