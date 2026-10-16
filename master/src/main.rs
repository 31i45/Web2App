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

/// 事件循环自定义事件。
#[derive(Debug, Clone)]
enum FormEvent {
  /// 打包请求（图标字节可选，None = 用 w2a 默认图标）。
  Pack { url: String, icon: Option<Vec<u8>> },
  /// 后台打包线程的完成回执。打包必须离开事件循环线程：阻塞主循环 = GDK 停止
  /// 回应 WM ping = 桌面会话判「未响应」（Linux 实测被 GNOME 判死，e2e.sh [4] 门禁）。
  PackDone { out: std::path::PathBuf, result: Result<u64, String>, icon_note: String },
}

fn main() {
  // 无头打包模式：`web2app --pack <url> [--icon <png>] [--out <file>]`——打包动作
  // 的机械入口（G-1 门禁与脚本化打包用；CI 无法驱动 GUI 表单）。不带参数 =
  // GUI 表单，行为不变。
  let mut args = std::env::args().skip(1);
  if args.next().as_deref() == Some("--pack") {
    cli_pack(args);
  } else {
    run_master();
  }
}

/// CLI 打包：解析参数 → 平台 `pack` → 报告产物路径。退出码 0/1/2（成功/打包失败/用法错）。
fn cli_pack(args: impl Iterator<Item = String>) {
  let mut it = args;
  let Some(url) = it.next() else {
    eprintln!("usage: web2app --pack <url> [--icon <png>] [--out <file>]");
    std::process::exit(2);
  };
  let mut icon_path = None;
  let mut out = None;
  while let (Some(k), Some(v)) = (it.next(), it.next()) {
    match k.as_str() {
      "--icon" => icon_path = Some(std::path::PathBuf::from(v)),
      "--out" => out = Some(std::path::PathBuf::from(v)),
      other => {
        eprintln!("unknown option: {other}");
        std::process::exit(2);
      }
    }
  }
  let icon = match icon_path {
    Some(p) => match std::fs::read(&p) {
      Ok(bytes) => Some(bytes),
      Err(e) => {
        eprintln!("PACK_FAIL: read icon {}: {e}", p.display());
        std::process::exit(1);
      }
    },
    None => None,
  };
  let out = out.unwrap_or_else(|| dir_of_current_exe().join(ui::product_filename(&url)));
  match crate::platform::pack(&out, &url, icon) {
    Ok(b) => println!("OK {} ({} bytes)", b.exe.display(), b.size),
    Err(e) => {
      eprintln!("PACK_FAIL: {e}");
      std::process::exit(1);
    }
  }
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

  // proxy 双用：IPC 回投 Pack；run 闭包内 Pack → start_pack（线程）→ PackDone。
  let ipc = proxy.clone();
  let builder = wry::WebViewBuilder::new_with_web_context(&mut context)
    .with_html(ui::form_html())
    .with_ipc_handler(move |req| {
      // 协议契约的行为级规格见 ui::parse_pack_message 测试
      if let Some((url, icon)) = ui::parse_pack_message(&req.into_body()) {
        let _ = ipc.send_event(FormEvent::Pack { url, icon });
      }
    });
  // 挂载经平台外壳：Windows 直通 `build(window)`；Linux 必须走 GTK 容器
  //（Wayland 句柄问题同 child，见 platform::mount_webview）。
  let webview = match crate::platform::mount_webview(builder, &window)
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
      tao::event::Event::UserEvent(FormEvent::Pack { url, icon }) => start_pack(&proxy, url, icon),
      tao::event::Event::UserEvent(FormEvent::PackDone { out, result, icon_note }) => {
        master.on_pack_done(&out, result, &icon_note);
      }
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

/// 打包转后台线程执行，完成经 proxy 回投 PackDone。事件循环线程只做 UI——
/// 阻塞它 = GDK 停止回应 _NET_WM_PING = 桌面会话「未响应」（Linux 实测被
/// GNOME 判死）。防重复提交由表单按钮 disabled 承担，PackDone（set_status）解锁。
/// 产物路径 = master 所在目录/product_filename(url)。
fn start_pack(
  proxy: &tao::event_loop::EventLoopProxy<FormEvent>,
  url: String,
  icon: Option<Vec<u8>>,
) {
  let out = dir_of_current_exe().join(ui::product_filename(&url));
  let proxy = proxy.clone();
  std::thread::spawn(move || {
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
    let result = platform::pack(&out, &url, icon)
      .map(|b| b.size)
      .map_err(|e| e.to_string());
    let _ = proxy.send_event(FormEvent::PackDone { out, result, icon_note });
  });
}

/// 母版运行时状态（窗口存活期持有）。
struct Master {
  webview: Option<WebView>,
  #[allow(dead_code)]
  _window: tao::window::Window,
}

impl Master {
  /// 打包完成回执（后台线程 → PackDone → 主线程）：状态回写并恢复打包按钮。
  fn on_pack_done(&mut self, out: &std::path::Path, result: Result<u64, String>, icon_note: &str) {
    match result {
      Ok(bytes) => {
        let kb = bytes as f64 / 1024.0;
        // Linux 产物是 .deb（ar 归档，不是可执行文件）：完成后独占一行给出
        // **绝对路径**安装命令——相对 ./<name> 在用户当前 cwd 不是产物目录时
        // 照抄即失败（实测：产物在 dist/，终端在 ~）。chmod +x 直接执行必报
        // `!<arch>` 语法错误。apt 接受绝对路径的 .deb（参数含 / 即按文件处理）。
        let hint = if cfg!(target_os = "linux") {
          format!("\n安装（deb 是软件包，不可直接执行）：sudo apt install {}", out.display())
        } else {
          String::new()
        };
        self.set_status(
          &format!("完成：{}（{:.0} KB{}）{}", out.display(), kb, icon_note, hint),
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
/// master 跨平台均为裸可执行分发（Windows .exe / Linux 裸 ELF），用户下载放哪
/// 产物落哪——所在目录必然可写，两平台语义对称。
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
  fn product_filename_matches_install_name() {
    // 文件名 = 安装名：Windows = <host>.exe（PE 单文件）；Linux = <Debian 包名>.deb
    // （包名 = host 本身，见 platform/linux.rs）。同一名字贯穿装/用/卸，
    // `apt install ./<文件名>` 后 `apt remove <文件名去 .deb>` 照抄即可。
    let n = ui::product_filename("https://example.com");
    #[cfg(target_os = "windows")]
    {
      assert_eq!(n, "example.com.exe");
      // 非 ASCII 字符替换为下划线（Windows 文件名允许）
      let m = ui::product_filename("https://中文站.com");
      assert!(m.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_'));
    }
    #[cfg(not(target_os = "windows"))]
    {
      assert_eq!(n, "example.com.deb");
      assert_eq!(ui::product_filename("http://127.0.0.1:8100"), "127.0.0.1-8100.deb");
      // 退化 host（首字符非字母数字）：包名回退 web2app- 前缀保 dpkg 合规
      let m = ui::product_filename("https://中文站.com");
      assert!(m.starts_with("web2app-") && m.ends_with(".deb"), "got: {m}");
      assert!(m[..m.len() - 4].chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.'));
    }
  }
}
