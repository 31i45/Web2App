//! child/src/webview.rs — 统一渲染层（Windows 原生 WebView 适配层）。
//!
//! v2 Windows-only：仅 WebView2 后端。上层（main.rs）只面向 `open_webview`
//! 一个入口。master 有自己的窗口构建逻辑（不依赖本模块）。
//!
//! 主线窗口承载全部宿主侧能力：新窗口路由（见 `popup` 模块）、HTML5 全屏、
//! 缩放手势、剪贴板、下载反馈、标题同步、Alt+Home 回首页，并把页面 `window.close()`
//! 降级为无操作（浏览器对「非脚本打开的顶层窗」同样忽略关闭请求，此处对齐该语义
//! 以免页面意外终结应用）。wry 的回调都是 `'static` 的，拿不到 `&EventLoop`，故需要
//! 宿主侧动作（导航、全屏、标题、提示）统一经 `EventLoopProxy` 回流主循环执行。

use crate::popup::{self, NewWindowAction};
use tail_core::AppConfig;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tao::{
  event::{Event, WindowEvent},
  event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy},
  window::{Fullscreen, Icon, Theme, Window, WindowBuilder},
};
use wry::{WebContext, WebView, WebViewExtWindows};

/// 子应用自定义事件：宿主侧状态变更一律回流主循环处理。
#[derive(Debug)]
pub enum ChildEvent {
  /// 新窗口请求判定为站内 → 在当前窗口就地导航。
  OpenInCurrent(String),
  /// 页面全屏状态变化（HTML5 Fullscreen API）。
  Fullscreen(bool),
  /// 页面文档标题变化 → 同步窗口标题。
  SetTitle(String),
  /// 页面内轻提示（下载反馈等）。
  Toast(String),
}

/// 打开一个 WebView 窗口并阻塞至窗口关闭。
///
/// `config` 提供目标 URL、标题与可选用户图标（设为窗口图标）。
/// 用户选图 → 设为窗口图标；未选图 → 回落内嵌 w2a 默认图标
/// （Windows 窗口类图标不自动读 exe 资源节，须显式设置）。
/// 构建失败（窗口或 WebView）返回 Err——如 Windows 缺 WebView2 运行时、
/// 窗口创建失败等——由调用方决定如何呈现给用户（机制不预设死亡策略）。
pub fn open_webview(config: AppConfig) -> Result<(), String> {
  let event_loop: tao::event_loop::EventLoop<ChildEvent> =
    EventLoopBuilder::<ChildEvent>::with_user_event().build();
  let proxy = event_loop.create_proxy();
  let icon = config
    .icon_png
    .as_deref()
    .and_then(crate::icon::window_icon)
    .or_else(crate::icon::default_window_icon);
  let window = build_window(&event_loop, &config.title, None, icon)?;
  // 关闭哨兵必须在 WebView 构建（wry 挂接 subclass）之前安装，见 tail_core 文档。
  tail_core::arm_close_watchdog(tao::platform::windows::WindowExtWindows::hwnd(&window));
  // WebContext 需存活至事件循环结束（数据目录指向系统应用数据区，不污染 exe 目录）
  let mut context = WebContext::new(Some(webview_data_dir(&config.url)));
  let webview = build_webview(&window, &config, &mut context, proxy)
    .map_err(|e| format!("创建 WebView 失败：{e}\n（Windows 请安装 WebView2 运行时：https://developer.microsoft.com/microsoft-edge/webview2/）"))?;
  // 页面主题跟随系统（含 prefers-color-scheme），与浏览器一致；
  // 旧版 WebView2 运行时可能不支持此 API，静默忽略。
  let _ = webview.set_theme(wry::Theme::Auto);

  event_loop.run(move |event, _, control_flow| {
    *control_flow = ControlFlow::Wait;
    match event {
      // ---- 宿主侧动作（均由 'static 回调经 proxy 转达） ----
      Event::UserEvent(ChildEvent::OpenInCurrent(url)) => {
        let _ = webview.load_url(&url);
      }
      Event::UserEvent(ChildEvent::Fullscreen(on)) => {
        window.set_fullscreen(if on { Some(Fullscreen::Borderless(None)) } else { None });
      }
      Event::UserEvent(ChildEvent::SetTitle(title)) => {
        window.set_title(&title);
      }
      Event::UserEvent(ChildEvent::Toast(msg)) => {
        let _ = webview.evaluate_script(&toast_script(&msg));
      }
      Event::WindowEvent {
        event: WindowEvent::CloseRequested,
        ..
      } => {
        // 生命周期与 WebView 绑定：窗口即应用，关闭即退出，无驻留。
        // 先关弹窗：其 WebView 与 WebView2 子进程必须随宿主一起回收（零残留）。
        popup::close_all();
        // Exit 之外加看门狗兜底：单条 WM_CLOSE 路径（Alt+F4/任务栏关闭/
        // taskkill/自动化）下 WebView2 消息泵可能吞掉退出信号，见 tail_core 文档。
        *control_flow = ControlFlow::Exit;
        tail_core::quit_watchdog();
      }
      _ => {}
    }
  })
  // tao 的 run() 返回 `!`（永不返回），`!` 直接收敛为本函数返回类型；
  // 正常路径到此即退出进程，Err 仅发生在上方构建阶段。
}

