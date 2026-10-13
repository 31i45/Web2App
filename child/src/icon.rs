//! child/src/icon.rs — 窗口图标转换（tao::Icon 层）。
//!
//! PNG 解码与缩放在 tail_core（数据层，零 tao 依赖）；本模块只把像素字节
//! 包成 tao::Icon。两条通道：
//! - 用户图：尾部配置中的 icon_png → window_icon
//! - 默认图：内嵌 w2a 256px PNG → default_window_icon（未选用户图时的回落）
//!
//! 关键事实：Windows 窗口类图标从不自动回落 exe `.rsrc` 资源——
//! 未显式设置时标题栏显示系统默认图标。故「未选用户图」必须主动
//! 设为内嵌 w2a 图标（`default_window_icon`），而非依赖资源节。

use tao::window::Icon;

/// 用户 PNG → tao 窗口图标。
///
/// 超过 256px 的源图 box 缩到 256（系统图标上限，兼顾体积与清晰度）；
/// 解码失败返回 None（调用方回落内嵌默认图标，不阻塞运行）。
pub fn window_icon(png: &[u8]) -> Option<Icon> {
  let (w, h, rgba) = tail_core::decode_png(png)?;
  if w == 0 || h == 0 {
    return None;
  }
  let (rw, rh, data) = if w > 256 || h > 256 {
    let d = 256u32;
    // 等比缩放贴到 256 边界内（保持宽高比）
    let scale = d as f64 / w.max(h) as f64;
    let nw = ((w as f64 * scale).round() as u32).clamp(1, d);
    let nh = ((h as f64 * scale).round() as u32).clamp(1, d);
    (nw, nh, tail_core::resize_box(&rgba, w, h, nw, nh))
  } else {
    (w, h, rgba)
  };
  Icon::from_rgba(data, rw, rh).ok()
}

/// 内嵌 w2a 默认图标（256px PNG，编译期打入二进制，~28KB）。
///
/// 用途：「未选用户图」时的窗口图标回落。Windows 窗口类图标不会自动
/// 读 exe 资源节，必须显式设置。
pub fn default_window_icon() -> Option<Icon> {
  const W2A_PNG: &[u8] = include_bytes!("../../assets/logo_256.png");
  window_icon(W2A_PNG)
}

#[cfg(test)]
mod tests {
  use super::*;

  /// 测试用 PNG 构造器（仅测试需要编码能力，cfg 收敛避免 dead_code）。
  fn encode_png(w: u32, h: u32, rgba: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    {
      let mut enc = png::Encoder::new(&mut out, w, h);
      enc.set_color(png::ColorType::Rgba);
      enc.set_depth(png::BitDepth::Eight);
      let mut writer = enc.write_header().ok()?;
      writer.write_image_data(rgba).ok()?;
    }
    Some(out)
  }

  fn solid_rgba(w: u32, h: u32, c: [u8; 4]) -> Vec<u8> {
    let mut v = Vec::new();
    for _ in 0..w * h {
      v.extend_from_slice(&c);
    }
    v
  }

  #[test]
  fn window_icon_from_small_png() {
    let png = encode_png(32, 32, &solid_rgba(32, 32, [200, 100, 50, 255])).unwrap();
    assert!(window_icon(&png).is_some());
  }

  #[test]
  fn window_icon_downscales_large_png() {
    // 1024px 源 → 等比缩到 256（正方形图保持 256x256）
    let png = encode_png(1024, 1024, &solid_rgba(1024, 1024, [9, 9, 9, 255])).unwrap();
    assert!(window_icon(&png).is_some());
    // 非正方形超大图（1024x512 → 256x128）
    let png2 = encode_png(1024, 512, &solid_rgba(1024, 512, [9, 9, 9, 255])).unwrap();
    assert!(window_icon(&png2).is_some());
  }

  #[test]
  fn window_icon_rejects_garbage() {
    assert!(window_icon(b"garbage").is_none());
    assert!(window_icon(&[]).is_none());
  }

  #[test]
  fn default_window_icon_loads() {
    // 编译期内嵌的 w2a PNG 能解码为 Icon
    let icon = default_window_icon();
    assert!(icon.is_some(), "embedded w2a icon failed to decode");
  }
}
