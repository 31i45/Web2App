//! icon.rs — 图标域：favicon 获取、ICO 解析/组装、跨平台注入统一入口。
//!
//! 职责链：`fetch_favicon`（网络）→ `parse_image`（格式解析）→
//! `assemble_ico`（容器组装）→ `embed_icon`（平台注入适配层）。
//! 上层（builder.rs）只面向 IconEntry 列表，零格式知识。

use std::io::Read;
use std::path::Path;

/// 单个图标条目：尺寸 + PNG 数据（ICO 内嵌 PNG 压缩条目，Vista+ 标准）。
#[derive(Debug, Clone)]
pub struct IconEntry {
  pub width: u32,
  pub height: u32,
  pub png: Vec<u8>,
}

impl IconEntry {
  pub fn new(width: u32, height: u32, png: Vec<u8>) -> Self {
    Self { width, height, png }
  }
}

// ---------- favicon 获取 ----------

/// favicon 获取：GET `https://<host>/favicon.ico`（10s 超时，512KB 上限）。
///
/// 图标是可选增强：任何失败（网络/格式/尺寸）返回 None，不阻塞打包
/// ——产物照常生成，仅无图标。遵循「功能正常可用优先于锦上添花」。
pub fn fetch_favicon(url: &str) -> Option<Vec<IconEntry>> {
  let host = crate::builder::default_title(url);
  if host == "Web2App" {
    return None; // URL 无法解析出 host（防御，正常流程已被 validate_url 拦截）
  }
  let favicon_url = format!("https://{host}/favicon.ico");
  let agent = ureq::AgentBuilder::new()
    .timeout(std::time::Duration::from_secs(10))
    .build();
  let resp = agent
    .get(&favicon_url)
    .set(
      "User-Agent",
      "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Web2App/1.0",
    )
    .call()
    .ok()?;
  let mut bytes = Vec::new();
  resp
    .into_reader()
    .take(512 * 1024)
    .read_to_end(&mut bytes)
    .ok()?;
  parse_image(&bytes)
}

// ---------- 格式解析 ----------

/// 解析 favicon 响应体：PNG 直传 / ICO 容器展开（PNG 条目直用，BMP 条目转 PNG）。
pub fn parse_image(bytes: &[u8]) -> Option<Vec<IconEntry>> {
  if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
    let (w, h) = png_dims(bytes)?;
    return Some(vec![IconEntry::new(w, h, bytes.to_vec())]);
  }
  parse_ico(bytes)
}

/// 解析 ICO 容器；不含可识别条目返回 None。
pub fn parse_ico(bytes: &[u8]) -> Option<Vec<IconEntry>> {
  if bytes.len() < 6 {
    return None;
  }
  if &bytes[0..4] != &[0, 0, 1, 0] {
    return None; // 非 ICO 容器
  }
  let count = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
  let mut out = Vec::new();
  for i in 0..count {
    let e = 6 + i * 16;
    if e + 16 > bytes.len() {
      return None; // 目录项越界（损坏文件）
    }
    let w = bytes[e];
    let h = bytes[e + 1];
    let len = u32::from_le_bytes(bytes[e + 8..e + 12].try_into().ok()?) as usize;
    let off = u32::from_le_bytes(bytes[e + 12..e + 16].try_into().ok()?) as usize;
    if off.checked_add(len)? > bytes.len() {
      return None; // 数据越界（损坏文件）
    }
    let data = &bytes[off..off + len];
    let rw = if w == 0 { 256 } else { u32::from(w) }; // 0 表示 256
    let rh = if h == 0 { 256 } else { u32::from(h) };
    if data.starts_with(&[0x89, b'P', b'N', b'G']) {
      out.push(IconEntry::new(rw, rh, data.to_vec()));
    } else if let Some((w2, h2, rgba)) = dib_to_rgba(data) {
      let png = encode_png(w2, h2, &rgba)?;
      out.push(IconEntry::new(w2, h2, png));
    }
    // 其他格式条目跳过（单条不可识别不阻塞整体）
  }
  if out.is_empty() {
    None
  } else {
    Some(out)
  }
}

