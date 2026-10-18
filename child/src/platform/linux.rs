//! child/src/platform/linux.rs — Linux 平台外壳（child 侧）。
//!
//! 与 Windows 侧**同名同签名**的项收敛于此，使 Linux 目标可编译，且调用点
//!（`webview.rs` / `main.rs`）平台无关（见 `docs/PLATFORM_SEAM_NOTES.md` §1）。
//! 具体行为按设计文档 §7 阶段 1 落地：T-1-1 GTK 外壳（建窗 / 外发 / 弹窗 /
//! 全屏 / 关闭）、T-1-2 按键拦截（组件级 `key-press-event`，见下）、
//! T-1-3 数据目录隔离。未落地者以「阶段 1」标注，缺一或签名不符即编译失败
//!（这是刻意设计）。

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use gtk::{gdk, glib, prelude::*};
use tao::event_loop::EventLoopProxy;
use wry::{
  NewWindowFeatures, NewWindowResponse, WebView, WebViewBuilder, WebViewBuilderExtUnix,
  WebViewExtUnix,
};

use crate::popup::{NewWindowAction, PopupIpc};
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

/// no-op：看门狗是为绕开 Windows「WebView2 嵌套 COM 消息泵吞掉 WM_QUIT / WM_CLOSE」
/// 而存在（见 Windows 侧同名函数文档）。GTK 侧无此结构性吞没：主窗口的
/// `delete-event` 由 tao 转成 `WindowEvent::CloseRequested`（见 tao
/// `platform_impl/linux/event_loop.rs` 的 `connect_delete_event`），随后
/// `ControlFlow::Exit` 使 `run_return` 返回、`run` 立即 `process::exit`——确定退出，
/// 故 0b/阶段 1 均不武装看门狗。
pub(crate) fn quit_watchdog() {}

/// no-op：关闭哨兵（WNDPROC 替换 + 看门狗）是纯 Win32 机制，GTK 侧无对应物。
/// 参数仅用于与 Windows 侧同签名（调用点在 `webview.rs`，平台无关）。
///
/// GTK 侧弹窗自身的关闭确定性由 [`create_popup`] 内的 `delete-event` 处理保证
/// （同步 `destroy` + 摘除注册表项），非主窗口看门狗职责。
pub(crate) fn install_close_watchdog(_window: &tao::window::Window) {}

// ============================================================================
// 宿主层按键（T-1-2：V-3 三候选实测后落定「组件级 key-press-event」）
// ============================================================================
//
// 为什么需要宿主层：页面层 `keydown`（见 `webview::home_hotkey_script`）在**焦点位于
// 跨源 iframe 内**时收不到按键——按键被 iframe 的文档吞掉，托管页拿不到。宿主层的
// 组件级 `key-press-event` 在**按键送达内容层之前**于 GTK 层回调，因而能覆盖该边界。
// 分层职责与 Windows 侧一致：宿主层只做「判定」（[`is_home_accel`]）与「送达」
//（派发 `ChildEvent::GoHome` / 执行 `HOME_GO_SCRIPT`）；「是否已在首页 / 在途守卫 /
// BFCache 回跳」等决策全留在页面层。
//
// 机制选定（V-3，实测见 `docs/PLATFORM_SEAM_NOTES.md` §3）——三候选对比：
// - a `gtk::AccelGroup`（声明式加速键）：须把加速键组注册到**窗口**，作用域是全窗而非
//   该 WebView；V-3 已列其「可能看不到 F5/Ctrl+R」的疑虑。
// - b `gtk_key_snooper_install`：GTK3 已废弃、进程级全局、需 `unsafe` raw FFI，且须自行
//   按窗口过滤，与 WebView2 的 per-controller 语义相距最远。
// - c **组件级 `key-press-event`（选定）**：信号挂在 WebView 组件自身，作用域恰为该
//   WebView、无全局状态、无废弃 API，与 Windows `AcceleratorKeyPressed`（注册在该
//   controller 上）语义最接近；设计文档 §5.1 亦明示「优先组件级 `key-press-event`」。
//   VM 无头实测：a/b/c 在「页面 / 跨源 iframe / 可编辑框」三焦点下均能命中 Alt+Home，
//   故按设计偏好取 c。
//
// 双路径派发去重（Linux 特有，VM 实测）：同一物理按键的 GdkEvent 会被 GTK 经两条路径
// 派发到客户端（两条日志 `time` **完全相同**；X 层只有 1 次 KeyPress，纯 tao 窗口无此
// 现象）→ 若不去重，一次 Alt+Home 会送达两次。按**事件时间戳**去重：同一 `GdkEvent` 的
// 两份拷贝时间戳相同，而两次独立按键的时间戳必不相同（X 服务器毫秒时钟单调递增）。
//
// 自动重复：GDK 不提供 Windows `WasKeyDown` 那样的自动重复位（重复按下时间戳递增，
// 会被放行）。这与 Windows 的差异**不外显**——页面层 `homeAction` 的 `busy` / `isHome`
// 守卫使重复触发幂等（导航在途或已在首页时不动作），与阶段 1 两层并存的既有设计一致。

