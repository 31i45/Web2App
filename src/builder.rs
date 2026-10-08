//! builder.rs — 打包核心引擎。
//!
//! 唯一职责：`复制母版自身 → 追加尾部配置（含可选图标）`。
//! 无编译器调用、无临时目录、无平台分支、零 PE 手术。
//!
//! 自繁殖机制：产物 = 母版 + 尾部配置，产物含全部母版能力，可继续繁殖。
//! 图标策略：用户 PNG 内嵌尾部配置（产物运行时设为窗口图标）；
//! 不选图 → 尾部无 icon 字段，产物字节级继承母版（含 w2a 文件图标）。

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

/// 主流程：把「当前运行的母版」复制为产物并追加尾部配置。
///
/// - `out_path`：产物输出路径
/// - `url`：目标网页
/// - `icon_png`：用户选择的图标 PNG 字节（可选，内嵌尾部配置；
///   None → 尾部无 icon 字段，产物继承母版图标）
pub fn pack(out_path: &Path, url: &str, icon_png: Option<Vec<u8>>) -> io::Result<BuildOutput> {
  validate_url(url).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;

  let title = default_title(url);
  let self_exe = tail::current_exe()?;
  let product = fs::read(&self_exe)?;

  // 剥离母版自身可能存在的尾部配置（产物从裸母版出发，覆盖旧配置）
  let base = tail::strip_tail(&product);
  let product = base.to_vec();

  let config = AppConfig::new(url.to_string(), title, icon_png);
  let tmp_out = out_path.with_extension("tmp.pack");

  // 组装与落盘；任一步失败都保证临时文件被清理（不留残渣）
  let built = (|| -> io::Result<u64> {
    fs::write(&tmp_out, &product)?;
    let size = tail::write_tail(&tmp_out, &config)?;
    if let Some(dir) = out_path.parent() {
      fs::create_dir_all(dir)?;
    }
    // 原子落盘：先写临时文件再 rename，避免半写产物。
    // rename 失败常见原因：目标被占用（Windows 文件锁，如旧产物正在运行）、
    // 跨盘移动；回退 copy+delete。
    match fs::rename(&tmp_out, out_path) {
      Ok(_) => {}
      Err(_) => {
        fs::copy(&tmp_out, out_path)?;
        let _ = fs::remove_file(&tmp_out);
      }
    }
    Ok(size)
  })();

  match built {
    Ok(size) => Ok(BuildOutput { exe: out_path.to_path_buf(), size }),
    Err(e) => {
      let _ = fs::remove_file(&tmp_out);
      Err(e)
    }
  }
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
    let cfg = AppConfig::new("https://example.com".into(), "example.com".into(), None);
    fs::write(&out, &product_body).unwrap();
    tail::write_tail(&out, &cfg).unwrap();

    let read_back = tail::read_tail_from(&out).unwrap().unwrap();
    assert_eq!(read_back.url, "https://example.com");
    assert_eq!(read_back.title, "example.com");
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
  fn pack_with_icon_stores_in_tail() {
    // 用户图标 → 内嵌尾部配置（可读回，字节一致），不选图 → icon 为 None
    let dir = std::env::temp_dir().join("web2app_pack_icon");
    fs::create_dir_all(&dir).unwrap();

    let png: Vec<u8> = (0..64u8).map(|i| i.wrapping_mul(37)).collect(); // 任意字节即可（协议层不管内容）
    let out = dir.join("product.bin");
    pack(&out, "https://example.com", Some(png.clone())).unwrap();
    let cfg = tail::read_tail_from(&out).unwrap().unwrap();
    assert_eq!(cfg.url, "https://example.com");
    assert_eq!(cfg.icon_png.as_deref(), Some(png.as_slice()));

    // 不选图：icon 字段缺失 → None
    let out2 = dir.join("product2.bin");
    pack(&out2, "https://example.com", None).unwrap();
    let cfg2 = tail::read_tail_from(&out2).unwrap().unwrap();
    assert!(cfg2.icon_png.is_none());

    // 垃圾图标字节 → 原样存入（运行时解码失败自动回落，不阻塞）
    let out3 = dir.join("product3.bin");
    pack(&out3, "https://example.com", Some(b"garbage".to_vec())).unwrap();
    assert!(out3.exists());
  }

  #[test]
  fn pack_overwrites_existing_tail() {
    // 自繁殖链路：对同一产物重复打包，旧配置被覆盖（非叠加）
    let dir = std::env::temp_dir().join("web2app_pack_overwrite");
    fs::create_dir_all(&dir).unwrap();
    let out = dir.join("product.bin");

    pack(&out, "https://first.com", None).unwrap();
    pack(&out, "https://second.com", None).unwrap();

    let cfg = tail::read_tail_from(&out).unwrap().unwrap();
    assert_eq!(cfg.url, "https://second.com");

    // 幂等：同 URL 重复打包体积稳定（覆盖旧尾部，不叠加）
    pack(&out, "https://second.com", None).unwrap();
    let size2 = fs::metadata(&out).unwrap().len();
    pack(&out, "https://second.com", None).unwrap();
    let size3 = fs::metadata(&out).unwrap().len();
    assert_eq!(size2, size3);
  }

  #[test]
  fn pack_failure_leaves_no_tmp_residue() {
    // 失败路径清理：目标路径是一个目录（rename/copy 均失败），
    // 断言报错且临时文件不残留
    let dir = std::env::temp_dir().join("web2app_pack_fail");
    fs::create_dir_all(&dir).unwrap();
    // 占位目录充当产物目标：file → dir 的 rename/copy 必然失败
    fs::create_dir_all(dir.join("product.bin")).unwrap();

    let result = pack(&dir.join("product.bin"), "https://example.com", None);
    assert!(result.is_err());
    // 关键断言：不留 tmp 残渣
    assert!(!dir.join("product.bin.tmp.pack").exists(), "tmp residue leaked");
  }
}
