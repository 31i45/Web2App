//! master/src/winicon.rs — 用户自定义文件图标注入（Windows UpdateResource API）。
//!
//! 把用户 PNG 转为多尺寸 ICO（16/32/48/256，box 缩放），通过
//! BeginUpdateResourceW / UpdateResourceW / EndUpdateResourceW 注入 exe 的
//! .rsrc 节。child 编译期由 winres 嵌入 w2a 默认图标，master 在打包时覆盖。
//!
//! 非阻塞：任何失败返回 Err，调用方（builder::pack）仅警告并继续
//! （child 保留 w2a 默认文件图标，不阻塞打包）。
//!
//! 关键：inject 必须在 write_tail 之前——EndUpdateResource 重写整个 PE，
//! 会丢弃尾部追加的任何字节。

use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use windows_sys::Win32::Foundation::{BOOL, HANDLE};
use windows_sys::Win32::System::LibraryLoader::{
  BeginUpdateResourceW, EndUpdateResourceW, UpdateResourceW,
};

// MAKEINTRESOURCE(id)：Win32 资源 API 的哨兵约定——把整数资源 ID 装进
// PCWSTR 形参（低字=ID，高字=0），API 侧据 IS_INTRESOURCE 判别，永不按指针
// 解引用。故这里不是「悬空指针」，是协议本身；clippy 的 manual_dangling_ptr
// 无法识别该约定，在这唯一构造点定点豁免。
#[allow(clippy::manual_dangling_ptr)]
const fn make_int_resource(id: usize) -> *const u16 {
  id as *const u16
}
// RT_ICON = MAKEINTRESOURCE(3)
const RT_ICON: *const u16 = make_int_resource(3);
// RT_GROUP_ICON = MAKEINTRESOURCE(14)
const RT_GROUP_ICON: *const u16 = make_int_resource(14);
const LANG_NEUTRAL: u16 = 0;