/// 纯判定：给定 keyval 与修饰位，是否为「Alt+Home 按下」——与 Windows [`is_home_accel`]
/// 语义对等（取纯输入以便无 GTK 环境单测，Windows 侧同构）。
///
/// `key-press-event` 只会给出按下事件（无 KeyUp），故无需像 Windows 那样再按 `kind`
/// 排除 KeyUp。`alt` 取 `MOD1_MASK`（GDK 跨后端统一把 Alt 映射到 `GDK_MOD1_MASK`，X11
/// 的 Mod1 与 Wayland 的合成修饰均一致）；`Home` 取主键区 keyval（`KP_Home` 是不同
/// keyval，不命中——与 Windows 的 `VK_HOME` 语义一致）。与 Windows 一致，**只**看 Alt：
/// 额外按下 Ctrl/Shift 不影响命中。
fn is_home_accel(keyval: gdk::keys::Key, mods: gdk::ModifierType) -> bool {
  keyval == gdk::keys::constants::Home && mods.contains(gdk::ModifierType::MOD1_MASK)
}

/// 在 WebView 组件上注册 Alt+Home 处理器，命中时执行 `action`。
///
/// 与 Windows 侧同名函数对称：`action` 由调用方按窗口形态给出（主窗口回流
/// `ChildEvent::GoHome` 经 proxy → 主循环 `evaluate_script`；弹窗在本窗 WebView 上直接
/// 执行 `HOME_GO_SCRIPT`）。仅 [`crate::webview::HOST_ONLY`]（阶段 2 宿主层独占）时返回
/// `Stop` 吞掉按键；阶段 1 返回 `Proceed` 与页面层并存（对应 Windows 的 `SetHandled`
/// 门控）。
pub(crate) fn register_home_accel(webview: &WebView, action: impl Fn() + 'static) {
  // 组件级信号：`WebViewExtUnix::webview()` 取 WebKitGTK 原生 `WebKitWebView` widget
  //（它 `@extends gtk::Widget`，故 `connect_key_press_event` 可用；作用域即该组件）。
  let widget = webview.webview();
  let last_time = Cell::new(None::<u32>);
  widget.connect_key_press_event(move |_, ev| {
    if !is_home_accel(ev.keyval(), ev.state()) {
      return glib::Propagation::Proceed;
    }
    // 时间戳去重：吞掉同一次物理按键的第二份派发（见上方机制说明）。
    let t = ev.time();
    if last_time.get() == Some(t) {
      return glib::Propagation::Proceed;
    }
    last_time.set(Some(t));
    action();
    if crate::webview::HOST_ONLY {
      glib::Propagation::Stop
    } else {
      glib::Propagation::Proceed
    }
  });
}

/// 主窗口：命中 Alt+Home → 回流 `ChildEvent::GoHome`（宿主层按键机制入口，§1 接缝项）。
///
/// 动作经 proxy 回流主循环再 `evaluate_script`，与 Windows 侧同构、统一收口——不在
/// 按键回调内同步求值。
pub(crate) fn install_keys(webview: &WebView, proxy: EventLoopProxy<ChildEvent>) {
  register_home_accel(webview, move || {
    let _ = proxy.send_event(ChildEvent::GoHome);
  });
}

