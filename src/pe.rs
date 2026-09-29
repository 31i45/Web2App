//! pe.rs — PE 图标资源注入（仅 Windows 编译，其他平台为空模块）。
//!
//! 思路（纯 Rust 重写，零外部工具依赖）：
//! 1. 定位 `.rsrc` 节；若不存在（Rust 默认产物无资源节），在节表末尾登记
//!    新节、数据追加到文件末尾（对齐 file_align）。
//! 2. 把 ICO 字节按 RT_GROUP_ICON/RT_ICON 资源目录结构写入。
//! 3. 修正节数量、SizeOfImage、数据目录 2（资源表）。
//!
//! 只处理「给 exe 注入一组图标」这一件事，不实现通用资源编辑器。
//! 对上层暴露 `set_icon`，由 icon.rs 的 `embed_icon` 统一适配调用。

#![cfg(target_os = "windows")]

use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;

/// PE 关键偏移信息。
struct PeInfo {
  /// PE 签名偏移。
  pe_off: usize,
  /// 节表偏移。
  sec_off: usize,
  /// 现有节数量。
  sec_count: u16,
  /// 文件对齐。
  file_align: u32,
  /// 节对齐。
  sect_align: u32,
  /// `.rsrc` 节索引（存在时）。
  rsrc_idx: Option<usize>,
  /// 数据目录表在可选头内的偏移。
  dd_off: usize,
}

impl PeInfo {
  fn parse(f: &mut (impl Read + Seek)) -> io::Result<Self> {
    f.seek(SeekFrom::Start(0))?;
    let mut mz = [0u8; 2];
    f.read_exact(&mut mz)?;
    if &mz != b"MZ" {
      return Err(io::Error::new(io::ErrorKind::InvalidData, "非 PE 文件"));
    }
    f.seek(SeekFrom::Start(0x3C))?;
    let mut pe_off_b = [0u8; 4];
    f.read_exact(&mut pe_off_b)?;
    let pe_off = u32::from_le_bytes(pe_off_b) as usize;
    f.seek(SeekFrom::Start(pe_off as u64))?;
    let mut sig = [0u8; 4];
    f.read_exact(&mut sig)?;
    if &sig != b"PE\0\0" {
      return Err(io::Error::new(io::ErrorKind::InvalidData, "PE 签名缺失"));
    }

    let mut coff = [0u8; 20];
    f.read_exact(&mut coff)?;
    let sec_count = u16::from_le_bytes([coff[2], coff[3]]);
    let opt_size = u16::from_le_bytes([coff[16], coff[17]]);

    let opt_off = pe_off + 24;
    let mut opt_head = vec![0u8; opt_size as usize];
    f.read_exact(&mut opt_head)?;

    let magic = u16::from_le_bytes([opt_head[0], opt_head[1]]);
    let dd_off = match magic {
      0x10B => 96,  // PE32
      0x20B => 112, // PE32+
      _ => return Err(io::Error::new(io::ErrorKind::InvalidData, "未知 PE 格式")),
    };
    let sect_align = u32::from_le_bytes(opt_head[32..36].try_into().unwrap());
    let file_align = u32::from_le_bytes(opt_head[36..40].try_into().unwrap());

    let sec_off = opt_off + opt_size as usize;

    // 扫描节表找 .rsrc（节名字段固定 8 字节）
    f.seek(SeekFrom::Start(sec_off as u64))?;
    let mut sections = vec![0u8; 40 * sec_count as usize];
    f.read_exact(&mut sections)?;
    let mut rsrc_idx = None;
    for i in 0..sec_count as usize {
      let name = &sections[i * 40..i * 40 + 8];
      if name == b".rsrc\0\0\0" {
        rsrc_idx = Some(i);
      }
    }

    Ok(Self {
      pe_off,
      sec_off,
      sec_count,
      file_align,
      sect_align,
      rsrc_idx,
      dd_off,
    })
  }
}