/// PNG → 多尺寸 ICO 字节流（16/32/48/256，box 缩放）。
///
/// ICO 格式：6 字节头 + N 个 16 字节目录条目 + N 个图像数据块。
/// Vista+ 支持 PNG 原位嵌入（无需 BMP/DIB），每个尺寸的 PNG 直接作为数据块。
pub fn png_to_ico(png: &[u8]) -> Option<Vec<u8>> {
  let (w, h, rgba) = tail_core::decode_png(png)?;
  if w == 0 || h == 0 {
    return None;
  }

  // 目标尺寸：16/32/48/256（Windows 标准图标尺寸集）
  let sizes: [u32; 4] = [16, 32, 48, 256];
  let mut entries: Vec<(u32, u32, Vec<u8>)> = Vec::new(); // (w, h, png_bytes)

  for &sz in &sizes {
    let (rw, rh, pixels) = if w == sz && h == sz {
      (sz, sz, rgba.clone())
    } else {
      // 等比缩放到 sz 边界内（保持宽高比）
      let scale = sz as f64 / w.max(h) as f64;
      let nw = ((w as f64 * scale).round() as u32).clamp(1, sz);
      let nh = ((h as f64 * scale).round() as u32).clamp(1, sz);
      (nw, nh, tail_core::resize_box(&rgba, w, h, nw, nh))
    };
    // RGBA 像素重新编码为 PNG（ICO 在 Vista+ 支持 PNG 原位嵌入）
    let png_bytes = encode_png_bytes(rw, rh, &pixels)?;
    entries.push((rw, rh, png_bytes));
  }

  let count = entries.len();
  let header_size = 6;
  let dir_size = 16 * count;
  let mut offset = header_size + dir_size;

  let mut out = Vec::with_capacity(header_size + dir_size + 1024);
  // Header: reserved=0, type=1 (icon), count=N
  out.extend_from_slice(&[0, 0, 1, 0]);
  out.extend_from_slice(&(count as u16).to_le_bytes());

  // Directory entries (16 bytes each)
  for (w, h, png_bytes) in &entries {
    // w(1) h(1) colors(1)=0 reserved(1)=0 planes(2)=1 bitcount(2)=32
    let mut entry = [0u8; 8];
    entry[0] = if *w >= 256 { 0 } else { *w as u8 }; // 0 means 256
    entry[1] = if *h >= 256 { 0 } else { *h as u8 };
    entry[4] = 1; // planes
    entry[6] = 32; // bitcount
    out.extend_from_slice(&entry);
    out.extend_from_slice(&(png_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(&(offset as u32).to_le_bytes());
    offset += png_bytes.len();
  }

  // Image data blocks
  for (_, _, png_bytes) in &entries {
    out.extend_from_slice(png_bytes);
  }

  Some(out)
}

/// RGBA 像素 → PNG 字节流。
fn encode_png_bytes(w: u32, h: u32, rgba: &[u8]) -> Option<Vec<u8>> {
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

/// 拆分 ICO 字节流为 (RT_GROUP_ICON 资源数据, 各 RT_ICON 图像数据)。
///
/// RT_GROUP_ICON 资源格式 = 6 字节头 + N * 14 字节条目（offset 字段替换为 icon_id）。
fn split_ico(ico: &[u8]) -> Result<(Vec<u8>, Vec<Vec<u8>>), String> {
  if ico.len() < 6 {
    return Err("ICO too short".into());
  }
  let count = u16::from_le_bytes([ico[4], ico[5]]) as usize;
  if ico.len() < 6 + 16 * count {
    return Err("ICO directory truncated".into());
  }

  // RT_GROUP_ICON: 6 字节头 + count * 14 字节条目
  let mut group = Vec::with_capacity(6 + 14 * count);
  group.extend_from_slice(&[0, 0, 1, 0]); // reserved=0, type=1
  group.extend_from_slice(&(count as u16).to_le_bytes());

  let mut icon_datas = Vec::with_capacity(count);

  for i in 0..count {
    let entry_start = 6 + 16 * i;
    let entry = &ico[entry_start..entry_start + 16];
    // entry: w(1) h(1) colors(1) reserved(1) planes(2) bitcount(2) size(4) offset(4)
    let size = u32::from_le_bytes([entry[8], entry[9], entry[10], entry[11]]) as usize;
    let offset = u32::from_le_bytes([entry[12], entry[13], entry[14], entry[15]]) as usize;

    if offset + size > ico.len() {
      return Err(format!("ICO image {} data out of bounds", i));
    }
    icon_datas.push(ico[offset..offset + size].to_vec());

    // RT_GROUP_ICON 条目（14 字节）：w h colors reserved planes bitcount size icon_id
    group.extend_from_slice(&entry[..8]); // w h colors reserved planes bitcount
    group.extend_from_slice(&entry[8..12]); // size
    group.extend_from_slice(&((i + 1) as u16).to_le_bytes()); // icon_id (1-based)
  }

  Ok((group, icon_datas))
}

/// 把 ICO 注入 exe 的 .rsrc 节（group id=1，icon ids 1..N）。
///
/// 失败返回 Err（调用方非阻塞处理）。
pub fn inject_file_icon(exe: &Path, png: &[u8]) -> Result<(), String> {
  let ico = png_to_ico(png).ok_or_else(|| "PNG→ICO 转换失败".to_string())?;
  let (group_dir, icon_datas) = split_ico(&ico)?;

  // 路径 → 宽字符 PCWSTR（null 终止）
  let wide: Vec<u16> = exe
    .as_os_str()
    .encode_wide()
    .chain(std::iter::once(0))
    .collect();

  unsafe {
    // bDeleteExistingResources = FALSE（仅更新图标资源，保留其他资源）
    let h: HANDLE = BeginUpdateResourceW(wide.as_ptr(), 0 as BOOL);
    if h.is_null() {
      return Err("BeginUpdateResourceW 失败".to_string());
    }

    // 写入各 RT_ICON 资源（id 1..N）
    for (i, data) in icon_datas.iter().enumerate() {
      let icon_id = make_int_resource(i + 1);
      let ok = UpdateResourceW(
        h,
        RT_ICON,
        icon_id,
        LANG_NEUTRAL,
        data.as_ptr() as *const core::ffi::c_void,
        data.len() as u32,
      );
      if ok == 0 {
        let _ = EndUpdateResourceW(h, 1 as BOOL); // discard
        return Err(format!("UpdateResourceW RT_ICON {} 失败", i + 1));
      }
    }

    // 写入 RT_GROUP_ICON（id 1）— 指向上述 RT_ICON ids
    let ok = UpdateResourceW(
      h,
      RT_GROUP_ICON,
      make_int_resource(1),
      LANG_NEUTRAL,
      group_dir.as_ptr() as *const core::ffi::c_void,
      group_dir.len() as u32,
    );
    if ok == 0 {
      let _ = EndUpdateResourceW(h, 1 as BOOL); // discard
      return Err("UpdateResourceW RT_GROUP_ICON 失败".to_string());
    }

    // 提交（fDiscard = FALSE）
    if EndUpdateResourceW(h, 0 as BOOL) == 0 {
      return Err("EndUpdateResourceW 失败".to_string());
    }
  }

  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  /// 测试用 PNG 构造器。
  fn encode_png(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    {
      let mut enc = png::Encoder::new(&mut out, w, h);
      enc.set_color(png::ColorType::Rgba);
      enc.set_depth(png::BitDepth::Eight);
      let mut writer = enc.write_header().unwrap();
      writer.write_image_data(rgba).unwrap();
    }
    out
  }

  fn solid_rgba(w: u32, h: u32, c: [u8; 4]) -> Vec<u8> {
    let mut v = Vec::with_capacity((w * h * 4) as usize);
    for _ in 0..w * h {
      v.extend_from_slice(&c);
    }
    v
  }

  #[test]
  fn png_to_ico_rejects_garbage_and_empty() {
    assert!(png_to_ico(b"not a png").is_none());
    assert!(png_to_ico(b"").is_none());
  }

  #[test]
  fn png_to_ico_emits_four_standard_sizes_with_png_blocks() {
    let png = encode_png(64, 64, &solid_rgba(64, 64, [10, 20, 30, 255]));
    let ico = png_to_ico(&png).expect("ico");

    // ICO 头：reserved=0 type=1 count=4（16/32/48/256）
    assert_eq!(&ico[0..4], &[0, 0, 1, 0]);
    assert_eq!(u16::from_le_bytes([ico[4], ico[5]]), 4);

    // 目录条目尺寸字段：16,32,48,0(=256)
    let widths: Vec<u8> = (0..4).map(|i| ico[6 + 16 * i]).collect();
    assert_eq!(widths, vec![16, 32, 48, 0]);

    // 每个数据块都是合法 PNG（PNG 签名）——Vista+ 原位嵌入契约
    for i in 0..4 {
      let entry = 6 + 16 * i;
      let size = u32::from_le_bytes(ico[entry + 8..entry + 12].try_into().unwrap()) as usize;
      let offset = u32::from_le_bytes(ico[entry + 12..entry + 16].try_into().unwrap()) as usize;
      assert_eq!(&ico[offset..offset + 8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
      // 块尺寸自洽且不越界
      assert!(size >= 8 && offset + size <= ico.len());
    }
  }

  #[test]
  fn split_ico_roundtrips_directory_and_data_blocks() {
    // 经 png_to_ico 产出的真 ICO 拆分：group 条目数/尺寸、icon id 1..N、
    // 数据块逐字节一致——这是 UpdateResource 写入 .rsrc 的输入契约。
    let png = encode_png(32, 32, &solid_rgba(32, 32, [1, 2, 3, 255]));
    let ico = png_to_ico(&png).unwrap();
    let (group, icons) = split_ico(&ico).unwrap();

    assert_eq!(icons.len(), 4);
    // RT_GROUP_ICON = 6 字节头 + N×14 字节条目
    assert_eq!(group.len(), 6 + 14 * 4);
    assert_eq!(&group[0..4], &[0, 0, 1, 0]);
    assert_eq!(u16::from_le_bytes([group[4], group[5]]), 4);

    for (i, data) in icons.iter().enumerate() {
      // group 条目末两字节 = icon_id（1-based）
      let ge = 6 + 14 * i;
      assert_eq!(u16::from_le_bytes([group[ge + 12], group[ge + 13]]), (i + 1) as u16);
      // 数据块与 ICO 目录指向的偏移一致
      let entry = 6 + 16 * i;
      let size = u32::from_le_bytes(ico[entry + 8..entry + 12].try_into().unwrap()) as usize;
      let offset = u32::from_le_bytes(ico[entry + 12..entry + 16].try_into().unwrap()) as usize;
      assert_eq!(data.as_slice(), &ico[offset..offset + size]);
    }
  }

  #[test]
  fn split_ico_rejects_truncated_input() {
    assert!(split_ico(b"").is_err());
    assert!(split_ico(&[0, 0, 1, 0, 2, 0]).is_err()); // 声称 2 图但无目录
    // 目录条目指向越界数据
    let mut bad = vec![0u8; 6 + 16];
    bad[2] = 1;
    bad[3] = 0;
    bad[4] = 1;
    bad[5] = 0;
    // size=100（位于 entry+8），offset 字段保持 0 → 0+100 超出总长
    bad[14..18].copy_from_slice(&100u32.to_le_bytes());
    assert!(split_ico(&bad).is_err());
  }
}