// ============================================================================
// WebView 挂载（TO-1 接缝项：Linux 必须走 GTK 容器，见下）
// ============================================================================

/// 把构建好的 WebView 挂到窗口上——**Linux 必须走 `build_gtk`，不走 wry 的通用
/// `build(window)`**。
///
/// 为什么（VM 实测，见 `docs/PLATFORM_SEAM_NOTES.md` §3）：wry 的 `webkitgtk` 后端
/// 通用 `build()` 只有 `x11` 一条句柄路径，**只接受 `RawWindowHandle::Xlib`**；而
/// tao 在 Wayland 会话下交出的句柄是 `Wayland`（tao 依 GDK 后端类型二选一）→ 直接报
/// `Error::UnsupportedWindowHandle`，**Linux 上根本起不了 WebView**（0b 未做运行时
/// 验证故未暴露）。wry 官方 README 对「同时支持 X11 与 Wayland」的唯一建议是
/// `WebViewBuilderExtUnix::build_gtk` 挂到 tao 的默认 GTK 容器；该路径不经句柄转换，
/// 与 GDK 后端无关（X11 与 Wayland 两种后端均已实测通过）。
///
/// 容器取 `WindowExtUnix::default_vbox`（tao 默认加为窗口唯一子节点的纵向 `GtkBox`）。
/// **不可取 `gtk_window()`**：`GtkApplicationWindow` 是 `GtkBin` 子类，已被该 `GtkBox`
/// 占满，再 `add` 会触发 GTK 警告且布局无保证（VM 实测复现该警告）。`default_vbox`
/// 返回 `None` 只发生在显式 `with_default_vbox(false)`（本应用不使用）→ 回落通用
/// `build`，与 Windows 的降级路径一致（失败即 `Err`，由调用方决定呈现方式）。
pub(crate) fn mount_webview<'a>(
  builder: WebViewBuilder<'a>,
  window: &'a tao::window::Window,
) -> Result<WebView, wry::Error> {
  // `WindowExtUnix` 是 `tao::platform::unix` 的 Unix-only trait——平台判断留在本文件内。
  use tao::platform::unix::WindowExtUnix;
  match window.default_vbox() {
    Some(container) => builder.build_gtk(container),
    None => builder.build(window),
  }
}

// ============================================================================
// External：交给系统默认程序
// ============================================================================

/// 用系统默认程序打开 URL（XDG `xdg-open`，与 Windows `ShellExecuteW` 同语义）。
///
/// 异步派发（`spawn` 而非 `status`），不阻塞 UI 线程——与 `ShellExecuteW` 的非阻塞
/// 行为一致。真协议（`http(s)` / `mailto:` / 自定义协议…）交给系统协议处理器是
/// 浏览器的标准行为（阶段 1 可考虑按桌面环境细化为 `gio open`）。
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
// Popup：独立 GTK 顶层窗 + WebView（阶段 1：T-1-1）
// ============================================================================
//
// 三层事实决定了本段形状（均在 VM 上核对过源码，见 docs/PLATFORM_SEAM_NOTES.md §3）：
//
// 1. **必须自建窗口**：wry 的新窗口回调是 `'static` 的，拿不到 tao `&EventLoop`
//    （`run()` 消耗 `self`），无法用 `WindowBuilder::build` 建窗；GTK 侧改用
//    `gtk::Window::new(Toplevel)` 自建顶层窗。这与 wry 内建 `Allow` 分支的做法同构
//    （它自建 `gtk::ApplicationWindow` + `Box` 再返回 webview widget）。
// 2. **返回载荷是原生 WebView**：`NewWindowResponse::Create` 在 Linux 上的载荷类型为
//    `webkit2gtk::WebView`（`wry` 收到后 `upcast::<gtk::Widget>()` 交给 WebKit 当新窗
//    内容）；Windows 的载荷是 `ICoreWebView2`。**故设计文档取同签名 `#[cfg]` 模块而非
//    抽象**（ADR-1）。
// 3. **related view 是契约**：新 WebView 必须与发起方 related——`webkit_web_view_new_with_related_view`
//    使新视图**共享同一 web 进程与网络会话**（WebKitGTK 文档原话：「to share the same
//    web process and network session」）→ 弹窗与主窗口同 cookie/存储/profile。
//    故 `with_related_view(features.opener.webview)` 不可省。
//
// 弹窗内再发起的新窗口请求走同一套纯判定（`popup::classify_new_window`）与平台管线
//（嵌套受限：`create_popup` 递归建窗）。