/// 将 ICO 数据注入 PE 文件（原地修改）。
pub fn set_icon(exe_path: &Path, ico_bytes: &[u8]) -> io::Result<()> {
  let mut file = std::fs::OpenOptions::new().read(true).write(true).open(exe_path)?;
  let pe = PeInfo::parse(&mut file)?;

  // 读取整个文件（母版 <10MB，全量读写最简单可靠）
  file.seek(SeekFrom::Start(0))?;
  let mut image = Vec::new();
  file.read_to_end(&mut image)?;

  // 新增 .rsrc 节的 RVA = 现有节最大虚拟端点（对齐 sect_align）
  let rsrc_rva = compute_virt_end(&image, &pe);
  let rsrc = build_rsrc_section(ico_bytes, rsrc_rva);
  let image = append_or_replace_rsrc(image, &pe, &rsrc, rsrc_rva)?;

  file.seek(SeekFrom::Start(0))?;
  file.set_len(0)?;
  file.write_all(&image)?;
  Ok(())
}

/// 剥离已存在的 `.rsrc` 节（物理截断回收体积）。
///
/// 仅当该节是**最后一个节**且物理上位于**文件末尾**（由本工具的注入方式
/// 保证）时才剥离；否则原样返回（不碰非本工具注入的资源节）。
/// 用于产物组装：移除母版 logo 图标，让产物图标完全由 favicon 决定。
pub fn strip_rsrc(mut image: Vec<u8>) -> Vec<u8> {
  let Ok(pe) = PeInfo::parse(&mut std::io::Cursor::new(&image)) else {
    return image;
  };
  let Some(idx) = pe.rsrc_idx else {
    return image; // 无 .rsrc，原样
  };
  if idx + 1 != pe.sec_count as usize {
    return image; // 非尾节：非本工具注入，不动
  }

  let entry_off = pe.sec_off + idx * 40;
  let raw_size = u32::from_le_bytes(image[entry_off + 16..entry_off + 20].try_into().unwrap());
  let raw_ptr = u32::from_le_bytes(image[entry_off + 20..entry_off + 24].try_into().unwrap());
  let raw_end = pad(raw_ptr as usize + raw_size as usize, pe.file_align.max(1));
  if raw_end != image.len() {
    return image; // 物理不在文件末尾（后面已有其他数据），不动
  }

  // 1) 节数量 -1
  let n = pe.sec_count - 1;
  image[pe.pe_off + 6..pe.pe_off + 8].copy_from_slice(&n.to_le_bytes());
  // 2) 节表条目清零
  image[entry_off..entry_off + 40].fill(0);
  // 3) 数据目录 2（资源表）清零
  let dd = pe.pe_off + 24 + pe.dd_off + 2 * 8;
  image[dd..dd + 8].fill(0);
  // 4) SizeOfImage = 剩余节最大虚拟端点（加载器按节表映射，重算安全）
  let mut m = 0u32;
  for i in 0..n as usize {
    let off = pe.sec_off + i * 40;
    let va = u32::from_le_bytes(image[off + 12..off + 16].try_into().unwrap());
    let vs = u32::from_le_bytes(image[off + 8..off + 12].try_into().unwrap());
    m = m.max(va + vs.max(1));
  }
  let soi = pad(m.max(0x1000) as usize, pe.sect_align.max(1)) as u32;
  let soi_off = pe.pe_off + 24 + 56;
  image[soi_off..soi_off + 4].copy_from_slice(&soi.to_le_bytes());
  // 5) 物理截断回收（raw_ptr 由注入时的 pad 保证已对齐）
  image.truncate(raw_ptr as usize);
  image
}

/// 计算现有节的最大虚拟端点（对齐 sect_align），作为新节 VirtualAddress。
fn compute_virt_end(image: &[u8], pe: &PeInfo) -> u32 {
  let mut m = 0u32;
  for i in 0..pe.sec_count as usize {
    let off = pe.sec_off + i * 40;
    let va = u32::from_le_bytes(image[off + 12..off + 16].try_into().unwrap());
    let vs = u32::from_le_bytes(image[off + 8..off + 12].try_into().unwrap());
    m = m.max(va + vs.max(1));
  }
  pad(m as usize, pe.sect_align.max(1)) as u32
}

