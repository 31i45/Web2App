// ============================================================================
// PNG 解码与 box 缩放（窗口图标转换的纯数据层；tao::Icon 在 child/icon.rs）
// ============================================================================

/// PNG 像素总量上限（解压炸弹防御）：4096×4096 = 16.7M 像素 ≈ 67MB RGBA。
/// 图标场景远用不到；超限直接拒绝，避免「小文件大画布」拖垮内存。
const MAX_PIXELS: u64 = 4096 * 4096;

/// 尺寸是否在安全限内（含 u32 乘法溢出防御）。
fn dims_within_limit(w: u32, h: u32) -> bool {
  u64::from(w) * u64::from(h) <= MAX_PIXELS
}

/// PNG → RGBA8（仅 RGB/RGBA 且位深 8bit，其余返回 None；尺寸超限返回 None）。
pub fn decode_png(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
  let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
  let mut reader = decoder.read_info().ok()?;
  // 炸弹防御：read_info 已解析 IHDR，在分配像素缓冲前拒绝超限画布
  if !dims_within_limit(reader.info().width, reader.info().height) {
    return None;
  }
  // 位深必须为 8：png 解码器默认 Transformations::IDENTITY，16bit 不会自动降位深，
  // 若放行则 buffer 呈 8 字节/像素，却被按 4 字节/像素 RGBA 返回（尺寸语义自相矛盾）。
  if reader.info().bit_depth != png::BitDepth::Eight {
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
  // u64 域运算后再截断为 usize：与 dims_within_limit 的溢出防御口径一致。
  let mut out = Vec::with_capacity((u64::from(dw) * u64::from(dh) * 4) as usize);
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
  fn decode_png_rejects_sixteen_bit() {
    // 16bit RGBA：png 解码器默认不降位深 → 必须拒绝（否则 buffer 呈 8 字节/像素，
    // 却被按 4 字节/像素 RGBA 返回，尺寸语义自相矛盾）。
    let mut out = Vec::new();
    {
      let mut enc = png::Encoder::new(&mut out, 2u32, 2u32);
      enc.set_color(png::ColorType::Rgba);
      enc.set_depth(png::BitDepth::Sixteen);
      let mut w = enc.write_header().unwrap();
      w.write_image_data(&[0u8; 2 * 2 * 4 * 2]).unwrap();
    }
    assert!(decode_png(&out).is_none());
  }
}