/// 存活弹窗（key = 本模块自增 id，与 Windows 侧以 HWND 为 key 语义对等）。
///
/// 必须持有 `WebView`：`NewWindowResponse::Create` 交给 WebKit 的原生 WebView 归该
/// WebView 所有，提前 drop 会令弹窗失去宿主（wry 的 `Allow` 分支同样把 webview 存进
/// 内部 `related_webviews` 表）。`WebView` 非 `Send`，故用 thread_local
///（GTK 回调与窗口信号都在 GTK 主线程上执行）。
struct Popup {
  window: gtk::Window,
  webview: WebView,
}

thread_local! {
  static POPUPS: RefCell<HashMap<isize, Popup>> = RefCell::new(HashMap::new());
  /// 弹窗句柄 id 分配器。GTK 侧没有可作 key 的原生句柄（与 Win32 的 HWND 不同），
  /// 故自增分配；用于在 `navigate_popup` / `set_fullscreen` / `close_all_popups` 之间
  /// 标识弹窗，并由宿主注入弹窗文档（`window.__w2aIpcTag`）供 ipc 消息回投（见 §3.7）。
  static NEXT_POPUP_ID: Cell<isize> = const { Cell::new(1) };
}

/// 生成弹窗的「本窗标记 + 关闭覆盖」脚本（每次文档提交时由宿主注入本窗，见 [`create_popup`]）。
///
/// 为什么不能用 `with_initialization_script`（VM 实测的平台事实，见
/// `docs/PLATFORM_SEAM_NOTES.md` §3.7）：Linux 弹窗的 WebView 必须以 `related_view`
/// 依附发起方（否则 WebKit 内部断言 `WindowFeatures` abort），而**承载文档的是发起方
/// （主窗）的 `UserContentManager`**——弹窗自身 `WebViewBuilder` 上的 UCM（含 `wry`
/// 注入的 `window.ipc` 与本窗初始化脚本）**从不生效**。故本窗唯一的注入通道是宿主在
/// **文档提交**时对本窗 `evaluate_script`（wry 的 `PageLoadEvent::Started` 在 webkitgtk
/// 上即 `LoadEvent::Committed`）。
///
/// 脚本做两件事，均内嵌本窗 `id`：
/// 1. 置 `window.__w2aIpcTag`——主窗的 `INJECT_SCRIPT`（在弹窗文档中同样生效，见上）在
///    `send` 处按该标记给消息追加 `w2a:popup:<id>:` 前缀，使主窗处理器能把消息回投本窗
///    （见 [`popup_ipc`] 与 `webview::POPUP_IPC_PREFIX`）；
/// 2. 覆盖 `window.close()`——WebKitGTK **拒绝**应用自建视图的关闭请求（页面非
///    `openedByDOM`，连 `close` 信号都不发出，见模块内注释），且主窗的 `CLOSE_GUARD`
///    在弹窗文档中同样生效（把它降级为无操作），故须覆盖为「经 IPC 送回宿主」，
///    由 [`close_popup`] 销毁整窗——即 Windows 侧 `WindowCloseRequested → DestroyWindow`
///    的 Linux 对等物。
fn popup_bind_script(id: isize) -> String {
  format!(
    r#"(function () {{
  window.__w2aIpcTag = {id};
  try {{
    Object.defineProperty(window, 'close', {{
      value: function () {{
        try {{ window.ipc.postMessage('w2a:popup:{id}:close'); }} catch (e) {{}}
      }},
      writable: true, configurable: true
    }});
  }} catch (e) {{}}
}})();"#
  )
}