/// 构造完整 `.rsrc` 节内容（目录树 + 数据条目层 + 数据）。
///
/// `rsrc_rva` 是新增 `.rsrc` 节的虚拟地址；IMAGE_RESOURCE_DATA_ENTRY 的
/// OffsetToData 是绝对 RVA（相对镜像基址），故数据偏移 = rsrc_rva + 节内偏移。
///
/// 标准结构（目录项 8B，数据条目 16B）：
/// 根目录(L1) → 类型目录(L2) → 语言目录(L3) → 数据条目 → 真实数据；
/// 子目录偏移带 `0x8000_0000` 标志，数据条目偏移无标志。
fn build_rsrc_section(ico_bytes: &[u8], rsrc_rva: u32) -> Vec<u8> {
  let icons = split_ico_entries(ico_bytes);
  assert!(!icons.is_empty(), "ICO 无条目");
  let icon_count = icons.len();

  // ---- 目录树布局（相对节起始）----
  let root_items = 2;
  let group_dir_off = 16 + root_items * 8; // 32
  let icon_dir_off = group_dir_off + 16 + 8; // 56
  let leaf_dirs_off = icon_dir_off + 16 + icon_count * 8;
  let leaf_count = 1 + icon_count; // group 语言目录 + 每个 icon 的语言目录
  let data_entry_off = leaf_dirs_off + leaf_count * 24; // 每语言目录 16B 头 + 8B 项
  let data_off = data_entry_off + leaf_count * 16;

  // ---- 数据区 ----
  // GROUP_ICON 资源数据：GRPICONHEADER(6B) + 每条目 14B（nID 重写为 1..=n）
  let mut grp_data = Vec::with_capacity(6 + icon_count * 14);
  grp_data.extend_from_slice(&[0, 0, 1, 0]);
  grp_data.extend_from_slice(&(icon_count as u16).to_le_bytes());
  for (i, (w, h, _, len)) in icons.iter().enumerate() {
    let dim = if *w >= 256 { 0u8 } else { *w as u8 };
    let dim_h = if *h >= 256 { 0u8 } else { *h as u8 };
    grp_data.push(dim);
    grp_data.push(dim_h);
    grp_data.push(0);
    grp_data.push(0);
    grp_data.extend_from_slice(&[1, 0]); // planes
    grp_data.extend_from_slice(&[32, 0]); // bpp
    grp_data.extend_from_slice(&len.to_le_bytes());
    grp_data.extend_from_slice(&((i + 1) as u16).to_le_bytes()); // RT_ICON ID
  }

  // 图标数据布局：group 在前，icons 依序（相对节起始的偏移）
  let mut data = Vec::new();
  let mut rel_offs = Vec::with_capacity(1 + icon_count);
  rel_offs.push(0u32);
  data.extend_from_slice(&grp_data);
  for (_, _, data_off_ico, len) in &icons {
    rel_offs.push(data.len() as u32);
    let s = *data_off_ico as usize;
    data.extend_from_slice(&ico_bytes[s..s + *len as usize]);
  }

  // ---- 组装目录树 ----
  let mut sec = Vec::with_capacity(data_off + data.len());

  // 根目录（Level 1）
  push_dir_header(&mut sec, root_items as u16, 0);
  push_dir_entry(&mut sec, u32::from(RT_GROUP_ICON), group_dir_off as u32 | 0x8000_0000);
  push_dir_entry(&mut sec, u32::from(RT_ICON), icon_dir_off as u32 | 0x8000_0000);

  // group 目录（Level 2，仅 ID=1 一项）
  debug_assert_eq!(sec.len(), group_dir_off);
  push_dir_header(&mut sec, 1, 0);
  push_dir_entry(&mut sec, 1, leaf_dirs_off as u32 | 0x8000_0000);

  // icon 目录（Level 2，ID=1..n）
  debug_assert_eq!(sec.len(), icon_dir_off);
  push_dir_header(&mut sec, icon_count as u16, 0);
  for i in 0..icon_count {
    let leaf = leaf_dirs_off + (1 + i) * 24;
    push_dir_entry(&mut sec, (i + 1) as u32, leaf as u32 | 0x8000_0000);
  }

  // 语言目录（Level 3，1+icon_count 个）：目录项指向数据条目（无高位标志）
  debug_assert_eq!(sec.len(), leaf_dirs_off);
  for i in 0..leaf_count {
    push_dir_header(&mut sec, 1, 0);
    let entry = data_entry_off + i * 16;
    push_dir_entry(&mut sec, 0x0409, entry as u32);
  }

  // 数据条目层：group 数据 + 各 icon 数据（RVA = rsrc_rva + 节内偏移）
  debug_assert_eq!(sec.len(), data_entry_off);
  push_data_entry(&mut sec, rsrc_rva + rel_offs[0], grp_data.len() as u32);
  for i in 0..icon_count {
    push_data_entry(&mut sec, rsrc_rva + rel_offs[i + 1], icons[i].3);
  }

  debug_assert_eq!(sec.len(), data_off);
  sec.extend_from_slice(&data);
  sec
}

