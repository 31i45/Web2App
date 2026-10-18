//! child/src/popup.rs — 新窗口请求（target=_blank / window.open）的**路由判定**（纯）。
//!
//! 背景：wry 默认未注册新窗口处理器，WebView2 会把新窗口请求直接吞掉——这正是
//! 部分站点「链接点了没反应」的根因。本模块给出唯一的出口决策
//! （[`classify_new_window`]），**零 OS 依赖**、可任意宿主单测。
//!
//! 各分支的**窗口管线实现**（就地导航 / 系统外发 / 建独立弹窗 / 全屏）在
//! `crate::platform`（Windows 见 `platform/windows.rs`）——按设计文档 §3.2，
//! 本文件只保留纯判定。

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

/// 弹窗 ipc 消息要求的动作（见 [`parse_popup_ipc`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PopupIpc {
  /// 页面请求关闭本弹窗（覆盖后的 `window.close()`）。
  Close,
  /// 本弹窗的 HTML5 全屏状态变化。
  Fullscreen(bool),
}

/// 解析弹窗 ipc 消息体 `<id>:<kind>[:<arg>]`（`w2a:popup:` 前缀由调用方剥离）。
///
/// 为什么消息自带 id：Linux 上弹窗的 ipc 消息**不走弹窗自己的处理器**，而被路由到主窗
/// 处理器（实测，见 `docs/PLATFORM_SEAM_NOTES.md` §3.7）——故脚本以 id 标记来源，主窗
/// 处理器据此把动作回投到对应弹窗（一组弹窗同时存在时不能靠「最近一个」猜）。
///
/// 消息来自页面（不可信输入），格式非法一律 `None`（静默忽略，不 panic）；调用方据此
/// 直接跳过。仅 Linux 会产生此类消息，Windows 侧不可达。
pub(crate) fn parse_popup_ipc(msg: &str) -> Option<(isize, PopupIpc)> {
  let (id, rest) = msg.split_once(':')?;
  let id = id.parse::<isize>().ok()?;
  let action = match rest {
    "close" => PopupIpc::Close,
    "fullscreen:1" => PopupIpc::Fullscreen(true),
    "fullscreen:0" => PopupIpc::Fullscreen(false),
    _ => return None,
  };
  Some((id, action))
}

/// 空白窗判定：空串或 `about:blank`（大小写不敏感）。
pub(crate) fn is_blank(url: &str) -> bool {
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

/// 是否为浏览器内部伪协议：这类 URL 交给系统默认程序只会静默失败或乱开，
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

  #[test]
  fn parse_popup_ipc_accepts_known_actions() {
    assert_eq!(parse_popup_ipc("7:close"), Some((7, PopupIpc::Close)));
    assert_eq!(parse_popup_ipc("12:fullscreen:1"), Some((12, PopupIpc::Fullscreen(true))));
    assert_eq!(parse_popup_ipc("12:fullscreen:0"), Some((12, PopupIpc::Fullscreen(false))));
  }

  #[test]
  fn parse_popup_ipc_rejects_malformed_page_input() {
    // 页面可控输入：非 `id:动作` 形状一律拒绝，且不得 panic
    for m in [
      "",
      "close",
      ":close",
      "x:close",
      "3:",
      "3:fullscreen",
      "3:fullscreen:2",
      "3:fullscreen:1:extra",
      "3:close:extra",
      "3:unknown",
      "3 close",
    ] {
      assert_eq!(parse_popup_ipc(m), None, "{m}");
    }
    // 无 id 前缀的旧形状与主窗消息（`w2a:fullscreen:1` 剥前缀后为 `1`）不构成动作
    assert_eq!(parse_popup_ipc("1"), None);
  }
}
