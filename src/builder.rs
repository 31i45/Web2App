//! builder.rs — 打包核心引擎。
//!
//! 唯一职责：`复制母版自身 → 追加尾部配置`。
//! 无编译器调用、无临时目录、无平台分支。
//!
//! 自繁殖机制：产物 = 母版 + 尾部配置，产物含全部母版能力，可继续繁殖。

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
  // host 必须包含至少一个点且点两侧均含字符（排除 ".com" / "a." 形态）
  let dot = host.find('.').unwrap_or(0);
  if dot == 0 || dot == host.len() - 1 || host.matches('.').count() < 1 {
    return Err("URL 主机名无效".into());
  }
  Ok(())
}

/// 主流程：把「当前运行的母版」复制为产物并完成注入。
///
/// - `out_path`：产物输出路径
/// - `url`：目标网页
pub fn pack(out_path: &Path, url: &str) -> io::Result<BuildOutput> {
  validate_url(url).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;

  let title = default_title(url);
  let self_exe = tail::current_exe()?;
  let mut product = fs::read(&self_exe)?;

  // 剥离母版自身可能存在的尾部配置（产物从裸母版出发，覆盖旧配置）
  let base = tail::strip_tail(&product);
  product = base.to_vec();

  // 追加尾部配置（URL + title）
  let config = AppConfig::new(url.to_string(), title);
  let tmp_out = out_path.with_extension("tmp.pack");
  fs::write(&tmp_out, &product)?;
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
}