/// 目录头（16B）：特征/时间戳/版本全零 + 命名项数(+12) + ID 项数(+14)。
fn push_dir_header(out: &mut Vec<u8>, id_items: u16, named_items: u16) {
  out.extend_from_slice(&[0; 12]);
  out.extend_from_slice(&named_items.to_le_bytes());
  out.extend_from_slice(&id_items.to_le_bytes());
}

/// 目录项（8B）：`id:u32 + offset:u32`。
/// offset 高位 `0x8000_0000` 表示指向子目录（低 31 位为相对节偏移）。
fn push_dir_entry(out: &mut Vec<u8>, id: u32, offset: u32) {
  out.extend_from_slice(&id.to_le_bytes());
  out.extend_from_slice(&offset.to_le_bytes());
}

/// IMAGE_RESOURCE_DATA_ENTRY（16B）：RVA + Size + CodePage + Reserved。
fn push_data_entry(out: &mut Vec<u8>, rva: u32, size: u32) {
  out.extend_from_slice(&rva.to_le_bytes());
  out.extend_from_slice(&size.to_le_bytes());
  out.extend_from_slice(&0u32.to_le_bytes()); // CodePage
  out.extend_from_slice(&0u32.to_le_bytes()); // Reserved
}

/// 拆解 ICO 容器为 (w, h, offset, len) 条目列表。
fn split_ico_entries(ico: &[u8]) -> Vec<(u32, u32, u32, u32)> {
  let count = u16::from_le_bytes([ico[4], ico[5]]) as usize;
  let mut out = Vec::with_capacity(count);
  for i in 0..count {
    let off = 6 + i * 16;
    let w = ico[off];
    let h = ico[off + 1];
    let len = u32::from_le_bytes([ico[off + 8], ico[off + 9], ico[off + 10], ico[off + 11]]);
    let data_off = u32::from_le_bytes([ico[off + 12], ico[off + 13], ico[off + 14], ico[off + 15]]);
    out.push((u32::from(w), u32::from(h), data_off, len));
  }
  out
}

fn pad(n: usize, a: u32) -> usize {
  if a <= 1 {
    n
  } else {
    (n + a as usize - 1) & !(a as usize - 1)
  }
}

