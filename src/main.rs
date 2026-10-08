//! main.rs — 唯一入口。
//!
//! 分发逻辑（一套代码三平台）：
//! 1. `--master`：强制母版模式（产物的自繁殖入口）。
//! 2. 尾部有配置 → 打开目标网页（普通 Web2App 应用）。
//! 3. 尾部无配置 → 母版模式，显示打包表单。
//!
//! Release 构建为 Windows GUI 子系统：双击运行不出现后台控制台。

// GUI 子系统：release 下无控制台窗口（debug 保留便于开发调试）
#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]

mod builder;
mod icon;
mod tail;
mod ui;
mod webview;

use tao::event_loop::ControlFlow;
use tao::window::Theme;
use wry::{WebContext, WebView};

/// 事件循环自定义事件：打包请求（图标字节可选，None = 继承母版 w2a）。
#[derive(Debug, Clone)]
enum FormEvent {
  Pack { url: String, icon: Option<Vec<u8>> },
}

fn main() {
  let args: Vec<String> = std::env::args().collect();

  // 1) `--master` 强制母版模式：产物 exe 由此再次打包新应用（自繁殖入口）
  if args.len() == 2 && args[1] == "--master" {
    return run_master();
  }

  // 2) 读取尾部配置 → 决定母版/应用模式
  match tail::read_tail() {
    Ok(Some(c)) => {
      if let Err(e) = webview::open_webview(c) {
        show_fatal_and_exit(&e);
      }
    }
    Ok(None) => run_master(),
    Err(e) => show_fatal_and_exit(&format!(
      "读取尾部配置失败：{e}\n（文件可能已损坏，请重新获取本程序）"
    )),
  }
}

/// 母版模式：表单窗口 + 打包执行。
fn run_master() {
  let event_loop =
    tao::event_loop::EventLoopBuilder::<FormEvent>::with_user_event().build();
  // IPC 事件经 EventLoopProxy 投递到事件循环
  let proxy = event_loop.create_proxy();

  // 母版窗口图标：内嵌 w2a（Windows 窗口类图标不自动读 exe 资源节，须显式设置）
  let icon = icon::default_window_icon();
  let window = webview::build_window(&event_loop, "Web2App 打包器", Theme::Dark, icon);
  // 母版表单 WebView：固定数据目录（不打扰任何 exe 目录）
  let mut context = WebContext::new(Some(webview::webview_data_dir("web2app-master")));

  let webview = match wry::WebViewBuilder::new_with_web_context(&mut context)
    .with_html(ui::form_html())
    .with_ipc_handler(move |req| {
      // 协议契约的行为级规格见 ui::parse_pack_message 测试
      if let Some((url, icon)) = ui::parse_pack_message(&req.into_body()) {
        let _ = proxy.send_event(FormEvent::Pack { url, icon });
      }
    })
    .build(&window)
  {
    Ok(w) => w,
    Err(e) => show_fatal_and_exit(&format!(
      "创建母版界面失败：{e}\n（Windows 请安装 WebView2 运行时：https://developer.microsoft.com/microsoft-edge/webview2/）"
    )),
  };

  let mut master = Master {
    webview: Some(webview),
    _window: window,
  };

  event_loop.run(move |event, _, control_flow| {
    *control_flow = ControlFlow::Wait;
    match event {
      tao::event::Event::UserEvent(FormEvent::Pack { url, icon }) => master.on_pack(url, icon),
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
  /// 执行打包并回报状态。阻塞 UI 数秒（纯本地复制，无网络等待）。
  fn on_pack(&mut self, url: String, icon: Option<Vec<u8>>) {
    let filename = ui::product_filename(&url);
    let out_dir = dir_of_current_exe();
    let out = out_dir.join(&filename);

    // 图标可选：None / 解码失败 → 产物继承母版 w2a 图标，不阻塞打包
    let icon_note = match icon.as_deref().and_then(icon::decode_png) {
      Some((w, h, _)) => format!("，图标 {w}x{h}"),
      None => {
        if icon.is_some() {
          "，图标解析失败用默认".to_string()
        } else {
          String::new()
        }
      }
    };

    match builder::pack(&out, &url, icon) {
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

  /// 更新页面状态文字（status div）并恢复打包按钮（防重复提交的解锁点）。
  fn set_status(&mut self, text: &str, ok: bool) {
    if let Some(w) = self.webview.as_ref() {
      let js = format!(
        "(()=>{{const s=document.getElementById('status');if(s){{s.textContent={:?};s.style.color={:?};}}const b=document.getElementById('pack');if(b){{b.disabled=false;}}}})();",
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
fn dir_of_current_exe() -> std::path::PathBuf {
  tail::current_exe()
    .ok()
    .and_then(|p| p.parent().map(|d| d.to_path_buf()))
    .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
}

/// 致命错误的用户可见呈现后退出（返回 `!`）。
///
/// 动机：release 为 Windows GUI 子系统且 `panic = "abort"`，`eprintln` 无人看见、
/// panic 无输出——错误必须以用户可感知的方式呈现，否则双击即"静默死亡"。
/// Windows 用 MessageBoxW（`extern` 直接声明，零新依赖）；Unix 有 stderr 直写。
#[cfg(target_os = "windows")]
fn show_fatal_and_exit(msg: &str) -> ! {
  #[link(name = "user32")]
  extern "system" {
    fn MessageBoxW(
      hwnd: *mut core::ffi::c_void,
      text: *const u16,
      caption: *const u16,
      utype: u32,
    ) -> i32;
  }
  const MB_OK: u32 = 0x0;
  const MB_ICONERROR: u32 = 0x10;
  let text: Vec<u16> = msg.encode_utf16().chain(std::iter::once(0)).collect();
  let caption: Vec<u16> = "Web2App".encode_utf16().chain(std::iter::once(0)).collect();
  unsafe {
    MessageBoxW(
      std::ptr::null_mut(),
      text.as_ptr(),
      caption.as_ptr(),
      MB_OK | MB_ICONERROR,
    );
  }
  std::process::exit(1)
}

#[cfg(not(target_os = "windows"))]
fn show_fatal_and_exit(msg: &str) -> ! {
  eprintln!("Web2App 致命错误：{msg}");
  std::process::exit(1)
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
