//! child/src/platform/windows.rs — Windows 平台外壳（child 侧）。
//!
//! 阶段 0a/0b 迁移：原 `tail_core` 的 OS 触碰项、原 `accelerator.rs`（宿主层按键）
//! 与原 `popup.rs` 的裸 Win32 窗口管线集中于此，使 `tail-core` 零 OS 依赖、
//! `popup.rs` 只留纯路由判定。**Windows 行为零变更**。Unix 同名实现见
//! `platform/linux.rs`。
//!
//! 接口命名对齐设计文档 §3.3（`install_keys` / `create_popup` / `navigate_popup` /
//! `close_all_popups` / `set_fullscreen` / `shell_open` / `fatal` /
//! `install_close_watchdog` / `data_dir`）。设计文档给出的 `hwnd: RawWindowHandleInfo`
//! 为示意类型，实际签名以本文件为准（接缝在 0b 显形，见 §7 T-0b-2）。

use crate::popup::{classify_new_window, is_http_url, NewWindowAction};
use crate::webview::ChildEvent;
use std::cell::RefCell;
use std::collections::HashMap;
use std::num::NonZeroIsize;
use std::path::PathBuf;
use std::rc::Rc;

use tao::event_loop::EventLoopProxy;
use webview2_com::{
  AcceleratorKeyPressedEventHandler,
  Microsoft::Web::WebView2::Win32::{
    COREWEBVIEW2_KEY_EVENT_KIND, COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN,
    COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN, COREWEBVIEW2_PHYSICAL_KEY_STATUS,
    ICoreWebView2AcceleratorKeyPressedEventArgs,
  },
};
use wry::raw_window_handle::{
  HandleError, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle,
};
use wry::{
  NewWindowFeatures, NewWindowResponse, PageLoadEvent, WebView, WebViewBuilderExtWindows,
  WebViewExtWindows,
};

// ============================================================================
// 数据目录（自 tail-core 外迁，见平台模块头）
// ============================================================================

/// `<数据基目录>/Web2App/<sub>`：品牌目录与子系统名的拼接收敛于此，
/// 保证 child（apps）落在 `%LOCALAPPDATA%\Web2App\` 下，不污染 exe 目录。
///
/// 探测链：%LOCALAPPDATA% → 兜底 temp。Windows 上 LOCALAPPDATA 几乎总存在；
/// 缺失（极端环境）回落 temp 保证不 panic。
pub(crate) fn data_dir(sub: &str) -> PathBuf {
  let base = std::env::var_os("LOCALAPPDATA")
    .map(PathBuf::from)
    .unwrap_or_else(std::env::temp_dir);
  base.join("Web2App").join(sub)
}

// ============================================================================
// 致命错误呈现（Windows GUI 子系统下 stderr 无人看，必须 MessageBox）
// ============================================================================

