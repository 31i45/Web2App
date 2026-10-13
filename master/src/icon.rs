//! master/src/icon.rs — 母版窗口图标（与 child/icon.rs 同构，master 不依赖 child）。
//!
//! master 与 child 是独立 crate（child 是二进制 crate，无 lib 导出），
//! 窗口图标转换逻辑各自持有。共用数据层（tail_core::decode_png/resize_box）。
//! 此处仅母版窗口自身图标用；产物文件图标走 winicon.rs（UpdateResource）。

use tao::window::Icon;

/// 用户 PNG → tao 窗口图标。
pub fn window_icon(png: &[u8]) -> Option<Icon> {
  let (w, h, rgba) = tail_core::decode_png(png)?;
  if w == 0 || h == 0 {
    return None;
  }
  let (rw, rh, data) = if w > 256 || h > 256 {
    let d = 256u32;
    let scale = d as f64 / w.max(h) as f64;
    let nw = ((w as f64 * scale).round() as u32).clamp(1, d);
    let nh = ((h as f64 * scale).round() as u32).clamp(1, d);
    (nw, nh, tail_core::resize_box(&rgba, w, h, nw, nh))
  } else {
    (w, h, rgba)
  };
  Icon::from_rgba(data, rw, rh).ok()
}

/// 内嵌 w2a 默认图标（256px PNG，编译期打入二进制）。
pub fn default_window_icon() -> Option<Icon> {
  const W2A_PNG: &[u8] = include_bytes!("../../assets/logo_256.png");
  window_icon(W2A_PNG)
}
