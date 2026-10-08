//! icon.rs — 图标域：用户图片解码与窗口图标转换。
//!
//! 图标显示的两条通道（第一性原理拆分）：
//! - **文件图标**（Explorer/任务栏）：母版由 build.rs 经 `.rc` 编译 w2a 图标，
//!   产物字节级继承，无需任何注入（pe 手术已整体删除）；
//! - **窗口图标**（标题栏/运行时任务栏）：本模块把尾部配置中的用户 PNG
//!   （或内嵌 w2a 默认图）转为 tao Icon，三平台同一 API。
//!
//! 关键事实：Windows 窗口类图标从不自动回落 exe `.rsrc` 资源——
//! 未显式设置时标题栏显示系统默认图标。故「未选用户图」必须主动
//! 设为内嵌 w2a 图标（`default_window_icon`），而非依赖资源节。

use tao::window::Icon;

/// PNG 像素总量上限（解压炸弹防御）：4096×4096 = 16.7M 像素 ≈ 67MB RGBA。
/// 图标场景远用不到；超限直接拒绝，避免「小文件大画布」拖垮内存。
const MAX_PIXELS: u64 = 4096 * 4096;

/// 尺寸是否在安全限内（含 u32 乘法溢出防御）。
fn dims_within_limit(w: u32, h: u32) -> bool {
  u64::from(w) * u64::from(h) <= MAX_PIXELS
}

/// PNG → RGBA8（仅 RGB/RGBA 8bit，其余返回 None；尺寸超限返回 None）。
pub fn decode_png(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
  let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
  let mut reader = decoder.read_info().ok()?;
  // 炸弹防御：read_info 已解析 IHDR，在分配像素缓冲前拒绝超限画布
  if !dims_within_limit(reader.info().width, reader.info().height) {
    return None;
  }
  let mut buf = vec![0u8; reader.output_buffer_size().unwrap_or(0)];
  let info = reader.next_frame(&mut buf).ok()?;
  let rgba = match info.color_type {
    png::ColorType::Rgba => buf[..info.buffer_size()].to_vec(),
    png::ColorType::Rgb => {
      let n = info.buffer_size();
      let mut out = Vec::with_capacity(n / 3 * 4);
      for px in buf[..n].as_chunks::<3>().0 {
        out.extend_from_slice(&[px[0], px[1], px[2], 0xFF]);
      }
      out
    }
    _ => return None,
  };
  Some((info.width, info.height, rgba))
}

/// box filter 缩放（目标像素 = 源对应矩形区域的平均值），任意比例通用。
pub fn resize_box(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
  if dw == 0 || dh == 0 || sw == 0 || sh == 0 {
    return Vec::new();
  }
  if dw == sw && dh == sh {
    return src.to_vec();
  }
  let mut out = Vec::with_capacity((dw * dh * 4) as usize);
  for y in 0..dh {
    let sy0 = y * sh / dh;
    let sy1 = ((y + 1) * sh / dh).max(sy0 + 1);
    for x in 0..dw {
      let sx0 = x * sw / dw;
      let sx1 = ((x + 1) * sw / dw).max(sx0 + 1);
      let (mut r, mut g, mut b, mut a) = (0u32, 0u32, 0u32, 0u32);
      let mut n = 0u32;
      for sy in sy0..sy1 {
        for sx in sx0..sx1 {
          let p = ((sy * sw + sx) * 4) as usize;
          r += u32::from(src[p]);
          g += u32::from(src[p + 1]);
          b += u32::from(src[p + 2]);
          a += u32::from(src[p + 3]);
          n += 1;
        }
      }
      out.extend_from_slice(&[(r / n) as u8, (g / n) as u8, (b / n) as u8, (a / n) as u8]);
    }
  }
  out
}

