//! child/src/popup.rs — 新窗口请求（target=_blank / window.open）的路由与「弹窗」窗口。
//!
//! 背景：wry 默认未注册新窗口处理器，WebView2 会把新窗口请求直接吞掉——这正是
//! 部分站点「链接点了没反应」的根因。本模块给出唯一的出口决策（`classify_new_window`）
//! 与各分支的实现：就地导航（SameTab，由 webview.rs 主循环执行）、系统默认程序
//! （External，`shell_open`）、独立窗口（Popup，`create`）、不处理（Ignore）。
//!
//! 为何用裸 Win32 而非 tao 建弹窗：tao 的 `EventLoop::run` 消耗 `self`，而 wry 的
//! 新窗口回调是 `'static` 的，拿不到 `&EventLoop` 就无法调用 `WindowBuilder::build`。
//! 故此处用 `CreateWindowExW` 建顶层窗口，并经 `wry::raw_window_handle` 零依赖地
//! 实现 `HasWindowHandle` 交给 wry 挂 WebView（wry 0.57 的
//! `WebViewBuilder::build<W: HasWindowHandle>` 不限于 tao 窗口）。
//!
//! WebView2 契约（Microsoft `put_NewWindow` 文档）：提供的 CoreWebView2
//! 「不得被提前导航过」；设置后由 WebView2 自行「替换并导航」到目标 Uri。故建窗时
//! **不**调用 `with_url`（否则既违反契约又造成双重加载）。同时新 WebView 必须与发起方
//! **同 Environment**（同一 profile），故沿用 `features.opener.environment`。
//!
//! 窗口过程只自理 WM_CLOSE（毁窗）与 WM_DESTROY（释放 WebView）：wry 在 build 时
//! 已对本窗口 `SetWindowSubclass` 接管 WM_SIZE / WM_SETFOCUS / WM_ENTERSIZEMOVE /
//! WM_MOVE / WM_MOVING 与自身的 WM_DESTROY 清理（见 wry webview2/mod.rs
//! `parent_subclass_proc`），故无需自写尺寸同步。

use std::cell::RefCell;
use std::collections::HashMap;
use std::num::NonZeroIsize;
use std::rc::Rc;

use crate::webview::inject_script;
use wry::raw_window_handle::{
  HandleError, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle,
};
use wry::{
  NewWindowFeatures, NewWindowResponse, PageLoadEvent, WebView, WebViewBuilderExtWindows,
  WebViewExtWindows,
};

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

// ============================================================================
// 路由决策（纯函数，规则集中在此，便于单测穷举）
// ============================================================================

/// 新窗口请求的去向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewWindowAction {
  /// 站内新窗口 → 在承接窗口内就地导航（本应用不提供多标签）。
  SameTab,
  /// 交给系统默认程序打开（跨站外链 / mailto: 等真实可外发协议）。
  External,
  /// 建独立窗口（空白窗或 `window.open` 带尺寸/位置特征，如 OAuth 授权窗）。
  Popup,
  /// 不处理（浏览器内部伪协议，外发无处理器、内载不可靠）。
  Ignore,
}

/// 判定新窗口请求的去向。规则自上而下命中即返回：
///
/// 1. 空白窗（`window.open()` 无 URL → `about:blank`）→ [`Popup`]：脚本要的是一个
///    「稍后自己导航」的窗口，必须在应用内建窗——外发只会让系统浏览器弹一张空白页；
/// 2. http(s) 带弹窗特征 → [`Popup`]；否则同站 → [`SameTab`]、跨站 → [`External`]；
/// 3. 浏览器内部伪协议（`blob:` / `data:` / `javascript:` / `filesystem:` / 非 blank 的
///    `about:`）→ [`Ignore`]：既无系统处理器可外发，也无法在应用内可靠承载；
/// 4. 其余真实可外发协议（`mailto:` / `tel:` / 自定义协议…）→ [`External`]。
///
/// [`External`]: NewWindowAction::External
/// [`Popup`]: NewWindowAction::Popup
/// [`SameTab`]: NewWindowAction::SameTab
/// [`Ignore`]: NewWindowAction::Ignore
pub fn classify_new_window(url: &str, opener_host: &str, has_popup_features: bool) -> NewWindowAction {
  if is_blank(url) {
    return NewWindowAction::Popup;
  }
  if is_http_url(url) {
    if has_popup_features {
      return NewWindowAction::Popup;
    }
    return if same_site(&tail_core::host_of(url), opener_host) {
      NewWindowAction::SameTab
    } else {
      NewWindowAction::External
    };
  }
  if is_internal_pseudo(url) {
    return NewWindowAction::Ignore;
  }
  NewWindowAction::External
}