/// 为 `Popup` 分支建独立 GTK 顶层窗并挂 WebView，返回交给 WebKit 的 `Create` 响应。
///
/// 任一步失败返回 `None`——调用方按 [`open_external_if_http`] 降级，不 panic
///（release 为 `panic = "abort"`，panic 即静默死亡）。
///
/// `features` 由 wry 给出：Linux 下 `size` / `position` 恒为 `None`（wry 的
/// `webkitgtk` 后端不转发 `window.open` 的尺寸/位置特征）→ 页面级的「带特征弹窗」
/// 在 Linux 上无法与「普通跨站链接」区分，故这类请求按纯判定归入 `SameTab` /
/// `External`，只有空白窗（`window.open()` 无 URL）与嵌套复用走本函数。这是 wry
/// 后端的事实，非本代码的选择；等价于阶段 1 前的现状，非退化。
/// `opener.webview` 是必须沿用的 related view（见模块头第 3 点）。
pub(crate) fn create_popup(
  url: &str,
  features: NewWindowFeatures,
  home: &str,
) -> Option<NewWindowResponse> {
  // GTK 未初始化时建窗必 panic → 显式确认并降级（正常不可达：本函数只在 tao 事件
  // 循环运行中的新窗口回调里被调用）。
  if !gtk::is_initialized_main_thread() {
    return None;
  }
  // 尺寸/位置：与 Windows 侧同策略（下限钳制避免站点用 1x1 制造用户看不见也关不掉
  // 的窗口）。Linux 下 features 恒为 None → 走缺省值。
  let (w, h) = match features.size.as_ref() {
    Some(s) => (
      clamp(s.width.round() as i32, 320, 10000),
      clamp(s.height.round() as i32, 240, 10000),
    ),
    None => (960, 720),
  };
  let id = NEXT_POPUP_ID.with(|c| {
    let v = c.get();
    c.set(v + 1);
    v
  });

  let window = gtk::Window::new(gtk::WindowType::Toplevel);
  // 初始标题：空白窗用品牌名（避免窗口标题显示 "about:blank"），其余用 host
  //（与 Windows 侧同规则）。
  let title_src = if crate::popup::is_blank(url) { "" } else { url };
  window.set_title(&tail_core::default_title(title_src));
  window.set_default_size(w, h);
  if let Some(p) = features.position.as_ref() {
    window.move_(p.x.round() as i32, p.y.round() as i32);
  }

  let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
  window.add(&container);

  // 弹窗自身的同站判定基准：随其导航更新（on_page_load 回填）。
  let host = Rc::new(RefCell::new(tail_core::host_of(url)));
  let home_for_nested = home.to_string();
  let title_window = window.clone();
  let host_for_load = Rc::clone(&host);
  // 本窗「标记 + 关闭覆盖」脚本（每次文档提交注入，见 [`popup_bind_script`]）。
  let bind_for_load = popup_bind_script(id);

  let mut builder = WebViewBuilder::new()
    // 刻意不 with_url：WebKit 会在本处理器的 `Create` 响应后自行导航到目标 URL
    //（与 Windows 的 WebView2 `SetNewWindow` 契约同构）。
    // 不设 `with_initialization_script` / `with_ipc_handler`：本平台弹窗文档承载在
    // 发起方（主窗）的 UCM 上，本窗 UCM 上的注入从不生效（见 [`popup_bind_script`]）。
    .with_hotkeys_zoom(true)
    .with_back_forward_navigation_gestures(true)
    .with_clipboard(true)
    .with_document_title_changed_handler(move |title| {
      title_window.set_title(&title);
    })
    .with_on_page_load_handler(move |event, loaded| {
      if matches!(event, wry::PageLoadEvent::Started) {
        // 文档已提交：注入本窗标记与关闭覆盖（从注册表取本窗 WebView 执行）。
        POPUPS.with(|m| {
          if let Some(p) = m.borrow().get(&id) {
            let _ = p.webview.evaluate_script(&bind_for_load);
          }
        });
      } else {
        *host_for_load.borrow_mut() = tail_core::host_of(&loaded);
      }
    })
    // 弹窗内部再发起的新窗口请求走同一套路由（同站就地导航、跨站交系统、嵌套受限）。
    .with_new_window_req_handler(move |target, f| {
      let has_popup = f.size.is_some() || f.position.is_some();
      let opener = host.borrow().clone();
      match crate::popup::classify_new_window(&target, &opener, has_popup) {
        NewWindowAction::SameTab => {
          navigate_popup(id, &target);
          NewWindowResponse::Deny
        }
        NewWindowAction::External => {
          shell_open(&target);
          NewWindowResponse::Deny
        }
        NewWindowAction::Popup => match create_popup(&target, f, &home_for_nested) {
          Some(response) => response,
          None => {
            open_external_if_http(&target);
            NewWindowResponse::Deny
          }
        },
        NewWindowAction::Ignore => NewWindowResponse::Deny,
      }
    })
    // 契约：新 WebView 必须与发起方 related（共享 web 进程与网络会话，见模块头）。
    // **必需**：去掉后 WebKit 内部断言 `WindowFeatures` abort（实测）。
    .with_related_view(features.opener.webview);
  if cfg!(debug_assertions) {
    builder = builder.with_devtools(true);
  }

  let webview = match builder.build_gtk(&container) {
    Ok(v) => v,
    Err(_) => return None,
  };
  // 交给 WebKit 的原生视图（GObject 引用计数，clone 后再把 wry 侧 WebView 存入注册表）。
  let raw = webview.webview();

  // 宿主层 Alt+Home（弹窗路径，与 Windows 侧 `register_home_accel` + `WM_APP_HOME` 对等）：
  // 命中即在本窗 WebView 上执行页面层的决策单点 `__w2aHome()`。GTK 侧无需像 Windows 那样
  // 经窗口消息延后派发——`evaluate_script` 走 WebKit 异步 `run_javascript`（不阻塞、不在
  // GTK 信号回调内重入），故直接按 id 从注册表取本窗 WebView 执行。回调在按键时触发，
  // 此时本弹窗项已在 `POPUPS` 中（下方插入），取用时仅短借 `POPUPS`（`evaluate_script`
  // 不同步回改注册表，无 `RefCell` 重入）。
  register_home_accel(&webview, move || {
    POPUPS.with(|m| {
      if let Some(p) = m.borrow().get(&id) {
        let _ = p.webview.evaluate_script(crate::webview::HOME_GO_SCRIPT);
      }
    });
  });

  // 弹窗关闭的回收钩子（与 Windows 侧 WM_DESTROY → 摘除 POPUPS 项对等）。
  // 关闭路径本身交给 GTK：`delete-event` 未挂处理器时由 GTK 默认处理器**同步**
  // `gtk_widget_destroy`（这正是 Windows 侧 WM_CLOSE 分支要「同步 DestroyWindow」
  // 才换来的确定性），随后本就绪的 `destroy` 信号在此摘除注册表项，避免持有
  // 已销毁窗口。
  window.connect_destroy(move |_| {
    POPUPS.with(|m| {
      m.borrow_mut().remove(&id);
    });
  });

  window.show_all();
  POPUPS.with(|m| {
    m.borrow_mut().insert(id, Popup { window, webview });
  });
  Some(NewWindowResponse::Create { webview: raw })
}