/// 用户 PNG → tao 窗口图标（跨平台统一入口）。
///
/// 超过 256px 的源图 box 缩到 256（系统图标上限，兼顾体积与清晰度）；
/// 解码失败返回 None（调用方回落内嵌默认图标，不阻塞运行）。
pub fn window_icon(png: &[u8]) -> Option<Icon> {
  let (w, h, rgba) = decode_png(png)?;
  if w == 0 || h == 0 {
    return None;
  }
  let (rw, rh, data) = if w > 256 || h > 256 {
    let d = 256u32;
    // 等比缩放贴到 256 边界内（保持宽高比）
    let scale = d as f64 / w.max(h) as f64;
    let nw = ((w as f64 * scale).round() as u32).clamp(1, d);
    let nh = ((h as f64 * scale).round() as u32).clamp(1, d);
    (nw, nh, resize_box(&rgba, w, h, nw, nh))
  } else {
    (w, h, rgba)
  };
  Icon::from_rgba(data, rw, rh).ok()
}

/// 内嵌 w2a 默认图标（256px PNG，编译期打入二进制，~28KB）。
///
/// 用途：「未选用户图」时的窗口图标回落。Windows 窗口类图标不会自动
/// 读 exe 资源节，必须显式设置；此路径三平台统一（macOS/Linux 任务栏同理）。
pub fn default_window_icon() -> Option<Icon> {
  const W2A_PNG: &[u8] = include_bytes!("../assets/logo_256.png");
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
  fn png_roundtrip() {
    let rgba = solid_rgba(8, 8, [1, 2, 3, 4]);
    let png = encode_png(8, 8, &rgba).unwrap();
    let (w, h, out) = decode_png(&png).unwrap();
    assert_eq!((w, h), (8, 8));
    assert_eq!(out, rgba);
  }

  #[test]
  fn decode_png_rgb_expands_to_rgba() {
    // RGB PNG → RGBA（补 0xFF alpha）
    let mut out = Vec::new();
    {
      let mut enc = png::Encoder::new(&mut out, 4u32, 1u32);
      enc.set_color(png::ColorType::Rgb);
      enc.set_depth(png::BitDepth::Eight);
      let mut writer = enc.write_header().unwrap();
      writer.write_image_data(&[10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120]).unwrap();
    }
    let (w, h, rgba) = decode_png(&out).unwrap();
    assert_eq!((w, h), (4, 1));
    assert_eq!(rgba, vec![10, 20, 30, 255, 40, 50, 60, 255, 70, 80, 90, 255, 100, 110, 120, 255]);
  }

  #[test]
  fn decode_png_rejects_garbage() {
    assert!(decode_png(b"not a png").is_none());
    assert!(decode_png(&[]).is_none());
  }

  #[test]
  fn resize_box_downscale_averages() {
    // 2x2 四色 → 1x1：各通道取平均（127.5 → 127 整除）
    let src = vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255];
    let out = resize_box(&src, 2, 2, 1, 1);
    assert_eq!(out, vec![127, 127, 127, 255]);
  }

  #[test]
  fn resize_box_identity() {
    let src = solid_rgba(4, 4, [7, 8, 9, 10]);
    assert_eq!(resize_box(&src, 4, 4, 4, 4), src);
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
  fn dims_within_limit_rejects_bomb() {
    // 正常图标尺寸
    assert!(dims_within_limit(256, 256));
    assert!(dims_within_limit(4096, 4096)); // 恰好等于上限
    // 超限（炸弹画布）：5000×5000 = 25M 像素
    assert!(!dims_within_limit(5000, 5000));
    assert!(!dims_within_limit(30000, 30000));
    // 极端长宽比：u32 乘法溢出防御
    assert!(!dims_within_limit(u32::MAX, u32::MAX));
    assert!(!dims_within_limit(u32::MAX, 2));
    // 1×u32::MAX 不溢出 u64 但超限
    assert!(!dims_within_limit(1, u32::MAX));
    // 零尺寸：0 像素 ≤ 上限（后续 window_icon 另行拒绝）
    assert!(dims_within_limit(0, 0));
  }

  #[test]
  fn default_window_icon_loads() {
    // 编译期内嵌的 w2a PNG 能解码为 Icon
    let icon = default_window_icon();
    assert!(icon.is_some(), "embedded w2a icon failed to decode");
  }
}
