//! builder.rs — 打包核心引擎。
//!
//! 唯一职责：`复制母版自身 → 注入 favicon 图标 → 追加尾部配置`。
//! 无编译器调用、无临时目录；平台差异收敛于 icon::embed_icon。
//!
//! 自繁殖机制：产物 = 母版 + 尾部配置，产物含全部母版能力，可继续繁殖。

use crate::icon::{self, IconEntry};
use crate::tail::{self, AppConfig};
use std::fs;
use std::io;
use std::path::Path;

/// 打包结果。
#[derive(Debug)]
pub struct BuildOutput {
  /// 产物路径。
  pub exe: std::path::PathBuf,
  /// 产物字节数。
  pub size: u64,
}

/// 从 URL 提取默认应用标题（host 去协议前缀）。
pub fn default_title(url: &str) -> String {
  let s = url
    .trim()
    .trim_start_matches("https://")
    .trim_start_matches("http://");
  let host = s.split(['/', '?', '#']).next().unwrap_or("Web2App");
  if host.is_empty() {
    "Web2App".to_string()
  } else {
    host.to_string()
  }
}

/// URL → host → 安全字符序列（单点持有：ui 产物文件名与 webview 数据目录共用）。
/// 非 `[A-Za-z0-9.-]` 字符替换为下划线。
pub fn sanitize_host(url: &str) -> String {
  default_title(url)
    .chars()
    .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' })
    .collect()
}

/// 校验 URL 合法性（scheme http/https）。
pub fn validate_url(url: &str) -> Result<(), String> {
  let u = url.trim();
  if !(u.starts_with("https://") || u.starts_with("http://")) {
    return Err("URL 必须以 https:// 或 http:// 开头".into());
  }
  let host = u
    .trim_start_matches("https://")
    .trim_start_matches("http://")
    .split(['/', '?', '#'])
    .next()
    .unwrap_or("");
  if host.is_empty() {
    return Err("URL 主机名为空".into());
  }
  // host 必须含点且点两侧均有字符（排除 ".com" / "a." / 无点形态）
  match host.find('.') {
    Some(dot) if dot > 0 && dot < host.len() - 1 => {}
    _ => return Err("URL 主机名无效".into()),
  }
  Ok(())
}

/// 主流程：把「当前运行的母版」复制为产物并完成注入。
///
/// - `out_path`：产物输出路径
/// - `url`：目标网页
/// - `favicon`：目标网页 favicon（None 或空 → 跳过图标注入，不报错）
pub fn pack(out_path: &Path, url: &str, favicon: Option<Vec<IconEntry>>) -> io::Result<BuildOutput> {
  validate_url(url).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;

  let title = default_title(url);
  let self_exe = tail::current_exe()?;
  let mut product = fs::read(&self_exe)?;

  // 剥离母版自身可能存在的尾部配置（产物从裸母版出发，覆盖旧配置）
  let base = tail::strip_tail(&product);
  product = base.to_vec();

  let config = AppConfig::new(url.to_string(), title);
  let tmp_out = out_path.with_extension("tmp.pack");
  fs::write(&tmp_out, &product)?;

  // favicon 图标注入（在尾部追加之前，保证协议层与资源层布局独立）：
  // 先剥离母版 logo 节（产物图标完全由 favicon 决定）再注入；
  // 任一步失败不阻塞打包（产物保留母版 logo）。
  if let Some(entries) = favicon {
    if !entries.is_empty() {
      // clone 保留母版字节：注入失败回退时使用
      let stripped = icon::strip_icon(product.clone());
      let ico = icon::assemble_ico(&entries);
      fs::write(&tmp_out, &stripped)?;
      if let Err(e) = icon::embed_icon(&tmp_out, &ico) {
        eprintln!("favicon 注入失败（保留母版图标）: {e}");
        fs::write(&tmp_out, &product)?;
      }
    }
  }

  let size = tail::write_tail(&tmp_out, &config)?;

  // 原子落盘：先写临时文件再 rename，避免半写产物
  if let Some(dir) = out_path.parent() {
    fs::create_dir_all(dir)?;
  }
  match fs::rename(&tmp_out, out_path) {
    Ok(_) => {}
    // Windows 跨盘 rename 失败时回退 copy+delete
    Err(_) => {
      fs::copy(&tmp_out, out_path)?;
      let _ = fs::remove_file(&tmp_out);
    }
  }

  Ok(BuildOutput { exe: out_path.to_path_buf(), size })
}