/// 空白窗判定：空串或 `about:blank`（大小写不敏感）。
fn is_blank(url: &str) -> bool {
  let t = url.trim();
  t.is_empty() || t.eq_ignore_ascii_case("about:blank")
}

/// scheme 是否为 http/https（大小写不敏感）。
pub fn is_http_url(url: &str) -> bool {
  match url.trim_start().split_once("://") {
    Some((scheme, _)) => scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"),
    None => false,
  }
}

/// 是否为浏览器内部伪协议：这类 URL 交给 ShellExecute 只会静默失败或乱开，
/// 也不宜在应用内导航（现代浏览器对顶层 `data:`/`blob:` 导航亦大多拦截）。
fn is_internal_pseudo(url: &str) -> bool {
  let scheme = url.trim_start().split_once(':').map(|(s, _)| s).unwrap_or("");
  ["about", "data", "blob", "javascript", "filesystem"]
    .iter()
    .any(|s| scheme.eq_ignore_ascii_case(s))
}

/// 同站判定：host 完全相同，或一方是另一方的子域（`www.a.com` ≡ `a.com`）。
/// 用「点 + 后缀」而非裸后缀匹配，避免 `evila.com` 被误判为 `a.com` 的子域。
fn same_site(a: &str, b: &str) -> bool {
  if a.is_empty() || b.is_empty() {
    return false;
  }
  a == b || a.ends_with(&format!(".{b}")) || b.ends_with(&format!(".{a}"))
}

// ============================================================================
// External：交给系统默认程序
// ============================================================================