/// 构建原生窗口（标题栏主题跟随系统）。
/// `theme`：`None` = 跟随系统；`icon`：窗口图标（None = 不设，Windows 下显示系统默认）。
/// 构建失败返回 Err——与 `build_webview` 一致，由调用方决定呈现方式；
/// 不在此 panic（release 为 GUI 子系统 + panic=abort，panic 即静默死亡）。
pub fn build_window(
  event_loop: &tao::event_loop::EventLoop<ChildEvent>,
  title: &str,
  theme: Option<Theme>,
  icon: Option<Icon>,
) -> Result<Window, String> {
  let mut builder = WindowBuilder::new()
    .with_title(title)
    .with_theme(theme)
    .with_inner_size(tao::dpi::LogicalSize::new(1100u32, 750u32))
    .with_min_inner_size(tao::dpi::LogicalSize::new(420u32, 320u32));
  if let Some(ic) = icon {
    builder = builder.with_window_icon(Some(ic));
  }
  builder.build(event_loop).map_err(|e| format!("创建窗口失败：{e}"))
}

/// 构建主窗口 WebView（产物形态：加载目标 URL）。
///
/// 除加载目标页外，本函数注册了全部宿主侧能力：
/// - 缩放热键 / 捏合缩放、触摸板横扫与侧键前进后退、剪贴板读取（WebView2 均默认关闭）；
/// - 初始化脚本：HTML5 全屏状态上报 + 页面内轻提示 + Alt+Home 回首页；另加一层
///   「关闭守卫」把页面 `window.close()` 降级为无操作（见模块头说明）；
/// - 新窗口请求路由（站内就地导航 / 跨站交系统 / 建独立窗口 / 伪协议忽略），见 `popup`；
/// - 文档标题同步（回流主循环 `set_title`）；
/// - 下载完成反馈：**不**注册「开始下载」处理器——一旦注册 wry 会 `SetHandled(true)`
///   关掉 WebView2 自带的下载面板。保持默认（面板 + 落点）后才补一条完成提示。
///
/// devtools 仅 debug 构建启用（release 零开销）。
/// 失败原因典型为系统 WebView 运行时缺失（Windows WebView2），返回 Err 交由调用方呈现。
pub fn build_webview(
  window: &Window,
  config: &AppConfig,
  context: &mut WebContext,
  proxy: EventLoopProxy<ChildEvent>,
) -> Result<WebView, wry::Error> {
  // 当前页 host：站内/跨站判定的基准，随导航更新（on_page_load 回填）。
  // 回调均在同一线程执行，锁仅为跨闭包共享状态，无竞争。
  let current_host = Arc::new(Mutex::new(tail_core::host_of(&config.url)));

  let ipc_proxy = proxy.clone();
  let title_proxy = proxy.clone();
  let download_done_proxy = proxy.clone();
  let host_for_load = Arc::clone(&current_host);
  // 回首页目标：主窗口与弹窗共用同一份（弹窗内的 Alt+Home 也回到应用起始页）。
  let home_for_popup = config.url.clone();

  let mut builder = wry::WebViewBuilder::new_with_web_context(context)
    .with_url(&config.url)
    .with_hotkeys_zoom(true)
    .with_back_forward_navigation_gestures(true)
    .with_clipboard(true)
    .with_initialization_script(inject_script(&config.url))
    // 关闭守卫仅主窗口需要：弹窗的 `window.close()` 应正常关窗。
    .with_initialization_script(CLOSE_GUARD)
    .with_ipc_handler(move |req| {
      if let Some(flag) = req.into_body().strip_prefix("w2a:fullscreen:") {
        let _ = ipc_proxy.send_event(ChildEvent::Fullscreen(flag == "1"));
      }
    })
    .with_document_title_changed_handler(move |title| {
      let _ = title_proxy.send_event(ChildEvent::SetTitle(title));
    })
    .with_download_completed_handler(move |uri, _path, ok| {
      let verb = if ok { "下载完成" } else { "下载失败" };
      let _ = download_done_proxy.send_event(ChildEvent::Toast(format!("{verb}：{}", file_name(&uri))));
    })
    .with_on_page_load_handler(move |event, loaded| {
      if matches!(event, wry::PageLoadEvent::Finished) {
        if let Ok(mut host) = host_for_load.lock() {
          *host = tail_core::host_of(&loaded);
        }
      }
    })
    .with_new_window_req_handler(move |url, features| {
      // wry 默认不注册本处理器，WebView2 会静默丢弃新窗口请求——这正是
      // 「target=_blank 链接点了没反应」的根因。此处给出唯一出口决策。
      let has_popup = features.size.is_some() || features.position.is_some();
      let opener = current_host.lock().map(|h| h.clone()).unwrap_or_default();
      match popup::classify_new_window(&url, &opener, has_popup) {
        NewWindowAction::SameTab => {
          let _ = proxy.send_event(ChildEvent::OpenInCurrent(url));
          wry::NewWindowResponse::Deny
        }
        NewWindowAction::External => {
          popup::shell_open(&url);
          wry::NewWindowResponse::Deny
        }
        NewWindowAction::Popup => popup::create(&url, features, &home_for_popup).unwrap_or_else(|| {
          // 建窗失败才降级；仅 http(s) 外发有意义（空白窗外发只会弹系统浏览器空白页）。
          popup::open_external_if_http(&url);
          wry::NewWindowResponse::Deny
        }),
        NewWindowAction::Ignore => wry::NewWindowResponse::Deny,
      }
    });

  if cfg!(debug_assertions) {
    builder = builder.with_devtools(true);
  }

  builder.build(window)
}