/// ICO 内嵌 DIB（无文件头的 BMP）→ (w, h, RGBA)。
///
/// 布局：BITMAPINFOHEADER(40B) + 像素行（bottom-up）+ AND mask（忽略）。
/// ICO 特例：biHeight 为双倍值（XOR 位图 + AND 掩码各占一半）；
/// 24bpp 行需按 4 字节对齐补齐。
fn dib_to_rgba(dib: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
  if dib.len() < 40 {
    return None;
  }
  let w = i32::from_le_bytes(dib[4..8].try_into().ok()?);
  let h2 = i32::from_le_bytes(dib[8..12].try_into().ok()?);
  let bpp = u16::from_le_bytes(dib[14..16].try_into().ok()?);
  if w <= 0 || h2 <= 0 || h2 % 2 != 0 {
    return None;
  }
  let (w, h) = (w as u32, (h2 / 2) as u32);
  let bytes_pp = match bpp {
    32 => 4,
    24 => 3,
    _ => return None, // 8bpp 及以下需调色板，罕见且复杂，跳过
  };
  let row_size = ((w as usize * bytes_pp + 3) / 4) * 4;
  let mut rgba = Vec::with_capacity((w * h * 4) as usize);
  // DIB 行序 bottom-up：源行 h-1-r 对应输出行 r（自上而下）
  for r in 0..h {
    let src_row = (h - 1 - r) as usize;
    let off = 40 + src_row * row_size;
    for x in 0..w as usize {
      let p = off + x * bytes_pp;
      if p + bytes_pp > dib.len() {
        return None;
      }
      rgba.push(dib[p + 2]); // R（BGR 序反转）
      rgba.push(dib[p + 1]); // G
      rgba.push(dib[p]); // B
      rgba.push(if bytes_pp == 4 { dib[p + 3] } else { 0xFF });
    }
  }
  Some((w, h, rgba))
}

// ---------- PNG 编解码与缩放 ----------

