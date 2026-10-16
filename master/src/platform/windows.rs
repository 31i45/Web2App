//! master/src/platform/windows.rs — Windows 平台外壳（master 侧）。
//!
//! 阶段 0a 机械迁移：原 `tail_core` 中的 OS 触碰项集中于此。**零行为变更**。
//! master 与 child 是独立 crate（child 为二进制、无 lib 导出），故同名外壳各自
//! 持有——与 `icon.rs` 的既有做法一致。Unix 同名实现见 `platform/linux.rs`。

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;

/// `<数据基目录>/Web2App/<sub>`：品牌目录与子系统名的拼接收敛于此，
/// 保证 master（master）落在 `%LOCALAPPDATA%\Web2App\` 下，不污染 exe 目录。
///
/// 探测链：%LOCALAPPDATA% → 兜底 temp。Windows 上 LOCALAPPDATA 几乎总存在；
/// 缺失（极端环境）回落 temp 保证不 panic。
pub(crate) fn data_dir(sub: &str) -> PathBuf {
  let base = std::env::var_os("LOCALAPPDATA")
    .map(PathBuf::from)
    .unwrap_or_else(std::env::temp_dir);
  base.join("Web2App").join(sub)
}

/// Windows verbatim `\\?\` 前缀剥离（零依赖替代 dunce）。
///
/// 仅 `\\?\C:\...` 驱动器号形态被剥离为 `C:\...`；`\\?\UNC\server\share`
/// 转写为 `\\server\share`。无需 canonicalize：调用方（master 的
/// `dir_of_current_exe` 写出目录）不要求符号链接解析。
pub(crate) fn strip_verbatim_prefix(p: PathBuf) -> PathBuf {
  let s = match p.to_str() {
    Some(s) => s,
    None => return p, // 非 UTF-8 路径，原样返回（Windows 上罕见但保守）
  };
  if let Some(rest) = s.strip_prefix(r"\\?\") {
    // 驱动器号形态：\\?\C:\foo → C:\foo
    let b = rest.as_bytes();
    if b.len() >= 2 && b[1] == b':' {
      return PathBuf::from(rest);
    }
    // UNC 形态：\\?\UNC\server\share → \\server\share
    if let Some(unc_rest) = rest.strip_prefix("UNC\\") {
      return PathBuf::from(format!(r"\\{unc_rest}"));
    }
  }
  p
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
/// 动机与调用约束见 child 侧同名函数（两 crate 各自持有同一实现）。
/// 参数取 `&Window` 而非裸句柄：调用点因此平台无关（Unix 同名函数为 no-op），
/// 句柄在本函数内自取（`WindowExtWindows::hwnd`）。
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
      return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    unsafe { CallWindowProcW(prev, hwnd, msg, wparam, lparam) }
  }

  // SAFETY: handle 为调用方持有的有效窗口句柄。SetWindowLongPtrW 原子换入
  // 哨兵过程并返回原过程；失败返回 0 时不保存（哨兵永不生效）。
  let prev =
    unsafe { SetWindowLongPtrW(handle, GWLP_WNDPROC, close_watch_proc as *const () as isize) };
  if prev != 0 {
    PREV_WNDPROCS.with(|m| {
      m.borrow_mut().insert(handle, prev);
    });
  }
}

// ============================================================================
// 打包器平台项（阶段 0b）
// ============================================================================
//
// Windows 的图标注入与签名由专用模块实现（`winicon.rs` / `sign.rs`，均为
// `#[cfg(target_os = "windows")] mod`），`pack` 即 `builder.rs` 的跨平台编排骨架
// （§4.1 禁止改写 → 本侧只是 re-export，不复制编排）。三者经本外壳统一暴露为
// `crate::platform::*`，使 `main.rs` 的打包调用点无 `#[cfg]`——Linux 侧同名项见
// `platform/linux.rs`（图标走 `.desktop` + hicolor、`.deb` 封装属阶段 1 T-1-4）。
//
// `inject_file_icon` 是 §3.3 之外**追加**的内部接缝：图标注入发生在共享编排骨架的
// 第 2 步（尾部追加之前），必须留在骨架内，故需要同名平台项。

pub(crate) use crate::builder::pack;
pub(crate) use crate::sign::maybe_sign;
pub(crate) use crate::winicon::inject_file_icon;

// ============================================================================
// WebView 挂载（child 侧同名接缝项 D1 的 master 版：Windows 直通）
// ============================================================================

/// 把表单 WebView 挂到窗口——Windows 直通 wry 通用 `build(window)`（句柄即
/// HWND，无平台差异）。Linux 侧同名函数走 `build_gtk`（见 `platform/linux.rs`）。
pub(crate) fn mount_webview<'a>(
  builder: wry::WebViewBuilder<'a>,
  window: &'a tao::window::Window,
) -> Result<wry::WebView, wry::Error> {
  builder.build(window)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn strip_verbatim_prefix_handles_win32_forms() {
    // 驱动器号形态：\\?\C:\foo → C:\foo
    assert_eq!(strip_verbatim_prefix(PathBuf::from(r"\\?\C:\foo\bar")), PathBuf::from(r"C:\foo\bar"));
    // UNC 形态：\\?\UNC\server\share → \\server\share
    assert_eq!(strip_verbatim_prefix(PathBuf::from(r"\\?\UNC\srv\share")), PathBuf::from(r"\\srv\share"));
    // 无前缀：原样返回
    assert_eq!(strip_verbatim_prefix(PathBuf::from(r"C:\plain")), PathBuf::from(r"C:\plain"));
    // 仅 `\\?\` 但既非驱动器也非 UNC：保守原样返回
    assert_eq!(strip_verbatim_prefix(PathBuf::from(r"\\?\weird")), PathBuf::from(r"\\?\weird"));
  }

  #[test]
  fn data_dir_lives_under_brand_and_is_absolute() {
    let s = data_dir("master").to_string_lossy().replace('\\', "/");
    assert!(s.ends_with("/Web2App/master"), "got: {s}");
    assert!(data_dir("master").is_absolute());
  }
}