/// 注入页面的初始化脚本：HTML5 全屏状态上报 + 页面内轻提示。
///
/// 主窗口与弹窗共用（经 [`inject_script`] 与「回首页」脚本拼成一份注入）。
/// 依赖 wry 注入的 `window.ipc.postMessage`（仅在设置了 ipc handler 时存在，
/// 见 wry `webview2/mod.rs` 的 `attach_ipc_handler`；wry 的注入脚本先于本脚本
/// 注册在 document-created 时执行）。全屏只上报状态，真全屏由宿主完成——
/// 主窗口走 tao `set_fullscreen`，弹窗走 `popup::set_fullscreen`（均为原生全屏，
/// 因 WebView2 内部的全屏元素只填满 webview，不是真全屏）。
pub(crate) const INJECT_SCRIPT: &str = r#"
(function () {
  if (window.__w2a) { return; }
  var api = {};
  window.__w2a = api;

  var notify = function () {
    var el = document.fullscreenElement || document.webkitFullscreenElement || null;
    try { window.ipc.postMessage(el ? 'w2a:fullscreen:1' : 'w2a:fullscreen:0'); } catch (e) {}
  };
  document.addEventListener('fullscreenchange', notify, true);
  document.addEventListener('webkitfullscreenchange', notify, true);

  api.toast = function (msg) {
    var box = document.getElementById('__w2a_toast');
    if (!box) {
      box = document.createElement('div');
      box.id = '__w2a_toast';
      box.style.cssText = [
        'position:fixed', 'left:50%', 'bottom:28px', 'transform:translateX(-50%)',
        'max-width:80vw', 'box-sizing:border-box', 'padding:10px 16px',
        'background:rgba(18,18,18,.92)', 'color:#fff', 'border-radius:8px',
        'font:14px/1.5 system-ui,-apple-system,"Segoe UI","Microsoft YaHei",sans-serif',
        'z-index:2147483647', 'pointer-events:none', 'opacity:0',
        'transition:opacity .18s ease', 'word-break:break-all'
      ].join(';');
      (document.body || document.documentElement).appendChild(box);
    }
    box.textContent = msg;
    box.style.opacity = '1';
    clearTimeout(box.__w2aTimer);
    box.__w2aTimer = setTimeout(function () { box.style.opacity = '0'; }, 2600);
  };
})();
"#;

