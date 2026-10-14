//! child/src/accelerator.rs — 宿主层 Alt+Home 快捷键（WebView2 `AcceleratorKeyPressed`）。
//!
//! 为什么需要宿主层：页面层 `keydown`（见 `webview::home_hotkey_script`）在**焦点位于
//! 跨源 iframe 内**时收不到按键——按键被 iframe 的文档吞掉，托管页拿不到。WebView2 的
//! `AcceleratorKeyPressed` 在**按键送达内容层之前**于宿主侧回调，因而能覆盖该边界。
//!
//! 分层职责：宿主层只做两件事——**判定**（[`is_home_accel`]）与**送达**（派发
//! `ChildEvent::GoHome`）；「是否已在首页 / 在途守卫 / BFCache 回跳」等决策逻辑全部
//! 留在页面层。宿主层**不**在 COM 回调内同步调用 `evaluate_script`（WebView2 回调
//! 重入风险），一律经 `EventLoopProxy` 回流主循环执行。
//!
//! 阶段门控见 [`HOST_ONLY`]。

use crate::webview::ChildEvent;
use tao::event_loop::EventLoopProxy;
use webview2_com::{
  AcceleratorKeyPressedEventHandler,
  Microsoft::Web::WebView2::Win32::{
    COREWEBVIEW2_KEY_EVENT_KIND, COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN,
    COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN, COREWEBVIEW2_PHYSICAL_KEY_STATUS,
    ICoreWebView2AcceleratorKeyPressedEventArgs,
  },
};
use wry::{WebView, WebViewExtWindows};

/// VK_HOME（Home 键虚拟键码）。
const VK_HOME: u16 = 0x24;

/// 阶段开关（Staged rollout）：
///
/// - `false`（阶段 1，当前）：页面层 `keydown` 与宿主层**并存**，宿主层**不**设
///   `SetHandled(true)`——设了会阻止按键送达内容层，与页面层回退互斥（净损失）。
///   双层输入由页面 `goHome` 的 200ms 时间戳去重（见 `webview::home_hotkey_script`）。
/// - `true`（阶段 2，spike 验收通过后）：删除页面层 `keydown`，宿主层独占，命中即
///   `SetHandled(true)` 吞掉按键。
///
/// 未做真实 spike 验收前必须保持 `false`：若 `AcceleratorKeyPressed` 实际不触发而
/// 又删了页面层监听，就是净损失。
pub(crate) const HOST_ONLY: bool = false;

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

/// 从事件参数读取按键状态并判定 Alt+Home；命中且 [`HOST_ONLY`] 时吞掉按键。
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
  if HOST_ONLY {
    let _ = args.SetHandled(true);
  }
  true
}

/// 在 WebView 的 controller 上注册 Alt+Home 处理器，命中时执行 `action`。
///
/// `action` 由调用方按窗口形态给出：主窗口回流 `ChildEvent::GoHome`（经 proxy →
/// 主循环 `evaluate_script`），弹窗 `PostMessageW(hwnd, WM_APP_HOME)` 在自己的
/// 窗口过程内执行。注册失败静默忽略——该能力是「额外送达」，失败不得回退既有行为。
pub(crate) fn register_with(webview: &WebView, action: impl Fn() + 'static) {
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

/// 主窗口便捷封装：命中 → 回流 `ChildEvent::GoHome`。
pub(crate) fn register(webview: &WebView, proxy: EventLoopProxy<ChildEvent>) {
  register_with(webview, move || {
    let _ = proxy.send_event(ChildEvent::GoHome);
  });
}

#[cfg(test)]
mod tests {
  use super::*;
  use webview2_com::Microsoft::Web::WebView2::Win32::{
    COREWEBVIEW2_KEY_EVENT_KIND_KEY_UP, COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_UP,
  };

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