/// 致命错误的用户可见呈现后退出（返回 `!`）。
///
/// 动机：release 为 Windows GUI 子系统且 `panic = "abort"`，`eprintln` 无人看见、
/// panic 无输出——错误必须以用户可感知的方式呈现，否则双击即"静默死亡"。
/// Windows 用 MessageBoxW（`extern` 直接声明，零新依赖）。
pub(crate) fn fatal(msg: &str) -> ! {
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

// ============================================================================
// 关闭确定性兜底（WNDPROC 哨兵 + 看门狗线程）
// ============================================================================

/// 关闭看门狗：300ms 后若进程仍存活，以退出码 0 结束进程。
///
/// 与 tao 事件处理里的 `ControlFlow::Exit` 配对使用。实测（tao 0.37 +
/// wry 0.57/WebView2）：单条 WM_CLOSE 路径下（Alt+F4、任务栏「关闭窗口」、
/// taskkill 无 /F、UI 自动化），即使投递 WM_QUIT 也会被 Chromium 嵌套 COM
/// 消息泵在 tao 外层 GetMessage 之前吞掉——进程要等下一条消息才退出；
/// 鼠标点 X 因伴随消息风暴通常无感，但「关闭即退出」不能依赖运气。
///
/// 看门狗在独立线程睡眠，不参与消息泵竞争，给出 300ms 的确定退出上限。
/// 正常路径几乎总先于它结束（tao run() 末尾自身即 process::exit，本就不做
/// 栈展开），线程随进程消失，零副作用；仅在框架卡死时兜底。WebView2 按
/// 浏览器进程被杀的常态设计，硬退出不损坏数据目录。
///
/// 幂等：多次调用仅多 spawn 无害线程，exit 先到先得。配套：
/// [`install_close_watchdog`]——在 WNDPROC 层面保证本函数必然被武装。
pub(crate) fn quit_watchdog() {
  std::thread::spawn(|| {
    std::thread::sleep(std::time::Duration::from_millis(300));
    std::process::exit(0);
  });
}

// 已武装窗口的「原窗口过程」注册表（hwnd → prev WNDPROC）。
//
// 按 hwnd 分槽而非全局单值：进程内可能存在多个窗口（主窗口 + 新窗口路由
// 产生的弹窗），单槽会在第二次武装时覆盖前一个窗口的链尾，令其哨兵调用错
// 窗口过程。窗口消息只在创建窗口的线程派发，故 thread_local 足够，
// 无需全局锁与跨线程可见性。
thread_local! {
  static PREV_WNDPROCS: RefCell<HashMap<isize, isize>> = RefCell::new(HashMap::new());
}

/// 在窗口上安装关闭哨兵：替换窗口过程，捕获 WM_CLOSE 并武装 [`quit_watchdog()`]。
///
/// 动机（实测 tao 0.37 + wry 0.57/WebView2）：单条 WM_CLOSE 路径下
/// （Process.CloseMainWindow、Alt+F4、任务栏「关闭窗口」、taskkill 无 /F），
/// tao 的 CloseRequested 事件可能**永不派发**——消息被 WebView2 嵌套 COM
/// 消息泵吞没，进程 30s+ 不自退（本仓库 e2e 实测复现）。事件层无法自救，
/// 必须在消息进入框架之前拦截。
///
/// 本函数在 WNDPROC 层面捕获 WM_CLOSE——先于一切框架事件派发，不受消息泵
/// 吞没影响——收到即武装看门狗（300ms 确定退出），随后无条件
/// `CallWindowProcW` 透传给原过程，正常事件链完全不受影响。
///
/// 调用约束：
/// - 必须在**创建 WebView 之前**调用：wry 挂接其 subclass 时以当前 WNDPROC
///   为链尾，先挂哨兵可保证 wry 的 WM_SIZE/WM_SETFOCUS 等处理仍在链上。
/// - 须在窗口所属线程调用（child/master 均在主线程窗口创建后立即调用）。
///
/// 无竞态：挂接发生在事件循环启动前、hwnd 尚未暴露给任何其他线程，
/// 消息不可能在 store 原过程之前到达哨兵 proc。
///
/// 签名取 `&Window`（而非 §3.3 示意中的裸句柄）：调用点因此平台无关——
/// Unix 侧同名函数为 no-op，无需调用方 `#[cfg]`。hwnd 提取留在本平台模块内。
pub(crate) fn install_close_watchdog(window: &tao::window::Window) {
  use tao::platform::windows::WindowExtWindows;
  let handle = window.hwnd();
  #[link(name = "user32")]
  extern "system" {
    fn SetWindowLongPtrW(hwnd: isize, index: i32, new_value: isize) -> isize;
    fn CallWindowProcW(prev: isize, hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize;
    fn DefWindowProcW(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize;
  }
  const GWLP_WNDPROC: i32 = -4;
  const WM_CLOSE: u32 = 0x0010;

  unsafe extern "system" fn close_watch_proc(
    hwnd: isize,
    msg: u32,
    wparam: usize,
    lparam: isize,
  ) -> isize {
    if msg == WM_CLOSE {
      quit_watchdog();
    }
    let prev = PREV_WNDPROCS.with(|m| m.borrow().get(&hwnd).copied()).unwrap_or(0);
    if prev == 0 {
      // 未登记（武装失败或已被替换）：退化为默认处理，避免向空过程转发。
      return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    unsafe { CallWindowProcW(prev, hwnd, msg, wparam, lparam) }
  }

  // SAFETY: handle 为调用方持有的有效窗口句柄。SetWindowLongPtrW 原子换入
  // 哨兵过程并返回原过程；失败返回 0 时不保存（哨兵永不生效，行为等同
  // 未安装，退化为仅 tao handler 路径的看门狗）。
  let prev =
    unsafe { SetWindowLongPtrW(handle, GWLP_WNDPROC, close_watch_proc as *const () as isize) };
  if prev != 0 {
    PREV_WNDPROCS.with(|m| {
      m.borrow_mut().insert(handle, prev);
    });
  }
}

// ============================================================================
// 宿主层 Alt+Home 快捷键（WebView2 `AcceleratorKeyPressed`）
// ============================================================================
//
// 为什么需要宿主层：页面层 `keydown`（见 `webview::home_hotkey_script`）在**焦点位于
// 跨源 iframe 内**时收不到按键——按键被 iframe 的文档吞掉，托管页拿不到。WebView2 的
// `AcceleratorKeyPressed` 在**按键送达内容层之前**于宿主侧回调，因而能覆盖该边界。
//
// 分层职责：宿主层只做两件事——**判定**（`is_home_accel`）与**送达**（派发
// `ChildEvent::GoHome`）；「是否已在首页 / 在途守卫 / BFCache 回跳」等决策逻辑全部
// 留在页面层。宿主层**不**在 COM 回调内同步调用 `evaluate_script`（WebView2 回调
// 重入风险），一律经 `EventLoopProxy` 回流主循环执行。

/// VK_HOME（Home 键虚拟键码）。
const VK_HOME: u16 = 0x24;

/// 纯判定：是否为「Alt+Home 首次按下」。
///
/// - `kind`：`COREWEBVIEW2_KEY_EVENT_KIND` 原始值；仅接受 KEY_DOWN / SYSTEM_KEY_DOWN，
///   排除 KEY_UP / SYSTEM_KEY_UP（否则松开 Alt 也会触发）。
/// - `vk`：虚拟键码，须为 VK_HOME(0x24)。
/// - `was_down`：`PhysicalKeyStatus.WasKeyDown`（自动重复位）；为真表示长按重复，忽略。
/// - `alt`：`PhysicalKeyStatus.IsMenuKeyDown`（Alt 是否按下）。
pub(crate) fn is_home_accel(kind: i32, vk: u16, was_down: bool, alt: bool) -> bool {
  let is_down = kind == COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN.0
    || kind == COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN.0;
  is_down && vk == VK_HOME && !was_down && alt
}

/// 从事件参数读取按键状态并判定 Alt+Home；命中且 [`crate::webview::HOST_ONLY`] 时吞掉按键。
///
/// 任一 getter 失败即视为未命中（非阻塞、不回退既有行为）。
unsafe fn accel_hit(args: &ICoreWebView2AcceleratorKeyPressedEventArgs) -> bool {
  let mut kind = COREWEBVIEW2_KEY_EVENT_KIND(0);
  if args.KeyEventKind(&mut kind).is_err() {
    return false;
  }
  let mut vk: u32 = 0;
  if args.VirtualKey(&mut vk).is_err() {
    return false;
  }
  let mut status = COREWEBVIEW2_PHYSICAL_KEY_STATUS::default();
  if args.PhysicalKeyStatus(&mut status).is_err() {
    return false;
  }
  if !is_home_accel(kind.0, vk as u16, status.WasKeyDown.as_bool(), status.IsMenuKeyDown.as_bool()) {
    return false;
  }
  // 仅在真的派发一次回首页时（且 HOST_ONLY）才吞掉按键；裸 Home / 无 Alt / KeyUp 绝不设。
  if crate::webview::HOST_ONLY {
    let _ = args.SetHandled(true);
  }
  true
}

/// 在 WebView 的 controller 上注册 Alt+Home 处理器，命中时执行 `action`。
///
/// `action` 由调用方按窗口形态给出：主窗口回流 `ChildEvent::GoHome`（经 proxy →
/// 主循环 `evaluate_script`），弹窗 `PostMessageW(hwnd, WM_APP_HOME)` 在自己的
/// 窗口过程内执行。注册失败静默忽略——该能力是「额外送达」，失败不得回退既有行为。
pub(crate) fn register_home_accel(webview: &WebView, action: impl Fn() + 'static) {
  let handler = AcceleratorKeyPressedEventHandler::create(Box::new(move |_sender, args| {
    if let Some(args) = args {
      if unsafe { accel_hit(&args) } {
        action();
      }
    }
    Ok(())
  }));
  let mut token: i64 = 0;
  unsafe {
    let _ = webview.controller().add_AcceleratorKeyPressed(&handler, &mut token);
  }
}

/// 主窗口：命中 Alt+Home → 回流 `ChildEvent::GoHome`（宿主层按键机制入口，§3.3）。
pub(crate) fn install_keys(webview: &WebView, proxy: EventLoopProxy<ChildEvent>) {
  register_home_accel(webview, move || {
    let _ = proxy.send_event(ChildEvent::GoHome);
  });
}

// ============================================================================
// WebView 挂载（接缝项）
// ============================================================================

/// 把构建好的 WebView 挂到窗口上：Windows **直通** wry 的通用 `build(window)`。
///
/// 直通即等价于接线前的 `builder.build(window)`——Windows 行为零变更。之所以抽出
/// 同名接缝项：Linux 侧**不能**走这条通用路径（wry 的 `webkitgtk` 后端通用 `build`
/// 只接受 Xlib 句柄，Wayland 会话下直接失败），必须改用 `build_gtk` 挂到 GTK 容器，
/// 见 `platform/linux.rs` 同名项文档。两份实现签名一致 → 调用点（`webview.rs`）
/// 平台无关（§3.4：平台判断只在 `platform/`）。
pub(crate) fn mount_webview<'a>(
  builder: wry::WebViewBuilder<'a>,
  window: &'a tao::window::Window,
) -> Result<WebView, wry::Error> {
  builder.build(window)
}

// ============================================================================
// External：交给系统默认程序
// ============================================================================

/// 用系统默认程序打开 URL（`ShellExecuteW`，零新依赖的裸 Win32 调用）。
///
/// 真实协议（`http(s)` / `mailto:` / 自定义协议…）走这里——交给系统协议处理器
/// 是浏览器的标准行为，比自己实现更正确。
pub(crate) fn shell_open(url: &str) {
  #[link(name = "shell32")]
  extern "system" {
    fn ShellExecuteW(
      hwnd: isize,
      op: *const u16,
      file: *const u16,
      params: *const u16,
      dir: *const u16,
      show: i32,
    ) -> isize;
  }
  const SW_SHOWNORMAL: i32 = 1;
  let op = wide("open");
  let file = wide(url);
  // SAFETY: 三个字符串参数均为本调用期间存活的 NUL 结尾宽串；hwnd=0（无属主）合法。
  unsafe {
    ShellExecuteW(
      0,
      op.as_ptr(),
      file.as_ptr(),
      std::ptr::null(),
      std::ptr::null(),
      SW_SHOWNORMAL,
    );
  }
}

/// 建窗失败时的降级：仅对 http(s) 交系统打开。
///
/// 空白窗（`about:blank`）外发只会让系统浏览器弹出一张空白页，故静默放弃。
pub(crate) fn open_external_if_http(url: &str) {
  if is_http_url(url) {
    shell_open(url);
  }
}

// ============================================================================
// Popup：独立原生窗口 + 共享环境的 WebView
// ============================================================================
//
// 为何用裸 Win32 而非 tao 建弹窗：tao 的 `EventLoop::run` 消耗 `self`，而 wry 的
// 新窗口回调是 `'static` 的，拿不到 `&EventLoop` 就无法调用 `WindowBuilder::build`。
// 故此处用 `CreateWindowExW` 建顶层窗口，并经 `wry::raw_window_handle` 零依赖地
// 实现 `HasWindowHandle` 交给 wry 挂 WebView（wry 0.57 的
// `WebViewBuilder::build<W: HasWindowHandle>` 不限于 tao 窗口）。
//
// WebView2 契约（Microsoft `put_NewWindow` 文档）：提供的 CoreWebView2
// 「不得被提前导航过」；设置后由 WebView2 自行「替换并导航」到目标 Uri。故建窗时
// **不**调用 `with_url`（否则既违反契约又造成双重加载）。同时新 WebView 必须与发起方
// **同 Environment**（同一 profile），故沿用 `features.opener.environment`。
//
// 窗口过程只自理 WM_CLOSE（毁窗）与 WM_DESTROY（释放 WebView）：wry 在 build 时
// 已对本窗口 `SetWindowSubclass` 接管 WM_SIZE / WM_SETFOCUS / WM_ENTERSIZEMOVE /
// WM_MOVE / WM_MOVING 与自身的 WM_DESTROY 清理（见 wry webview2/mod.rs
// `parent_subclass_proc`），故无需自写尺寸同步。

thread_local! {
  /// 存活弹窗的 WebView（key = 顶层窗口 HWND）。
  ///
  /// 必须持有：`NewWindowResponse::Create` 交给 WebView2 的 `ICoreWebView2` 归该
  /// WebView 所有，提前 drop 会让新窗口失去宿主。`WebView` 非 `Send`，故用
  /// thread_local（窗口消息与 wry 回调都在建窗线程上执行）。
  static POPUPS: RefCell<HashMap<isize, WebView>> = RefCell::new(HashMap::new());

  /// 进入全屏前的窗口样式与矩形（key = HWND），供退出全屏时还原。
  static PRE_FULLSCREEN: RefCell<HashMap<isize, (i32, Rect)>> = RefCell::new(HashMap::new());
}

/// 为 `Popup` 分支建独立窗口并挂 WebView，返回交给 WebView2 的 `Create` 响应。
///
/// 任一步失败返回 `None`——调用方按 [`open_external_if_http`] 降级，不 panic
/// （release 为 GUI 子系统 + `panic = "abort"`，panic 即静默死亡）。
///
/// `features` 由 wry 提供：`opener.environment` 是必须沿用的环境
/// （WebView2 契约：目标 WebView 必须与发起方同环境，否则 `SetNewWindow` 失败），
/// `size` / `position` 是页面请求的窗口特征。`home` 是应用起始页，注入给弹窗的
/// Alt+Home 回首页脚本（弹窗与主窗口共用同一首页）。
pub(crate) fn create_popup(
  url: &str,
  features: NewWindowFeatures,
  home: &str,
) -> Option<NewWindowResponse> {
  // 尺寸/位置：页面给了就用（缺省给一个可用的默认值）；下限钳制避免站点用
  // 1x1 之类的隐形窗制造用户既看不见也关不掉的窗口。
  let (w, h) = match features.size.as_ref() {
    Some(s) => (clamp(s.width.round() as i32, 320, 10000), clamp(s.height.round() as i32, 240, 10000)),
    None => (960, 720),
  };
  let (x, y) = match features.position.as_ref() {
    Some(p) => (p.x.round() as i32, p.y.round() as i32),
    None => (CW_USEDEFAULT, CW_USEDEFAULT),
  };
  let env = features.opener.environment;

  // 初始标题：空白窗用品牌名（避免窗口标题显示 "about:blank"），其余用 host。
  let title_src = if crate::popup::is_blank(url) { "" } else { url };
  let hwnd = create_window(title_src, x, y, w, h)?;
  let raw = RawWindow(hwnd);
  // 弹窗自身的同站判定基准：随其导航更新（on_page_load 回填）。
  let host = Rc::new(RefCell::new(tail_core::host_of(url)));
  // 嵌套弹窗也要能回首页：回调是 'static 的，故取所有权。
  let script = crate::webview::inject_script(home);
  let home_for_nested = home.to_string();

  let host_for_load = Rc::clone(&host);
  let mut builder = wry::WebViewBuilder::new()
    // 刻意不 set_url：WebView2 会在 SetNewWindow 后自行导航（见模块头契约说明）。
    .with_environment(env)
    .with_hotkeys_zoom(true)
    .with_back_forward_navigation_gestures(true)
    .with_clipboard(true)
    .with_initialization_script(script)
    .with_ipc_handler(move |req| {
      // HTML5 全屏：弹窗无 tao 事件循环，直接在 UI 线程上做 Win32 全屏切换。
      if let Some(flag) = req.into_body().strip_prefix("w2a:fullscreen:") {
        set_fullscreen(hwnd, flag == "1");
      }
    })
    .with_document_title_changed_handler(move |title| {
      let t = wide(&title);
      // SAFETY: hwnd 属本线程；t 为存活的 NUL 结尾宽串。
      unsafe { SetWindowTextW(hwnd, t.as_ptr()) };
    })
    .with_on_page_load_handler(move |event, loaded| {
      if matches!(event, PageLoadEvent::Finished) {
        *host_for_load.borrow_mut() = tail_core::host_of(&loaded);
      }
    })
    // 弹窗内部再发起的新窗口请求走同一套路由（同站就地导航、跨站交系统、嵌套受限）。
    .with_new_window_req_handler(move |target, f| {
      let has_popup = f.size.is_some() || f.position.is_some();
      let opener = host.borrow().clone();
      match classify_new_window(&target, &opener, has_popup) {
        NewWindowAction::SameTab => {
          navigate_popup(hwnd, &target);
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
    });
  if cfg!(debug_assertions) {
    builder = builder.with_devtools(true);
  }

  let webview = match builder.build(&raw) {
    Ok(v) => v,
    Err(_) => {
      // SAFETY: hwnd 本函数刚创建且未交给任何其他持有者，可安全销毁。
      unsafe { DestroyWindow(hwnd) };
      return None;
    }
  };
  let core = webview.webview();
  // SAFETY: hwnd 有效且属于本线程。
  unsafe { ShowWindow(hwnd, SW_SHOW) };
  // 宿主层 Alt+Home（弹窗路径）：弹窗无 tao 事件循环，命中后 PostMessage 到本窗口，
  // 由 popup_wnd_proc 的 WM_APP_HOME 分支取 POPUPS[hwnd] 执行回首页脚本
  // （wry 的 subclass 未处理的消息会经 DefSubclassProc 回到我们的窗口过程）。
  register_home_accel(&webview, move || {
    // SAFETY: hwnd 为刚创建的有效顶层窗口；PostMessageW 本身线程安全。
    unsafe { PostMessageW(hwnd, WM_APP_HOME, 0, 0) };
  });
  POPUPS.with(|m| {
    m.borrow_mut().insert(hwnd, webview);
  });
  Some(NewWindowResponse::Create { webview: core })
}

/// 让指定弹窗就地导航（其内部发起的站内新窗口请求）。
pub(crate) fn navigate_popup(hwnd: isize, url: &str) {
  POPUPS.with(|m| {
    if let Some(wv) = m.borrow().get(&hwnd) {
      let _ = wv.load_url(url);
    }
  });
}

/// 关闭全部弹窗（主窗口关闭前调用，保证窗口 / WebView2 子进程零残留）。
pub(crate) fn close_all_popups() {
  // 先取出句柄快照：DestroyWindow 会同步触发 WM_DESTROY → 回调里改 POPUPS，
  // 持着借用边遍历边改会 panic。
  let hwnds: Vec<isize> = POPUPS.with(|m| m.borrow().keys().copied().collect());
  for h in hwnds {
    // SAFETY: 句柄来自 POPUPS（均为有效的顶层窗口）；调用发生在建窗线程。
    unsafe { DestroyWindow(h) };
  }
}

/// 弹窗全屏切换（无边框覆盖所在显示器），退出时还原原样式与位置。
///
/// 仅由弹窗的 ipc 回调在 UI 线程调用；重复进入全屏幂等（已在全屏则直接返回）。
pub(crate) fn set_fullscreen(hwnd: isize, on: bool) {
  if on {
    if PRE_FULLSCREEN.with(|m| m.borrow().contains_key(&hwnd)) {
      return;
    }
    // SAFETY: hwnd 是有效顶层窗口，均为只读取值。
    let style = unsafe { GetWindowLongW(hwnd, GWL_STYLE) };
    let mut rect = Rect::default();
    // SAFETY: rect 为本栈变量，写入由系统填充。
    let has_rect = unsafe { GetWindowRect(hwnd, &mut rect) } != 0;
    let mut mi = MonitorInfo { cb_size: std::mem::size_of::<MonitorInfo>() as u32, ..Default::default() };
    // SAFETY: mi 由系统填充；flags 合法。
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    let has_mon = unsafe { GetMonitorInfoW(monitor, &mut mi) } != 0;
    PRE_FULLSCREEN.with(|m| m.borrow_mut().insert(hwnd, (style, rect)));
    if has_mon && has_rect {
      let r = mi.rc_monitor;
      // 去掉 WS_OVERLAPPEDWINDOW（标题栏/边框/系统菜单等）→ 无边框覆盖整屏。
      // SAFETY: hwnd 有效；样式位运算合法。
      unsafe { SetWindowLongW(hwnd, GWL_STYLE, style & !(WS_OVERLAPPEDWINDOW)) };
      // SAFETY: 目标矩形来自显示器信息；flags 合法。
      unsafe {
        SetWindowPos(
          hwnd,
          0,
          r.left,
          r.top,
          r.right - r.left,
          r.bottom - r.top,
          SWP_NOZORDER | SWP_FRAMECHANGED | SWP_SHOWWINDOW,
        );
      }
    }
  } else {
    let prev = PRE_FULLSCREEN.with(|m| m.borrow_mut().remove(&hwnd));
    if let Some((style, r)) = prev {
      // SAFETY: hwnd 有效；还原为进入前的样式与矩形。
      unsafe { SetWindowLongW(hwnd, GWL_STYLE, style) };
      // SAFETY: 同上。
      unsafe {
        SetWindowPos(
          hwnd,
          0,
          r.left,
          r.top,
          r.right - r.left,
          r.bottom - r.top,
          SWP_NOZORDER | SWP_FRAMECHANGED | SWP_SHOWWINDOW,
        );
      }
    }
  }
}

// ---------------------------------------------------------------------------
// 窗口过程与句柄桥
// ---------------------------------------------------------------------------

/// 把 HWND 包成 `HasWindowHandle` 交给 wry（wry 只取 `RawWindowHandle::Win32.hwnd`）。
struct RawWindow(isize);

impl HasWindowHandle for RawWindow {
  fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
    let hwnd = NonZeroIsize::new(self.0).ok_or(HandleError::Unavailable)?;
    let mut handle = Win32WindowHandle::new(hwnd);
    handle.hinstance = NonZeroIsize::new(module_handle());
    // SAFETY: 句柄由 create_window 创建，wry 在 build 期间同步取用，此刻必然有效；
    // 返回值借用 self，生命周期与句柄有效性一致。
    Ok(unsafe { WindowHandle::borrow_raw(RawWindowHandle::Win32(handle)) })
  }
}

/// 弹窗窗口过程：只处理 WM_CLOSE 与 WM_DESTROY，其余交系统默认处理。
unsafe extern "system" fn popup_wnd_proc(
  hwnd: isize,
  msg: u32,
  wparam: usize,
  lparam: isize,
) -> isize {
  match msg {
    WM_CLOSE => {
      // 同步毁窗：不依赖消息泵，弹窗关闭是确定性的。
      DestroyWindow(hwnd);
      0
    }
    WM_DESTROY => {
      // wry 的 parent subclass 已在链上先完成自身清理，此处只需释放我们持有的
      // WebView（放到临时变量，避免在 POPUPS 借用作用域内 drop）。
      let owned = POPUPS.with(|m| m.borrow_mut().remove(&hwnd));
      PRE_FULLSCREEN.with(|m| m.borrow_mut().remove(&hwnd));
      drop(owned);
      0
    }
    WM_APP_HOME => {
      // 宿主层 Alt+Home 送达（见 create_popup 内的注册）：取本窗 WebView 执行页面
      // 暴露的 `window.__w2aHome`。仅调用，不做判定——决策逻辑单点在页面层。
      POPUPS.with(|m| {
        if let Some(wv) = m.borrow().get(&hwnd) {
          let _ = wv.evaluate_script(crate::webview::HOME_GO_SCRIPT);
        }
      });
      0
    }
    _ => DefWindowProcW(hwnd, msg, wparam, lparam),
  }
}

/// 建顶层弹窗窗口（不可见，待 WebView 挂好后再 ShowWindow，避免白闪）。
fn create_window(title_src: &str, x: i32, y: i32, w: i32, h: i32) -> Option<isize> {
  let instance = module_handle();
  if instance == 0 {
    return None;
  }
  let class = wide(CLASS_NAME);
  let wc = WndClassExW {
    cb_size: std::mem::size_of::<WndClassExW>() as u32,
    style: CS_HREDRAW | CS_VREDRAW,
    lpfn_wnd_proc: Some(popup_wnd_proc),
    cb_cls_extra: 0,
    cb_wnd_extra: 0,
    h_instance: instance,
    h_icon: 0,
    // SAFETY: IDC_ARROW 是系统预定义光标资源（MAKEINTRESOURCE 语义），恒有效。
    h_cursor: unsafe { LoadCursorW(0, IDC_ARROW as *const u16) },
    h_br_background: COLOR_WINDOW + 1,
    lpsz_menu_name: std::ptr::null(),
    lpsz_class_name: class.as_ptr(),
    h_icon_sm: 0,
  };
  // SAFETY: wc 字段齐备、class 名 NUL 结尾且在本调用期间存活；重复注册返回 0
  //（ERROR_CLASS_ALREADY_EXISTS），无害：类已存在即为所需状态。
  unsafe { RegisterClassExW(&wc) };

  let title = wide(&tail_core::default_title(title_src));
  // SAFETY: class/title 为存活的 NUL 结尾宽串；hinstance 有效；parent/menu 传 0
  // 表示无属主顶层窗口；先不带 WS_VISIBLE，待 WebView 挂好后再 ShowWindow。
  let hwnd = unsafe {
    CreateWindowExW(
      0,
      class.as_ptr(),
      title.as_ptr(),
      WS_OVERLAPPEDWINDOW as u32,
      x,
      y,
      w,
      h,
      0,
      0,
      instance,
      0,
    )
  };
  if hwnd == 0 {
    None
  } else {
    Some(hwnd)
  }
}

fn module_handle() -> isize {
  // SAFETY: 取本进程模块句柄（参数为 NULL），无副作用。
  unsafe { GetModuleHandleW(std::ptr::null()) }
}

fn clamp(v: i32, lo: i32, hi: i32) -> i32 {
  v.max(lo).min(hi)
}

/// Rust 字符串 → NUL 结尾宽串（Win32 W 系列 API 的入参形式）。
fn wide(s: &str) -> Vec<u16> {
  s.encode_utf16().chain(std::iter::once(0)).collect()
}

// ---------------------------------------------------------------------------
// Win32 绑定（裸 extern，零新增 crate）
// ---------------------------------------------------------------------------

type WndProc = unsafe extern "system" fn(isize, u32, usize, isize) -> isize;

const CLASS_NAME: &str = "Web2AppPopupWindow";
const WS_OVERLAPPEDWINDOW: i32 = 0x00CF_0000;
const CS_HREDRAW: u32 = 0x0002;
const CS_VREDRAW: u32 = 0x0001;
const CW_USEDEFAULT: i32 = 0x8000_0000u32 as i32;
const SW_SHOW: i32 = 5;
const WM_CLOSE: u32 = 0x0010;
const WM_DESTROY: u32 = 0x0002;
/// 宿主层 Alt+Home 的自定义消息（WM_APP+1）：弹窗路径经 PostMessage 送达。
const WM_APP_HOME: u32 = 0x8001;
const COLOR_WINDOW: isize = 5;
const IDC_ARROW: usize = 32512;
const GWL_STYLE: i32 = -16;
const MONITOR_DEFAULTTONEAREST: u32 = 2;
const SWP_NOZORDER: u32 = 0x0004;
const SWP_FRAMECHANGED: u32 = 0x0020;
const SWP_SHOWWINDOW: u32 = 0x0040;

#[repr(C)]
struct WndClassExW {
  cb_size: u32,
  style: u32,
  lpfn_wnd_proc: Option<WndProc>,
  cb_cls_extra: i32,
  cb_wnd_extra: i32,
  h_instance: isize,
  h_icon: isize,
  h_cursor: isize,
  h_br_background: isize,
  lpsz_menu_name: *const u16,
  lpsz_class_name: *const u16,
  h_icon_sm: isize,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Rect {
  left: i32,
  top: i32,
  right: i32,
  bottom: i32,
}

#[repr(C)]
#[derive(Default)]
struct MonitorInfo {
  cb_size: u32,
  rc_monitor: Rect,
  rc_work: Rect,
  dw_flags: u32,
}

#[link(name = "kernel32")]
extern "system" {
  fn GetModuleHandleW(name: *const u16) -> isize;
}

#[link(name = "user32")]
extern "system" {
  fn RegisterClassExW(class: *const WndClassExW) -> u16;
  fn CreateWindowExW(
    ex_style: u32,
    class: *const u16,
    title: *const u16,
    style: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    parent: isize,
    menu: isize,
    instance: isize,
    param: isize,
  ) -> isize;
  fn DefWindowProcW(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize;
  fn DestroyWindow(hwnd: isize) -> i32;
  fn ShowWindow(hwnd: isize, cmd: i32) -> i32;
  fn LoadCursorW(instance: isize, name: *const u16) -> isize;
  fn SetWindowTextW(hwnd: isize, text: *const u16) -> i32;
  fn GetWindowLongW(hwnd: isize, index: i32) -> i32;
  fn SetWindowLongW(hwnd: isize, index: i32, value: i32) -> i32;
  fn GetWindowRect(hwnd: isize, rect: *mut Rect) -> i32;
  fn SetWindowPos(hwnd: isize, after: isize, x: i32, y: i32, cx: i32, cy: i32, flags: u32) -> i32;
  fn MonitorFromWindow(hwnd: isize, flags: u32) -> isize;
  fn GetMonitorInfoW(monitor: isize, info: *mut MonitorInfo) -> i32;
  fn PostMessageW(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> i32;
}

#[cfg(test)]
mod tests {
  use super::*;
  use webview2_com::Microsoft::Web::WebView2::Win32::{
    COREWEBVIEW2_KEY_EVENT_KIND_KEY_UP, COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_UP,
  };

  #[test]
  fn data_dir_lives_under_brand_and_is_absolute() {
    // 探测链不 panic 且返回绝对路径（LOCALAPPDATA 或 temp 均为绝对路径）
    let s = data_dir("apps").to_string_lossy().replace('\\', "/");
    assert!(s.ends_with("/Web2App/apps"), "got: {s}");
    assert!(data_dir("apps").is_absolute());
  }

  #[test]
  fn is_home_accel_predicate_matrix() {
    let down = COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN.0;
    let sysdown = COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN.0;
    let up = COREWEBVIEW2_KEY_EVENT_KIND_KEY_UP.0;
    let sysup = COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_UP.0;
    const HOME: u16 = VK_HOME;

    // 命中：两种 keydown 形态 + Home + 非重复 + Alt
    assert!(is_home_accel(down, HOME, false, true));
    assert!(is_home_accel(sysdown, HOME, false, true));
    // 不命中：keyup 系列（松开 Alt 不该触发）
    assert!(!is_home_accel(up, HOME, false, true));
    assert!(!is_home_accel(sysup, HOME, false, true));
    // 不命中：自动重复（WasKeyDown）
    assert!(!is_home_accel(down, HOME, true, true));
    assert!(!is_home_accel(sysdown, HOME, true, true));
    // 不命中：无 Alt
    assert!(!is_home_accel(down, HOME, false, false));
    assert!(!is_home_accel(sysdown, HOME, false, false));
    // 不命中：非 Home 键
    assert!(!is_home_accel(down, 0x23, false, true)); // VK_END
    assert!(!is_home_accel(down, 0x00, false, true));
  }
}