/// 关闭守卫（仅主窗口注入）：把页面 `window.close()` 降级为无操作。
///
/// wry 对 `WindowCloseRequested` 注册了 `DestroyWindow(hwnd)`，页面一旦调用
/// `window.close()` 就会直接销毁主窗口并终结应用。浏览器语义是「非脚本打开的顶层窗
/// 忽略关闭请求」；本应用主窗口从不由脚本打开，故对齐为无操作。弹窗**不**注入本脚本，
/// 其 `window.close()` 仍正常关窗。
const CLOSE_GUARD: &str = r#"
(function () {
  try {
    Object.defineProperty(window, 'close', {
      value: function () {}, writable: true, configurable: true
    });
  } catch (e) {
    try { window.close = function () {}; } catch (e2) {}
  }
})();
"#;

/// 生成注入脚本：共享能力（全屏上报 + 轻提示）+ 可选「回首页」快捷键。
///
/// `home` 为打包时配置的起始 URL；主窗口与弹窗注入同一份，故任何窗口按 Alt+Home
/// 都回到同一首页。为空（无配置的裸模板，正常不可达）时只注入共享部分。
pub(crate) fn inject_script(home: &str) -> String {
  let mut script = String::from(INJECT_SCRIPT);
  if !home.is_empty() {
    script.push_str(&home_hotkey_script(home));
  }
  script
}

/// 「回首页」快捷键脚本：Alt+Home → 回到起始 URL。
///
/// 浏览器把 Alt+Home 定义为「打开主页」，WebView2 没有主页概念，故由托管方补上。
/// 两个要点：只注册在最顶层帧（初始化脚本会注入每个帧，若在 iframe 内注册，Alt+Home
/// 只会导航该 iframe）；是否已在首页在按键时现算而非注入时算定（单页应用经
/// pushState 改地址不会新建文档，注入时算定的结果会过期）。
fn home_hotkey_script(home: &str) -> String {
  format!(
    r#"
(function () {{
  if (window.top !== window.self) {{ return; }}
  var W2A_HOME = {};
  var atHome = function () {{
    try {{
      var u = new URL(W2A_HOME);
      return location.origin === u.origin && location.pathname === u.pathname
        && location.search === u.search;
    }} catch (e) {{ return location.href === W2A_HOME; }}
  }};
  document.addEventListener('keydown', function (ev) {{
    if (!ev.altKey || ev.ctrlKey || ev.shiftKey || ev.metaKey) {{ return; }}
    if (ev.key !== 'Home' && ev.code !== 'Home') {{ return; }}
    ev.preventDefault();
    // 已在首页则不再导航：避免把一次快捷键变成整页刷新、丢掉滚动位置。
    if (!atHome()) {{ location.href = W2A_HOME; }}
  }}, true);
}})();
"#,
    tail_core::js_string_literal(home)
  )
}

/// 生成调用页面内轻提示的脚本（消息转义为 JS 字符串字面量，零 serde）。
fn toast_script(msg: &str) -> String {
  format!(
    "(function(){{try{{window.__w2a&&window.__w2a.toast({});}}catch(e){{}}}})();",
    tail_core::js_string_literal(msg)
  )
}

/// 从下载 URL 取文件名（仅用于提示文案，去除 query/fragment 与百分号编码）。
fn file_name(uri: &str) -> String {
  let path = uri.split(['?', '#']).next().unwrap_or(uri);
  let segment = path.rsplit('/').next().unwrap_or("");
  if segment.is_empty() {
    "文件".to_string()
  } else {
    percent_decode(segment)
  }
}