/// RGBA8 → PNG。
pub fn encode_png(w: u32, h: u32, rgba: &[u8]) -> Option<Vec<u8>> {
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

/// PNG → RGBA8（仅 RGB/RGBA 8bit，其余返回 None）。
pub fn decode_png(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
  let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
  let mut reader = decoder.read_info().ok()?;
  let mut buf = vec![0u8; reader.output_buffer_size().unwrap_or(0)];
  let info = reader.next_frame(&mut buf).ok()?;
  let rgba = match info.color_type {
    png::ColorType::Rgba => buf[..info.buffer_size()].to_vec(),
    png::ColorType::Rgb => {
      let n = info.buffer_size();
      let mut out = Vec::with_capacity(n / 3 * 4);
      for px in buf[..n].chunks_exact(3) {
        out.extend_from_slice(&[px[0], px[1], px[2], 0xFF]);
      }
      out
    }
    _ => return None,
  };
  Some((info.width, info.height, rgba))
}

/// 读 PNG IHDR 尺寸（不解码像素）。
fn png_dims(bytes: &[u8]) -> Option<(u32, u32)> {
  if bytes.len() < 24 {
    return None;
  }
  Some((
    u32::from_be_bytes(bytes[16..20].try_into().ok()?),
    u32::from_be_bytes(bytes[20..24].try_into().ok()?),
  ))
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

/// 单张 PNG → 多尺寸条目集（256/48/32/16，只缩小不放大，box 缩放保清晰）。
pub fn entries_from_png(png: &[u8]) -> Option<Vec<IconEntry>> {
  let (sw, sh, rgba) = decode_png(png)?;
  let mut out = Vec::new();
  for &d in &[256u32, 48, 32, 16] {
    if d > sw || d > sh {
      continue;
    }
    let resized = resize_box(&rgba, sw, sh, d, d);
    let png = encode_png(d, d, &resized)?;
    out.push(IconEntry::new(d, d, png));
  }
  if out.is_empty() {
    // 源图小于 16px：原尺寸直接使用
    out.push(IconEntry::new(sw, sh, png.to_vec()));
  }
  Some(out)
}

// ---------- 组装与注入 ----------

/// 多尺寸 PNG 条目 → ICO 容器（PNG 压缩条目，Vista+ 标准）。
pub fn assemble_ico(entries: &[IconEntry]) -> Vec<u8> {
  let data_start = 6 + 16 * entries.len();
  let mut out = Vec::with_capacity(data_start + entries.iter().map(|e| e.png.len()).sum::<usize>());
  out.extend_from_slice(&[0, 0, 1, 0]); // ICONDIR: 类型=图标
  out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
  let mut off = data_start as u32;
  for e in entries {
    out.push(if e.width >= 256 { 0 } else { e.width as u8 });
    out.push(if e.height >= 256 { 0 } else { e.height as u8 });
    out.push(0); // 调色板数（PNG 条目无意义）
    out.push(0); // 保留
    out.extend_from_slice(&1u16.to_le_bytes()); // planes
    out.extend_from_slice(&32u16.to_le_bytes()); // bpp
    out.extend_from_slice(&(e.png.len() as u32).to_le_bytes());
    out.extend_from_slice(&off.to_le_bytes());
    off += e.png.len() as u32;
  }
  for e in entries {
    out.extend_from_slice(&e.png);
  }
  out
}

/// 图标注入的跨平台统一入口（适配层，全部平台差异聚合于本函数一屏之内）：
/// - **Windows**：向 exe 的 `.rsrc` 节注入 RT_GROUP_ICON/RT_ICON 资源；
/// - **macOS/Linux**：文件级图标无统一系统机制，返回 Ok（窗口层由系统渲染）。
/// 上层调用零平台感知。
pub fn embed_icon(exe: &Path, ico: &[u8]) -> std::io::Result<()> {
  #[cfg(target_os = "windows")]
  {
    crate::pe::set_icon(exe, ico)
  }
  #[cfg(not(target_os = "windows"))]
  {
    let _ = (exe, ico);
    Ok(())
  }
}

/// 剥离已注入的图标节（母版 logo），使产物图标完全由 favicon 决定。
/// 仅当图标节位于文件末尾（本工具注入方式保证）时物理回收；否则原样返回。
/// 平台适配：Windows 走 PE 手术，其他平台无文件级图标、原样返回。
pub fn strip_icon(image: Vec<u8>) -> Vec<u8> {
  #[cfg(target_os = "windows")]
  {
    crate::pe::strip_rsrc(image)
  }
  #[cfg(not(target_os = "windows"))]
  {
    image
  }
}

#[cfg(test)]
mod tests {
  use super::*;

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
  fn png_dims_without_decode() {
    let png = encode_png(20, 30, &solid_rgba(20, 30, [9, 9, 9, 9])).unwrap();
    assert_eq!(png_dims(&png), Some((20, 30)));
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
  fn assemble_and_parse_ico_roundtrip() {
    let png = encode_png(16, 16, &solid_rgba(16, 16, [5, 6, 7, 255])).unwrap();
    let ico = assemble_ico(&[IconEntry::new(16, 16, png.clone())]);
    let parsed = parse_ico(&ico).unwrap();
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].width, 16);
    assert_eq!(parsed[0].png, png);
  }

  #[test]
  fn assemble_ico_marks_256_as_zero() {
    let png = encode_png(256, 256, &solid_rgba(256, 256, [1, 1, 1, 255])).unwrap();
    let ico = assemble_ico(&[IconEntry::new(256, 256, png)]);
    assert_eq!(ico[6], 0); // 宽度字节 0 = 256
    assert_eq!(ico[7], 0);
    // 解析侧还原为 256
    assert_eq!(parse_ico(&ico).unwrap()[0].width, 256);
  }

  #[test]
  fn parse_ico_mixed_png_and_bmp_entries() {
    // 手工构造：1 个 PNG 条目 + 1 个 2x1 32bpp BMP 条目
    let png = encode_png(16, 16, &solid_rgba(16, 16, [1, 2, 3, 255])).unwrap();
    // DIB: header 40B + 1 行 2 像素 BGRA（biHeight 写双倍 2，真实高 1）
    let mut dib = vec![0u8; 40];
    dib[0..4].copy_from_slice(&40u32.to_le_bytes()); // biSize
    dib[4..8].copy_from_slice(&2i32.to_le_bytes()); // biWidth
    dib[8..12].copy_from_slice(&2i32.to_le_bytes()); // biHeight 双倍（真实 1）
    dib[14..16].copy_from_slice(&32u16.to_le_bytes()); // bpp
    // bottom-up：唯一一行，BGRA ×2
    dib.extend_from_slice(&[0x11, 0x22, 0x33, 0xFF, 0x44, 0x55, 0x66, 0x88]);
    // 组装 ICO 容器
    let data_off = 6 + 2 * 16;
    let mut ico = vec![0, 0, 1, 0, 2, 0];
    ico.extend_from_slice(&[16, 16, 0, 0, 1, 0, 32, 0]); // entry0: 16x16 PNG
    ico.extend_from_slice(&(png.len() as u32).to_le_bytes());
    ico.extend_from_slice(&(data_off as u32).to_le_bytes());
    ico.extend_from_slice(&[2, 1, 0, 0, 1, 0, 32, 0]); // entry1: 2x1 BMP
    ico.extend_from_slice(&(dib.len() as u32).to_le_bytes());
    ico.extend_from_slice(&((data_off + png.len()) as u32).to_le_bytes());
    ico.extend_from_slice(&png);
    ico.extend_from_slice(&dib);

    let parsed = parse_ico(&ico).unwrap();
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].png, png); // PNG 条目直传零转换
    // BMP 条目转 PNG 后解码：BGRA(0x11,0x22,0x33,FF) → RGBA(0x33,0x22,0x11,FF)
    let (w, h, rgba) = decode_png(&parsed[1].png).unwrap();
    assert_eq!((w, h), (2, 1));
    assert_eq!(&rgba[..4], &[0x33, 0x22, 0x11, 0xFF]);
    assert_eq!(&rgba[4..8], &[0x66, 0x55, 0x44, 0x88]);
  }

  #[test]
  fn parse_ico_bmp_24bpp_row_padding() {
    // 3 像素宽 24bpp：行 9 字节 → 补齐 12 字节
    let mut dib = vec![0u8; 40];
    dib[4..8].copy_from_slice(&3i32.to_le_bytes()); // biWidth=3
    dib[8..12].copy_from_slice(&2i32.to_le_bytes()); // 双倍高（真实 1）
    dib[14..16].copy_from_slice(&24u16.to_le_bytes()); // bpp=24
    dib.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 0, 0, 0]); // BGR×3 + 3B padding
    let mut ico = vec![0, 0, 1, 0, 1, 0];
    ico.extend_from_slice(&[3, 1, 0, 0, 1, 0, 24, 0]);
    ico.extend_from_slice(&(dib.len() as u32).to_le_bytes());
    ico.extend_from_slice(&22u32.to_le_bytes());
    ico.extend_from_slice(&dib);

    let parsed = parse_ico(&ico).unwrap();
    let (w, h, rgba) = decode_png(&parsed[0].png).unwrap();
    assert_eq!((w, h), (3, 1));
    assert_eq!(rgba.len(), 12);
    // 3 像素 BGR → RGB 反转 + 24bpp 无 alpha 补 0xFF
    assert_eq!(&rgba[0..4], &[3, 2, 1, 0xFF]);
    assert_eq!(&rgba[4..8], &[6, 5, 4, 0xFF]);
    assert_eq!(&rgba[8..12], &[9, 8, 7, 0xFF]);
  }

  #[test]
  fn parse_ico_rejects_garbage() {
    assert!(parse_ico(b"").is_none());
    assert!(parse_ico(b"not an ico at all").is_none());
    assert!(parse_ico(&[0, 0, 1, 0, 5, 0]).is_none()); // 声称 5 条目但无数据
  }

  #[test]
  fn parse_image_passes_png_directly() {
    let png = encode_png(32, 32, &solid_rgba(32, 32, [9, 9, 9, 255])).unwrap();
    let entries = parse_image(&png).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].width, 32);
    assert_eq!(entries[0].png, png);
  }

  #[test]
  fn entries_from_png_generates_sizes() {
    let rgba = solid_rgba(64, 64, [10, 20, 30, 255]);
    let png = encode_png(64, 64, &rgba).unwrap();
    let entries = entries_from_png(&png).unwrap();
    // 64px 源：生成 48/32/16（256 因大于源跳过）
    let sizes: Vec<u32> = entries.iter().map(|e| e.width).collect();
    assert_eq!(sizes, vec![48, 32, 16]);
    for e in &entries {
      let (w, h, _) = decode_png(&e.png).unwrap();
      assert_eq!((w, h), (e.width, e.height));
    }
  }

  #[test]
  fn entries_from_png_tiny_source_passthrough() {
    let rgba = solid_rgba(8, 8, [1, 1, 1, 255]);
    let png = encode_png(8, 8, &rgba).unwrap();
    let entries = entries_from_png(&png).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!((entries[0].width, entries[0].height), (8, 8));
  }
}