/// 用系统默认程序打开 URL（`ShellExecuteW`，零新依赖的裸 Win32 调用）。
///
/// 真实协议（`http(s)` / `mailto:` / 自定义协议…）走这里——交给系统协议处理器
/// 是浏览器的标准行为，比自己实现更正确。
pub fn shell_open(url: &str) {
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
pub fn open_external_if_http(url: &str) {
  if is_http_url(url) {
    shell_open(url);
  }
}

// ============================================================================
// Popup：独立原生窗口 + 共享环境的 WebView
// ============================================================================

/// 为 `Popup` 分支建独立窗口并挂 WebView，返回交给 WebView2 的 `Create` 响应。
///
/// 任一步失败返回 `None`——调用方按 [`open_external_if_http`] 降级，不 panic
/// （release 为 GUI 子系统 + `panic = "abort"`，panic 即静默死亡）。
///
/// `features` 由 wry 提供：`opener.environment` 是必须沿用的环境
/// （WebView2 契约：目标 WebView 必须与发起方同环境，否则 `SetNewWindow` 失败），
/// `size` / `position` 是页面请求的窗口特征。`home` 是应用起始页，注入给弹窗的
/// Alt+Home 回首页脚本（弹窗与主窗口共用同一首页）。
pub fn create(url: &str, features: NewWindowFeatures, home: &str) -> Option<NewWindowResponse> {
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
  let title_src = if is_blank(url) { "" } else { url };
  let hwnd = create_window(title_src, x, y, w, h)?;
  let raw = RawWindow(hwnd);
  // 弹窗自身的同站判定基准：随其导航更新（on_page_load 回填）。
  let host = Rc::new(RefCell::new(tail_core::host_of(url)));
  // 嵌套弹窗也要能回首页：回调是 'static 的，故取所有权。
  let script = inject_script(home);
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
          navigate(hwnd, &target);
          NewWindowResponse::Deny
        }
        NewWindowAction::External => {
          shell_open(&target);
          NewWindowResponse::Deny
        }
        NewWindowAction::Popup => match create(&target, f, &home_for_nested) {
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
  POPUPS.with(|m| {
    m.borrow_mut().insert(hwnd, webview);
  });
  Some(NewWindowResponse::Create { webview: core })
}

/// 让指定弹窗就地导航（其内部发起的站内新窗口请求）。
pub fn navigate(hwnd: isize, url: &str) {
  POPUPS.with(|m| {
    if let Some(wv) = m.borrow().get(&hwnd) {
      let _ = wv.load_url(url);
    }
  });
}

/// 关闭全部弹窗（主窗口关闭前调用，保证窗口 / WebView2 子进程零残留）。
pub fn close_all() {
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
fn set_fullscreen(hwnd: isize, on: bool) {
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
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn classify_routes_http_by_site_and_popup_features() {
    let opener = "www.example.com";
    // 站内（同 host 或互为子域）普通链接 → 就地导航
    assert_eq!(
      classify_new_window("https://www.example.com/a", opener, false),
      NewWindowAction::SameTab
    );
    assert_eq!(
      classify_new_window("https://example.com/b", opener, false),
      NewWindowAction::SameTab
    );
    assert_eq!(
      classify_new_window("https://cdn.example.com/c", "example.com", false),
      NewWindowAction::SameTab
    );
    assert_eq!(
      classify_new_window("https://example.com/d", "cdn.example.com", false),
      NewWindowAction::SameTab
    );
    // 跨站普通链接 → 系统默认程序
    assert_eq!(
      classify_new_window("https://other.com/x", opener, false),
      NewWindowAction::External
    );
    // 前缀相似 ≠ 子域
    assert_eq!(
      classify_new_window("https://evilexample.com/x", "example.com", false),
      NewWindowAction::External
    );
    assert_eq!(
      classify_new_window("https://example.com.evil.com/x", "example.com", false),
      NewWindowAction::External
    );
    // 带 popup 特征 → 独立窗口（即便同站）
    assert_eq!(
      classify_new_window("https://www.example.com/login", opener, true),
      NewWindowAction::Popup
    );
    // 跨站 + popup 特征 → 独立窗口（不必外发）
    assert_eq!(
      classify_new_window("https://other.com/oauth", opener, true),
      NewWindowAction::Popup
    );
  }

  #[test]
  fn classify_keeps_blank_windows_in_app() {
    // 空白窗必须在应用内建窗，否则系统浏览器只弹一张空白页
    assert_eq!(
      classify_new_window("about:blank", "example.com", false),
      NewWindowAction::Popup
    );
    assert_eq!(classify_new_window("", "example.com", true), NewWindowAction::Popup);
    assert_eq!(
      classify_new_window("ABOUT:BLANK", "example.com", false),
      NewWindowAction::Popup
    );
  }

  #[test]
  fn classify_hands_real_schemes_to_system() {
    assert_eq!(
      classify_new_window("mailto:a@b.com", "example.com", false),
      NewWindowAction::External
    );
    assert_eq!(
      classify_new_window("tel:+8613800000000", "example.com", false),
      NewWindowAction::External
    );
    // 自定义协议即使带 popup 特征也交系统（不建窗）
    assert_eq!(
      classify_new_window("myapp://x", "example.com", true),
      NewWindowAction::External
    );
  }

  #[test]
  fn classify_ignores_browser_internal_pseudo_schemes() {
    for u in [
      "blob:https://example.com/1a2b",
      "data:text/html,<b>hi</b>",
      "javascript:alert(1)",
      "filesystem:https://example.com/y",
      "about:config",
    ] {
      assert_eq!(
        classify_new_window(u, "example.com", false),
        NewWindowAction::Ignore,
        "{u}"
      );
      assert_eq!(
        classify_new_window(u, "example.com", true),
        NewWindowAction::Ignore,
        "{u}"
      );
    }
  }

  #[test]
  fn same_site_requires_dot_boundary() {
    assert!(same_site("a.b.com", "b.com"));
    assert!(same_site("b.com", "b.com"));
    assert!(!same_site("ab.com", "b.com"));
    assert!(!same_site("b.com.evil.com", "b.com"));
    assert!(!same_site("", "b.com"));
    assert!(!same_site("b.com", ""));
  }
}