/// 百分号解码（UTF-8 感知，零依赖）。非法序列原样保留。
fn percent_decode(s: &str) -> String {
  let bytes = s.as_bytes();
  let mut out = Vec::with_capacity(bytes.len());
  let mut i = 0;
  while i < bytes.len() {
    if bytes[i] == b'%' && i + 2 < bytes.len() {
      if let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
        out.push((hi << 4) | lo);
        i += 3;
        continue;
      }
    }
    out.push(bytes[i]);
    i += 1;
  }
  String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
  match b {
    b'0'..=b'9' => Some(b - b'0'),
    b'a'..=b'f' => Some(b - b'a' + 10),
    b'A'..=b'F' => Some(b - b'A' + 10),
    _ => None,
  }
}

/// WebView 用户数据目录：系统应用数据区，exe 旁不落盘。
///
/// 路径为 `tail_core::data_dir("apps")` 下的一级：同一 host 下打包的不同应用
/// （`https://a.com/x` 与 `https://a.com/y`）profile 相互隔离，cookie / 存储 /
/// 缓存互不污染；同一 URL 的多次打包共享同一 profile。基目录探测单点持有在
/// `tail_core::data_base_dir`（与 master 共用同一探测链）。
pub fn webview_data_dir(url: &str) -> PathBuf {
  tail_core::data_dir("apps")
    .join(format!("{}_{}", tail_core::sanitize_host(url), tail_core::url_fingerprint(url)))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn data_dir_is_partitioned_by_host_and_url() {
    let dir = webview_data_dir("https://example.com/x?y=1");
    let s = dir.to_string_lossy().replace('\\', "/");
    assert!(s.contains("/Web2App/apps/example.com_"), "got: {s}");
    // 同 host 不同 URL → 不同目录（profile 隔离）
    let other = webview_data_dir("https://example.com/other").to_string_lossy().replace('\\', "/");
    assert_ne!(s, other, "same host with different path must not share a profile");
    // 同一 URL 幂等
    assert_eq!(s, webview_data_dir("https://example.com/x?y=1").to_string_lossy().replace('\\', "/"));
  }

  #[test]
  fn data_dir_sanitizes_non_ascii_host() {
    let dir = webview_data_dir("https://中文站.com");
    let s = dir.to_string_lossy().replace('\\', "/");
    assert!(!s.contains('中'), "got: {s}");
  }

  #[test]
  fn data_base_dir_is_absolute() {
    // 探测链不 panic 且返回绝对路径（LOCALAPPDATA 或 temp 都是绝对路径）
    assert!(tail_core::data_base_dir().is_absolute());
  }

  #[test]
  fn inject_script_embeds_home_hotkey_and_escapes_it() {
    let url = "https://a.com/x?a=1&b=\"q\"";
    let script = inject_script(url);
    // 首页 URL 以 JS 字符串字面量形式原样嵌入（含正确转义）
    assert!(script.contains(&tail_core::js_string_literal(url)), "got: {script}");
    assert!(!script.contains("b=\"q\""), "raw quote leaked into JS: {script}");
    assert!(script.contains("W2A_HOME"), "hotkey script missing");
    // 共享部分始终注入；无首页（裸模板）时不注册快捷键
    assert!(script.contains("__w2a"), "shared script missing");
    let bare = inject_script("");
    assert!(bare.contains("__w2a") && !bare.contains("W2A_HOME"));
  }

  #[test]
  fn file_name_extracts_and_decodes() {
    assert_eq!(file_name("https://a.com/dir/report%20final.pdf?x=1"), "report final.pdf");
    assert_eq!(file_name("https://a.com/%E4%B8%AD%E6%96%87.zip"), "中文.zip");
    assert_eq!(file_name("https://a.com/"), "文件");
  }

  #[test]
  fn percent_decode_handles_utf8_and_invalid_sequences() {
    assert_eq!(percent_decode("a%20b"), "a b");
    assert_eq!(percent_decode("%E4%B8%AD"), "中");
    assert_eq!(percent_decode("100%"), "100%");
    assert_eq!(percent_decode("a%zz"), "a%zz");
  }
}
