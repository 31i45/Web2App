//! master/src/main.rs — 打包器入口。
//!
//! 母版模式：显示打包表单（WebView 内嵌 HTML），用户输入 URL + 可选图标，
//! 点击打包 → 复制 child 模板 + 注入图标 + 追加尾部 + 可选签名 → 产物 exe。
//!
//! master 本身不含 child 的运行时逻辑（打开目标 URL）；它只负责打包。
//! child 模板编译期内嵌（CHILD_TEMPLATE in builder.rs），产物 = child + 尾部。
//!
//! Release 构建为 Windows GUI 子系统：双击运行不出现后台控制台。

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod builder;
mod icon;
mod platform;
mod tail_write;
mod ui;
// Windows-only 打包器平台项：文件图标注入（UpdateResource）与可选签名（signtool）。
// 经 `crate::platform` re-export（见 platform/windows.rs），使 builder.rs 无 #[cfg]。
#[cfg(target_os = "windows")]
mod sign;
#[cfg(target_os = "windows")]
mod winicon;

use tao::event_loop::ControlFlow;
use tao::window::Theme;
use wry::{WebContext, WebView};

/// 事件循环自定义事件：打包请求（图标字节可选，None = 用 w2a 默认图标）。
#[derive(Debug, Clone)]
enum FormEvent {
  Pack { url: String, icon: Option<Vec<u8>> },
}

fn main() {
  run_master();
}

/// 母版模式：表单窗口 + 打包执行。
fn run_master() {
  let event_loop = tao::event_loop::EventLoopBuilder::<FormEvent>::with_user_event().build();
  let proxy = event_loop.create_proxy();

  // 母版窗口图标：内嵌 w2a（Windows 窗口类图标不自动读 exe 资源节，须显式设置）
  let icon = icon::default_window_icon();
  let window = match build_master_window(&event_loop, "Web2App 打包器", Theme::Dark, icon) {
    Ok(w) => w,
    Err(e) => platform::fatal(&e),
  };
  // 关闭哨兵必须在 WebView 构建（wry 挂接 subclass）之前安装，见 platform 文档。
  // 传入 `&Window`（而非平台句柄）使调用点平台无关：Windows 内部自取 HWND，
  // Unix 侧为 no-op（见 platform/linux.rs）。
  platform::install_close_watchdog(&window);
  let mut context = WebContext::new(Some(ui::master_data_dir()));

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
    Err(e) => platform::fatal(&format!(
      "创建母版界面失败：{e}\n{}",
      if cfg!(target_os = "windows") {
        "（Windows 请安装 WebView2 运行时：https://developer.microsoft.com/microsoft-edge/webview2/）"
      } else if cfg!(target_os = "linux") {
        "（Linux 请安装 WebKitGTK 运行库：sudo apt install libwebkit2gtk-4.1-0）"
      } else {
        ""
      }
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
        // 同 child：看门狗保证单条 WM_CLOSE 路径（Alt+F4/任务栏关闭）确定终止。
        platform::quit_watchdog();
      }
      _ => {}
    }
  });
}

/// 构建母版窗口（深色标题栏）。
/// 构建失败返回 Err——与紧随其后的 WebView 构建失败同一策略，由调用方
/// 经 `platform::fatal` 呈现；不在此 panic（GUI 子系统 + panic=abort 即静默死亡）。
fn build_master_window(
  event_loop: &tao::event_loop::EventLoop<FormEvent>,
  title: &str,
  theme: Theme,
  icon: Option<tao::window::Icon>,
) -> Result<tao::window::Window, String> {
  let mut builder = tao::window::WindowBuilder::new()
    .with_title(title)
    .with_theme(Some(theme))
    .with_inner_size(tao::dpi::LogicalSize::new(520u32, 580u32))
    .with_min_inner_size(tao::dpi::LogicalSize::new(420u32, 320u32));
  if let Some(ic) = icon {
    builder = builder.with_window_icon(Some(ic));
  }
  builder.build(event_loop).map_err(|e| format!("创建母版窗口失败：{e}"))
}

/// 母版运行时状态（窗口存活期持有）。
struct Master {
  webview: Option<WebView>,
  #[allow(dead_code)]
  _window: tao::window::Window,
}

impl Master {
  /// 执行打包并回报状态。阻塞 UI 数秒（纯本地复制+注入，无网络等待）。
  fn on_pack(&mut self, url: String, icon: Option<Vec<u8>>) {
    let filename = ui::product_filename(&url);
    let out_dir = dir_of_current_exe();
    let out = out_dir.join(&filename);

    // 图标可选：None / 解码失败 → 产物用 w2a 默认图标，不阻塞打包
    let icon_note = match icon.as_deref().and_then(tail_core::decode_png) {
      Some((w, h, _)) => format!("，图标 {w}x{h}"),
      None => {
        if icon.is_some() {
          "，图标解析失败用默认".to_string()
        } else {
          String::new()
        }
      }
    };

    match platform::pack(&out, &url, icon) {
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
        "(()=>{{const s=document.getElementById('status');if(s){{s.textContent={};s.style.color={};}}const b=document.getElementById('pack');if(b){{b.disabled=false;}}}})();",
        tail_core::js_string_literal(text),
        tail_core::js_string_literal(if ok { "#7ee0a3" } else { "#ff8f8f" })
      );
      if let Err(e) = w.evaluate_script(&js) {
        eprintln!("状态回写失败: {e}");
      }
    }
  }
}

/// 产物输出目录：master 所在目录（自包含，不产生缓存目录）。
///
/// `current_exe` 在 Windows 下返回 `\\?\C:\...` verbatim 形态，剥离后路径更整洁
/// （同时供状态回写展示）。
fn dir_of_current_exe() -> std::path::PathBuf {
  tail_core::current_exe()
    .map(crate::platform::strip_verbatim_prefix)
    .ok()
    .and_then(|p| p.parent().map(|d| d.to_path_buf()))
    .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn product_filename_sanitizes_host() {
    assert_eq!(ui::product_filename("https://example.com"), "example.com.exe");
    assert_eq!(ui::product_filename("https://a.b/c?d=1"), "a.b.exe");
    // 非 ASCII 字符替换为下划线
    let n = ui::product_filename("https://中文站.com");
    assert!(n.ends_with(".com.exe"));
    assert!(n.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_'));
  }
}
