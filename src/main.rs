//! main.rs — 唯一入口。
//!
//! 分发逻辑（一套代码三平台）：
//! 1. `--set-icon <png> [target]`：工具模式，构建脚本自举母版 logo。
//! 2. `--master`：强制母版模式（产物的自繁殖入口）。
//! 3. 尾部有配置 → 打开目标网页（普通 Web2App 应用）。
//! 4. 尾部无配置 → 母版模式，显示打包表单。
//!
//! Release 构建为 Windows GUI 子系统：双击运行不出现后台控制台。

// GUI 子系统：release 下无控制台窗口（debug 保留便于开发调试）
#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]

mod builder;
mod icon;
mod pe;
mod tail;
mod ui;
mod webview;

use tao::event_loop::ControlFlow;
use tao::window::Theme;
use wry::{WebContext, WebView};

/// 事件循环自定义事件：打包请求。
#[derive(Debug, Clone)]
enum FormEvent {
  Pack { url: String },
}

fn main() {
  let args: Vec<String> = std::env::args().collect();

  // 1) 工具模式：`--set-icon <png> [target.exe]`，构建脚本自举母版 logo。
  //    Windows 进程无法写自身（文件锁），故总是对「发布副本」注入。
  if args.len() >= 3 && args.len() <= 4 && args[1] == "--set-icon" {
    let png = std::path::PathBuf::from(&args[2]);
    let target = args
      .get(3)
      .map(std::path::PathBuf::from)
      .unwrap_or_else(|| tail::current_exe().unwrap_or_default());
    match builder::tool_set_icon(&png, &target) {
      Ok(_) => println!("icon injected"),
      Err(e) => {
        eprintln!("set-icon failed: {e}");
        std::process::exit(1);
      }
    }
    return;
  }

  // 2) `--master` 强制母版模式：产物 exe 由此再次打包新应用（自繁殖入口）
  if args.len() == 2 && args[1] == "--master" {
    return run_master();
  }

  // 3) 读取尾部配置 → 决定母版/应用模式
  match tail::read_tail() {
    Ok(Some(c)) => webview::open_webview(c),
    Ok(None) => run_master(),
    Err(e) => {
      eprintln!("读取尾部配置失败: {e}");
      std::process::exit(1);
    }
  }
}

/// 母版模式：表单窗口 + 打包执行。
fn run_master() {
  let event_loop =
    tao::event_loop::EventLoopBuilder::<FormEvent>::with_user_event().build();
  // IPC 事件经 EventLoopProxy 投递到事件循环
  let proxy = event_loop.create_proxy();

  let window = webview::build_window(&event_loop, "Web2App 打包器", Theme::Dark);
  // 母版表单 WebView：固定数据目录（不打扰任何 exe 目录）
  let mut context = WebContext::new(Some(webview::webview_data_dir("web2app-master")));

  let webview = wry::WebViewBuilder::new_with_web_context(&mut context)
    .with_html(ui::form_html())
    .with_ipc_handler(move |req| {
      // JS 端约定：`pack:<url>`
      let body = req.into_body();
      if let Some(url) = body.strip_prefix("pack:") {
        let _ = proxy.send_event(FormEvent::Pack { url: url.to_string() });
      }
    })
    .build(&window)
    .expect("create master webview");

  let mut master = Master {
    webview: Some(webview),
    _window: window,
  };

  event_loop.run(move |event, _, control_flow| {
    *control_flow = ControlFlow::Wait;
    match event {
      tao::event::Event::UserEvent(FormEvent::Pack { url }) => master.on_pack(url),
      tao::event::Event::WindowEvent {
        event: tao::event::WindowEvent::CloseRequested,
        ..
      } => {
        master.webview = None;
        *control_flow = ControlFlow::Exit;
      }
      _ => {}
    }
  });
}

/// 母版运行时状态（窗口存活期持有）。
struct Master {
  webview: Option<WebView>,
  #[allow(dead_code)]
  _window: tao::window::Window,
}

impl Master {
  /// 执行打包并回报状态。阻塞 UI 数秒（favicon 拉取 ≤10s + 复制秒级完成）。
  fn on_pack(&mut self, url: String) {
    let filename = ui::product_filename(&url);
    let out_dir = dirs_of_current_exe();
    let out = out_dir.join(&filename);

    // favicon 获取：可选增强，任何失败不阻塞打包（产物照常生成，仅无图标）
    let favicon = icon::fetch_favicon(&url);
    let icon_note = match &favicon {
      Some(v) => format!("，图标 {} 种尺寸", v.len()),
      None => String::new(),
    };

    match builder::pack(&out, &url, favicon) {
      Ok(result) => {
        let kb = result.size as f64 / 1024.0;
        self.set_status(
          &format!("完成：{}（{:.0} KB{}）", result.exe.display(), kb, icon_note),
          true,
        );
      }
      Err(e) => self.set_status(&format!("失败：{e}"), false),
    }
  }

  /// 更新页面状态文字（status div）。
  fn set_status(&mut self, text: &str, ok: bool) {
    if let Some(w) = self.webview.as_ref() {
      let js = format!(
        "(()=>{{const s=document.getElementById('status');if(s){{s.textContent={:?};s.style.color={:?};}}}})();",
        text,
        if ok { "#7ee0a3" } else { "#ff8f8f" }
      );
      if let Err(e) = w.evaluate_script(&js) {
        eprintln!("状态回写失败: {e}");
      }
    }
  }
}

/// 产物输出目录：母版所在目录（自包含，不产生缓存目录）。
fn dirs_of_current_exe() -> std::path::PathBuf {
  tail::current_exe()
    .ok()
    .and_then(|p| p.parent().map(|d| d.to_path_buf()))
    .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn product_filename_sanitizes_host() {
    let ext = if cfg!(target_os = "windows") { ".exe" } else { "" };
    assert_eq!(ui::product_filename("https://example.com"), format!("example.com{ext}"));
    assert_eq!(ui::product_filename("https://a.b/c?d=1"), format!("a.b{ext}"));
    // 非 ASCII 字符替换为下划线
    let n = ui::product_filename("https://中文站.com");
    assert!(n.ends_with(&format!(".com{ext}")));
    assert!(n.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_'));
  }
}