/// 让指定弹窗就地导航（其内部发起的站内新窗口请求）。
pub(crate) fn navigate_popup(handle: isize, url: &str) {
  POPUPS.with(|m| {
    if let Some(p) = m.borrow().get(&handle) {
      let _ = p.webview.load_url(url);
    }
  });
}

/// 关闭全部弹窗（主窗口关闭前调用，保证窗口 / WebKit 子进程零残留）。
pub(crate) fn close_all_popups() {
  // 先取窗口快照：`close` 会经 `delete-event` 默认处理器**同步**销毁窗口并触发
  // `connect_destroy` 回改 POPUPS，持着借用边遍历边改会 panic。
  let windows: Vec<gtk::Window> = POPUPS.with(|m| m.borrow().values().map(|p| p.window.clone()).collect());
  for w in windows {
    // `gtk_window_close` 请求关闭：触发 delete-event（本窗未挂处理器）→ GTK 默认
    // 处理器同步销毁，语义与 Windows 侧 `DestroyWindow` 对等且无需 unsafe。
    w.close();
  }
}

/// 弹窗全屏切换。
///
/// 仅由弹窗的 ipc 回调在 GTK 主线程调用；重复进入全屏幂等（GTK 自身即幂等）。
/// 主窗口全屏不经本函数（走 tao `Window::set_fullscreen`）。
///
/// 先克隆窗口再离开借用作用域执行：GTK 全屏会引发窗口状态/尺寸变化信号，
/// 持着 `POPUPS` 借用执行有重入改表（`RefCell` 双重借用 panic）的风险。
pub(crate) fn set_fullscreen(handle: isize, on: bool) {
  let window = POPUPS.with(|m| m.borrow().get(&handle).map(|p| p.window.clone()));
  if let Some(w) = window {
    if on {
      w.fullscreen();
    } else {
      w.unfullscreen();
    }
  }
}