/// 把 `.rsrc` 节写入 PE 镜像：新节数据追加到文件末尾，节表末尾登记条目。
fn append_or_replace_rsrc(mut image: Vec<u8>, pe: &PeInfo, rsrc: &[u8], rsrc_rva: u32) -> io::Result<Vec<u8>> {
  let align = pe.file_align.max(1);

  match pe.rsrc_idx {
    Some(_) => Err(io::Error::new(
      io::ErrorKind::Unsupported,
      "PE 已含 .rsrc 节，覆盖逻辑未实现（Rust 产物无资源节）",
    )),
    None => {
      // 新节 raw 位置：文件末尾对齐 file_align（不动已有数据，标准追加方式）
      let new_sec_off = pad(image.len(), align);
      // 节表空间防御：新增节条目必须落在新节数据之前
      let sec_table_end = pe.sec_off + 40 * (pe.sec_count as usize + 1);
      if sec_table_end > new_sec_off {
        return Err(io::Error::new(io::ErrorKind::Unsupported, "PE 头空间不足以新增节"));
      }

      // 文件长度扩展：填零至 new_sec_off，再写 rsrc 并对齐
      image.resize(new_sec_off, 0);
      image.extend_from_slice(rsrc);
      let sec_raw_size = rsrc.len() as u32;
      let sec_raw_end = pad(image.len(), align);
      image.resize(sec_raw_end, 0);

      // 写节表新条目
      let entry_off = pe.sec_off + 40 * pe.sec_count as usize;
      let mut e = [0u8; 40];
      e[..8].copy_from_slice(&[0x2E, 0x72, 0x73, 0x72, 0x63, 0, 0, 0]); // ".rsrc"
      e[8..12].copy_from_slice(&(rsrc.len() as u32).to_le_bytes()); // VirtualSize
      e[12..16].copy_from_slice(&rsrc_rva.to_le_bytes()); // VirtualAddress
      e[16..20].copy_from_slice(&sec_raw_size.to_le_bytes()); // SizeOfRawData
      e[20..24].copy_from_slice(&(new_sec_off as u32).to_le_bytes()); // PointerToRawData
      e[36..40].copy_from_slice(&0x4000_0040u32.to_le_bytes()); // 可读+初始化数据
      image[entry_off..entry_off + 40].copy_from_slice(&e);

      // 更新节数量
      let n = pe.sec_count + 1;
      image[pe.pe_off + 6..pe.pe_off + 8].copy_from_slice(&n.to_le_bytes());

      // 更新 SizeOfImage（可选头 offset 56，PE32/PE32+ 相同）
      let new_size_of_image = pad((rsrc_rva + rsrc.len() as u32) as usize, pe.sect_align.max(1)) as u32;
      let soi_off = pe.pe_off + 24 + 56;
      image[soi_off..soi_off + 4].copy_from_slice(&new_size_of_image.to_le_bytes());

      // 数据目录 2（资源表）：RVA = 新节 VirtualAddress
      let dd_rsrc_off = pe.pe_off + 24 + pe.dd_off + 2 * 8;
      image[dd_rsrc_off..dd_rsrc_off + 4].copy_from_slice(&rsrc_rva.to_le_bytes());
      image[dd_rsrc_off + 4..dd_rsrc_off + 8].copy_from_slice(&(rsrc.len() as u32).to_le_bytes());

      Ok(image)
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  /// 构造最小合法 PE 镜像（MZ + PE 头 + 1 个空节），用于注入测试。
  fn minimal_pe() -> Vec<u8> {
    let mut img = vec![0u8; 0x400];
    img[0..2].copy_from_slice(b"MZ");
    img[0x3C..0x40].copy_from_slice(&0x100u32.to_le_bytes());
    img[0x100..0x104].copy_from_slice(b"PE\0\0");
    // COFF @0x104：Machine(2) Sections(2) TimeStamp(4) SymTab(4) NumSyms(4) OptSize(2) Chars(2)
    img[0x104..0x106].copy_from_slice(&0x8664u16.to_le_bytes()); // AMD64
    img[0x106..0x108].copy_from_slice(&1u16.to_le_bytes()); // 1 节
    img[0x114..0x116].copy_from_slice(&240u16.to_le_bytes()); // OptSize @ COFF+16
    let opt_off = 0x118; // PE签名(4) + COFF(20)
    img[opt_off..opt_off + 2].copy_from_slice(&0x20Bu16.to_le_bytes()); // PE32+
    img[opt_off + 24..opt_off + 32].copy_from_slice(&0x1400_0000u64.to_le_bytes()); // ImageBase
    img[opt_off + 32..opt_off + 36].copy_from_slice(&0x1000u32.to_le_bytes()); // SectAlign
    img[opt_off + 36..opt_off + 40].copy_from_slice(&0x200u32.to_le_bytes()); // FileAlign
    img[opt_off + 56..opt_off + 60].copy_from_slice(&0x3000u32.to_le_bytes()); // SizeOfImage
    let sec_off = opt_off + 240; // 0x208
    img[sec_off..sec_off + 8].copy_from_slice(b".text\0\0\0");
    img[sec_off + 8..sec_off + 12].copy_from_slice(&0x100u32.to_le_bytes()); // VS
    img[sec_off + 12..sec_off + 16].copy_from_slice(&0x1000u32.to_le_bytes()); // VA
    img[sec_off + 16..sec_off + 20].copy_from_slice(&0x200u32.to_le_bytes()); // RawSize
    img[sec_off + 20..sec_off + 24].copy_from_slice(&0x400u32.to_le_bytes()); // RawPtr
    img.truncate(0x600);
    img
  }

  fn test_ico() -> Vec<u8> {
    // 16x16 纯色 PNG → 单条目 ICO
    let mut rgba = Vec::new();
    for _ in 0..16 * 16 {
      rgba.extend_from_slice(&[0x88, 0x44, 0x22, 0xFF]);
    }
    let png = crate::icon::encode_png(16, 16, &rgba).unwrap();
    crate::icon::assemble_ico(&[crate::icon::IconEntry::new(16, 16, png)])
  }

  #[test]
  fn parse_minimal_pe() {
    let mut cur = std::io::Cursor::new(minimal_pe());
    let pe = PeInfo::parse(&mut cur).unwrap();
    assert_eq!(pe.pe_off, 0x100);
    assert_eq!(pe.sec_count, 1);
    assert!(pe.rsrc_idx.is_none());
    assert_eq!(pe.file_align, 0x200);
  }

  #[test]
  fn parse_rejects_non_pe() {
    let mut cur = std::io::Cursor::new(vec![0u8; 64]);
    assert!(PeInfo::parse(&mut cur).is_err());
  }

  #[test]
  fn inject_icon_into_minimal_pe() {
    let img = minimal_pe();
    let pe = {
      let mut cur = std::io::Cursor::new(img.clone());
      PeInfo::parse(&mut cur).unwrap()
    };
    let rsrc = build_rsrc_section(&test_ico(), 0x2000);
    let out = append_or_replace_rsrc(img, &pe, &rsrc, 0x2000).unwrap();

    // 重新解析：应识别到 .rsrc
    let mut cur = std::io::Cursor::new(out.clone());
    let pe2 = PeInfo::parse(&mut cur).unwrap();
    assert_eq!(pe2.sec_count, 2);
    assert!(pe2.rsrc_idx.is_some());

    // 节名正确
    let name_off = pe2.sec_off + 40;
    assert_eq!(&out[name_off..name_off + 8], b".rsrc\0\0\0");

    // 数据目录已登记
    let dd = pe2.pe_off + 24 + pe2.dd_off + 2 * 8;
    let rva = u32::from_le_bytes(out[dd..dd + 4].try_into().unwrap());
    let size = u32::from_le_bytes(out[dd + 4..dd + 8].try_into().unwrap());
    assert_eq!(rva, 0x2000);
    assert_eq!(size as usize, rsrc.len());
  }

  #[test]
  fn rsrc_section_contains_rt_group_and_icons() {
    let ico = test_ico();
    let sec = build_rsrc_section(&ico, 0x2000);
    // 根目录两项：RT_GROUP_ICON(14) + RT_ICON(3)，目录项 8B：id + offset
    let id1 = u32::from_le_bytes(sec[16..20].try_into().unwrap());
    let off1 = u32::from_le_bytes(sec[20..24].try_into().unwrap());
    let id2 = u32::from_le_bytes(sec[24..28].try_into().unwrap());
    let off2 = u32::from_le_bytes(sec[28..32].try_into().unwrap());
    assert_eq!(id1, 14);
    assert_eq!(id2, 3);
    // 子目录标志 + 相对节偏移
    assert_eq!(off1, 32 | 0x8000_0000);
    assert_eq!(off2, 56 | 0x8000_0000);
    // 头部命名/ID 计数（named 在 +12，id 在 +14）
    assert_eq!(u16::from_le_bytes(sec[12..14].try_into().unwrap()), 0);
    assert_eq!(u16::from_le_bytes(sec[14..16].try_into().unwrap()), 2);
  }

  #[test]
  fn rsrc_layout_uses_data_entry_layer() {
    let ico = test_ico(); // 1 个 icon
    let sec = build_rsrc_section(&ico, 0x2000);
    // 布局推导（1 个 icon）：根 32 → group 56 → icon 80 → 语言目录 80+2*24=128
    // 数据条目 128 → 数据 128+2*16=160
    assert!(sec.len() >= 160);

    // group 语言目录项：指向数据条目（无高位标志）
    let g_leaf_off = 80; // 语言目录 16B 头，项在 +16..+24
    let g_id = u32::from_le_bytes(sec[g_leaf_off + 16..g_leaf_off + 20].try_into().unwrap());
    let g_off = u32::from_le_bytes(sec[g_leaf_off + 20..g_leaf_off + 24].try_into().unwrap());
    assert_eq!(g_id, 0x0409);
    assert_eq!(g_off, 128); // 无 0x80000000

    // 数据条目：RVA = 0x2000 + 节内偏移；第一条为 group 数据
    let rva0 = u32::from_le_bytes(sec[128..132].try_into().unwrap());
    let sz0 = u32::from_le_bytes(sec[132..136].try_into().unwrap());
    assert_eq!(rva0, 0x2000);
    assert_eq!(sz0, 6 + 14); // GRPICONHEADER(6) + 1 条 14B

    // 第二条数据条目：icon 数据
    let rva1 = u32::from_le_bytes(sec[144..148].try_into().unwrap());
    let sz1 = u32::from_le_bytes(sec[148..152].try_into().unwrap());
    assert!(rva1 > 0x2000);
    assert!(sz1 > 0);
  }

  #[test]
  fn strip_rsrc_roundtrip() {
    // 注入 → 剥离 → 恢复无 .rsrc 状态（节数/数据目录/文件长度全部还原）
    let img = minimal_pe();
    let pe = {
      let mut cur = std::io::Cursor::new(img.clone());
      PeInfo::parse(&mut cur).unwrap()
    };
    let rsrc = build_rsrc_section(&test_ico(), 0x2000);
    let injected = append_or_replace_rsrc(img.clone(), &pe, &rsrc, 0x2000).unwrap();
    assert!(injected.len() > img.len()); // 确已注入

    let stripped = strip_rsrc(injected.clone());
    // 物理回收：长度恢复到注入前（0x600）
    assert_eq!(stripped.len(), img.len());

    // 节数还原为 1、无 .rsrc
    let pe2 = PeInfo::parse(&mut std::io::Cursor::new(&stripped)).unwrap();
    assert_eq!(pe2.sec_count, 1);
    assert!(pe2.rsrc_idx.is_none());

    // 数据目录 2 清零
    let dd = pe2.pe_off + 24 + pe2.dd_off + 2 * 8;
    assert_eq!(&stripped[dd..dd + 8], &[0u8; 8]);

    // 无 .rsrc 的输入原样返回（幂等）
    assert_eq!(strip_rsrc(img.clone()), img);
  }
}