/// 隐藏工具模式：`web2app --set-icon <png> [target.exe]`（构建脚本自举母版 logo）。
///
/// 替换语义：目标已含图标节时先剥离再注入。Windows 进程运行中
/// 无法改写自身文件（文件锁），故构建脚本总是对「发布副本」注入；
/// 缺省 target 为当前进程 exe。
pub fn tool_set_icon(png_path: &Path, target_exe: &Path) -> io::Result<()> {
  let png = fs::read(png_path)?;
  let entries = icon::entries_from_png(&png)
    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "logo PNG 无法解码"))?;
  let ico = icon::assemble_ico(&entries);
  // 已含图标节则先剥离（幂等：无则原样），实现「设置」而非「追加」
  let image = icon::strip_icon(fs::read(target_exe)?);
  fs::write(target_exe, image)?;
  icon::embed_icon(target_exe, &ico)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn default_title_extracts_host() {
    assert_eq!(default_title("https://example.com/path?q=1"), "example.com");
    assert_eq!(default_title("http://a.b.org"), "a.b.org");
    assert_eq!(default_title("https://example.com/"), "example.com");
  }

  #[test]
  fn default_title_fallback() {
    assert_eq!(default_title("https:///x"), "Web2App");
    assert_eq!(default_title(""), "Web2App");
  }

  #[test]
  fn validate_url_rules() {
    assert!(validate_url("https://example.com").is_ok());
    assert!(validate_url("http://a.cn/x").is_ok());
    assert!(validate_url("ftp://x.com").is_err());
    assert!(validate_url("example.com").is_err());
    assert!(validate_url("https://nohost/").is_err());
    assert!(validate_url("https://.com").is_err());
    assert!(validate_url("").is_err());
  }

  #[test]
  fn sanitize_host_rules() {
    assert_eq!(sanitize_host("https://example.com/x?q=1"), "example.com");
    assert_eq!(sanitize_host("https://a-b.c.org"), "a-b.c.org");
    // 非 ASCII / 非法字符 → 下划线（逐字符替换，「中文站」→ 3 个下划线）
    assert_eq!(sanitize_host("https://中文站.com"), "___.com");
    assert_eq!(sanitize_host("https://a_b~c.com"), "a_b_c.com");
    // 兜底：空 URL → 默认标题
    assert_eq!(sanitize_host(""), "Web2App");
  }

  #[test]
  fn pack_produces_reproducible_tail() {
    // 以临时文件模拟母版（非真实 exe 亦可：tail 协议与 PE 无关）
    let dir = std::env::temp_dir().join("web2app_builder_tests");
    fs::create_dir_all(&dir).unwrap();
    let master = dir.join("master.bin");
    fs::write(&master, b"FAKE_MASTER_EXE_BODY").unwrap();

    let out = dir.join("product.bin");

    // 直接复用 pack 内核逻辑（不经 self_exe）：手动构造
    let product_body = tail::strip_tail(&fs::read(&master).unwrap()).to_vec();
    let cfg = AppConfig::new("https://example.com".into(), "example.com".into());
    fs::write(&out, &product_body).unwrap();
    tail::write_tail(&out, &cfg).unwrap();

    let read_back = tail::read_tail_from(&out).unwrap().unwrap();
    assert_eq!(read_back.url, "https://example.com");
    assert_eq!(read_back.title, "example.com");
  }

  /// 读 PE 节数量（测试 helper，验证图标注入后节数 +1）。
  fn pe_section_count(bytes: &[u8]) -> Option<u16> {
    if bytes.len() < 0x40 {
      return None;
    }
    let pe_off = u32::from_le_bytes(bytes[0x3C..0x40].try_into().ok()?) as usize;
    Some(u16::from_le_bytes(bytes[pe_off + 6..pe_off + 8].try_into().ok()?))
  }

  #[test]
  fn pack_real_self_exe_roundtrip() {
    // 真实调用 pack 主路径：self_exe = 本测试二进制自身
    let dir = std::env::temp_dir().join("web2app_pack_real");
    fs::create_dir_all(&dir).unwrap();
    let out = dir.join("product.bin");

    let result = pack(&out, "https://example.com", None).unwrap();

    // 产物裸主体 = 自身字节（逐字节一致，验证「复制自身」）
    let self_bytes = fs::read(tail::current_exe().unwrap()).unwrap();
    let expected_base = tail::strip_tail(&self_bytes);
    let raw = fs::read(&out).unwrap();
    assert_eq!(&raw[..expected_base.len()], expected_base);

    // 尾部配置可读回
    let cfg = tail::read_tail_from(&out).unwrap().unwrap();
    assert_eq!(cfg.url, "https://example.com");
    assert_eq!(cfg.title, "example.com");
    assert_eq!(result.size, raw.len() as u64);

    // 原子落盘：临时文件已清理
    assert!(!dir.join("product.tmp.pack").exists());
  }

  #[test]
  fn pack_with_favicon_injects_icon() {
    // 带 favicon 的真实打包：Windows 上产物 PE 节数 +1（.rsrc 注入）
    let dir = std::env::temp_dir().join("web2app_pack_icon");
    fs::create_dir_all(&dir).unwrap();
    let out = dir.join("product.bin");

    let png = crate::icon::encode_png(32, 32, &vec![9u8, 8, 7, 255].repeat(32 * 32)).unwrap();
    let favicon = vec![crate::icon::IconEntry::new(32, 32, png)];

    pack(&out, "https://example.com", Some(favicon)).unwrap();

    // 尾部协议不受注入影响
    let cfg = tail::read_tail_from(&out).unwrap().unwrap();
    assert_eq!(cfg.url, "https://example.com");

    if cfg!(target_os = "windows") {
      let self_secs = pe_section_count(&fs::read(tail::current_exe().unwrap()).unwrap()).unwrap();
      let raw = fs::read(&out).unwrap();
      let base = tail::strip_tail(&raw);
      assert_eq!(pe_section_count(base).unwrap(), self_secs + 1);
    }
  }

  #[test]
  fn pack_favicon_replaces_existing_icon_on_bytes() {
    // 「母版已含 logo」场景的字节级验证：strip_icon 后注入，产物只有一个 .rsrc
    // （pack 的剥离+注入链路，不依赖 self_exe）
    if !cfg!(target_os = "windows") {
      return;
    }
    let png = crate::icon::encode_png(32, 32, &vec![9u8, 8, 7, 255].repeat(32 * 32)).unwrap();
    let favicon = vec![crate::icon::IconEntry::new(32, 32, png.clone())];

    // 模拟含 logo 的母版字节：裸 exe 字节 + 注入 w2a
    let mut master = b"MZ fake exe body for icon replace test........".to_vec();
    master.resize(0x400, 0);
    master[0..2].copy_from_slice(b"MZ");
    master[0x3C..0x40].copy_from_slice(&0x100u32.to_le_bytes());
    master[0x100..0x104].copy_from_slice(b"PE\0\0");
    master[0x106..0x108].copy_from_slice(&0u16.to_le_bytes()); // 0 节（最小壳）
    master[0x114..0x116].copy_from_slice(&240u16.to_le_bytes()); // OptSize
    master[0x118..0x11A].copy_from_slice(&0x20Bu16.to_le_bytes()); // PE32+
    master[0x118 + 32..0x118 + 36].copy_from_slice(&0x1000u32.to_le_bytes()); // SectAlign
    master[0x118 + 36..0x118 + 40].copy_from_slice(&0x200u32.to_le_bytes()); // FileAlign

    // 注入 logo → 剥离 → 应回到注入前字节数
    let dir = std::env::temp_dir().join("web2app_strip_test");
    fs::create_dir_all(&dir).unwrap();
    let f = dir.join("m.bin");
    fs::write(&f, &master).unwrap();
    let logo_ico = crate::icon::assemble_ico(&favicon);
    crate::icon::embed_icon(&f, &logo_ico).unwrap();
    let with_icon = fs::read(&f).unwrap();
    assert!(with_icon.len() > master.len());
    // strip 物理回收
    let stripped = crate::icon::strip_icon(with_icon.clone());
    assert_eq!(stripped.len(), master.len());
    // 再注入 favicon：成功（不再报「已含 .rsrc」）
    fs::write(&f, &stripped).unwrap();
    crate::icon::embed_icon(&f, &logo_ico).unwrap();
    assert!(fs::read(&f).unwrap().len() > master.len());
    // 幂等：对无图标字节 strip 原样返回
    assert_eq!(crate::icon::strip_icon(master.clone()), master);
  }
}