/// 关闭指定弹窗（页面 `window.close()` 经 IPC 送达，见 [`popup_bind_script`]）。
///
/// 与 [`close_all_popups`] 同一 GTK 路径：先取窗口快照再 `gtk_window_close` → delete-event
/// 未挂处理器时由 GTK 默认处理器**同步** `gtk_widget_destroy` → 就绪的 `connect_destroy`
/// 摘除注册表项。先克隆窗口再离开借用作用域，避免持着 `POPUPS` 借用触发重入改表
///（与 [`set_fullscreen`] 同构）。
fn close_popup(handle: isize) {
  let window = POPUPS.with(|m| m.borrow().get(&handle).map(|p| p.window.clone()));
  if let Some(w) = window {
    w.close();
  }
}

/// 分发弹窗 ipc 消息（平台接缝项，见 `webview.rs` 的主窗处理器）。
///
/// 调用点特殊：消息来自**主窗**处理器——本平台弹窗文档承载在发起方 UCM 上，弹窗消息被
/// 路由到主窗（见 [`popup_bind_script`]），故由主窗按消息内嵌的 id 回投到发起窗口。`handle`
/// 指向已关闭或伪造的弹窗时各分支均为无操作（`POPUPS` 查不到即返回）。
pub(crate) fn popup_ipc(handle: isize, action: PopupIpc) {
  match action {
    PopupIpc::Close => close_popup(handle),
    PopupIpc::Fullscreen(on) => set_fullscreen(handle, on),
  }
}

fn clamp(v: i32, lo: i32, hi: i32) -> i32 {
  v.max(lo).min(hi)
}

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

  /// 与 Windows 侧 `is_home_accel_predicate_matrix` 同构：Alt+Home 判定矩阵。
  #[test]
  fn is_home_accel_predicate_matrix() {
    let home = gdk::keys::constants::Home;
    let end = gdk::keys::constants::End;
    let kp_home = gdk::keys::constants::KP_Home;
    let alt = gdk::ModifierType::MOD1_MASK;
    // 命中：Alt + 主键区 Home
    assert!(is_home_accel(home, alt));
    // 非 Home 键不命中（含小键盘 Home——与 Windows 仅认 VK_HOME 一致）
    assert!(!is_home_accel(end, alt));
    assert!(!is_home_accel(kp_home, alt));
    // 无 Alt 不命中；Ctrl / Shift 单独按下不命中（与 Windows 一致：只看 Alt）
    assert!(!is_home_accel(home, gdk::ModifierType::empty()));
    assert!(!is_home_accel(home, gdk::ModifierType::CONTROL_MASK));
    assert!(!is_home_accel(home, gdk::ModifierType::SHIFT_MASK));
    // Alt 叠加额外修饰仍命中（Windows 亦只看 Alt）
    assert!(is_home_accel(home, alt | gdk::ModifierType::SHIFT_MASK));
  }
}
