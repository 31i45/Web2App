//! master/src/builder.rs — 打包核心引擎。
//!
//! 唯一职责：`复制 child 模板 → 注入用户图标（UpdateResource）→ 追加尾部配置 → 可选签名`。
//! child 模板编译期内嵌（CHILD_TEMPLATE），无 self_exe 读取，无自繁殖能力——
//! child crate 不含打包器代码，产物（child + 尾部）也无法调用 pack。

use tail_core::{self, AppConfig};
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

/// 编译期内嵌的 child 模板（由 master/build.rs 通过 CHILD_BIN_PATH 指向）。
/// child.exe 先于 master 构建（见 build.ps1），build.rs 拷贝至 OUT_DIR/child.bin。
pub const CHILD_TEMPLATE: &[u8] = include_bytes!(env!("CHILD_BIN_PATH"));

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

/// 主流程：把内嵌 child 模板复制为产物，注入用户图标，追加尾部配置，可选签名。
///
/// 打包顺序（CRITICAL — 防止尾部丢失）：
/// 1. 写 CHILD_TEMPLATE → tmp_out
/// 2. if icon_png Some: inject_file_icon(tmp_out) — 非阻塞（失败 child 保留 w2a 文件图标）
/// 3. write_tail(tmp_out, config) [config 携带 icon_png 供运行时窗口图标]
/// 4. mkdir parent + rename tmp → out（回退 copy+delete）
/// 5. maybe_sign(out) [非阻塞]
/// 6. return BuildOutput
///
/// 注入图标改写 PE .rsrc；必须在追加尾部之前完成，否则尾部会被 EndUpdateResource 丢弃。
pub fn pack(out_path: &Path, url: &str, icon_png: Option<Vec<u8>>) -> io::Result<BuildOutput> {
  validate_url(url).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;

  let title = tail_core::default_title(url);
  let tmp_out = out_path.with_extension("tmp.pack");

  // 组装与落盘；任一步失败都保证临时文件被清理（不留残渣）
  let built = (|| -> io::Result<u64> {
    // 1) 写 child 模板到临时文件
    fs::write(&tmp_out, CHILD_TEMPLATE)?;

    // 2) 注入用户图标到 .rsrc（非阻塞：失败仅警告，child 保留 w2a 默认文件图标）
    //    平台外壳：Windows = UpdateResource 注入 .rsrc；Linux = no-op（图标走
    //    `.desktop` + hicolor，属阶段 1 T-1-4）。
    if let Some(ref png) = icon_png {
      if let Err(e) = crate::platform::inject_file_icon(&tmp_out, png) {
        eprintln!("图标注入失败（非阻塞，用默认 w2a 图标）：{e}");
      }
    }

    // 3) 追加尾部配置（config 携带 icon_png 供运行时窗口图标）
    let config = AppConfig::new(url.to_string(), title, icon_png);
    let size = crate::tail_write::write_tail(&tmp_out, &config)?;

    // 4) 原子落盘：先写临时文件再 rename，避免半写产物。
    // rename 失败常见原因：目标被占用（Windows 文件锁，如旧产物正在运行）、
    // 跨盘移动；回退 copy+delete。
    if let Some(dir) = out_path.parent() {
      fs::create_dir_all(dir)?;
    }
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
    Ok(size) => {
      // 5) 可选签名（非阻塞）。平台外壳：Windows = signtool hook；Linux = no-op hook。
      if let Err(e) = crate::platform::maybe_sign(out_path) {
        eprintln!("签名失败（非阻塞）：{e}");
      }
      Ok(BuildOutput { exe: out_path.to_path_buf(), size })
    }
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
    // 以临时文件模拟：CHILD_TEMPLATE + tail 协议与 PE 无关
    let dir = std::env::temp_dir().join("web2app_builder_tests");
    fs::create_dir_all(&dir).unwrap();
    let out = dir.join("product.bin");

    let product_body = tail_core::strip_tail(CHILD_TEMPLATE).to_vec();
    let cfg = AppConfig::new("https://example.com".into(), "example.com".into(), None);
    fs::write(&out, &product_body).unwrap();
    crate::tail_write::write_tail(&out, &cfg).unwrap();

    let read_back = tail_core::read_tail_from(&out).unwrap().unwrap();
    assert_eq!(read_back.url, "https://example.com");
    assert_eq!(read_back.title, "example.com");
  }

  #[test]
  fn pack_produces_child_template_base() {
    // 产物裸主体 = CHILD_TEMPLATE（验证「从模板复制，非自身繁殖」）
    let dir = std::env::temp_dir().join("web2app_pack_template");
    fs::create_dir_all(&dir).unwrap();
    let out = dir.join("product.bin");

    let result = pack(&out, "https://example.com", None).unwrap();

    let raw = fs::read(&out).unwrap();
    let expected_base = tail_core::strip_tail(CHILD_TEMPLATE);
    assert_eq!(&raw[..expected_base.len()], expected_base);

    // 尾部配置可读回
    let cfg = tail_core::read_tail_from(&out).unwrap().unwrap();
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
    let cfg = tail_core::read_tail_from(&out).unwrap().unwrap();
    assert_eq!(cfg.url, "https://example.com");
    assert_eq!(cfg.icon_png.as_deref(), Some(png.as_slice()));

    // 不选图：icon 字段缺失 → None
    let out2 = dir.join("product2.bin");
    pack(&out2, "https://example.com", None).unwrap();
    let cfg2 = tail_core::read_tail_from(&out2).unwrap().unwrap();
    assert!(cfg2.icon_png.is_none());

    // 垃圾图标字节 → inject 失败非阻塞 → tail 仍存 icon_png（运行时解码失败自动回落）
    let out3 = dir.join("product3.bin");
    pack(&out3, "https://example.com", Some(b"garbage".to_vec())).unwrap();
    assert!(out3.exists());
  }

  #[test]
  fn pack_overwrites_existing_tail() {
    // 重复打包：旧配置被覆盖（非叠加）
    let dir = std::env::temp_dir().join("web2app_pack_overwrite");
    fs::create_dir_all(&dir).unwrap();
    let out = dir.join("product.bin");

    pack(&out, "https://first.com", None).unwrap();
    pack(&out, "https://second.com", None).unwrap();

    let cfg = tail_core::read_tail_from(&out).unwrap().unwrap();
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
    // 关键断言：不留 tmp 残渣。tmp 名 = out.with_extension("tmp.pack")，
    // 即 product.tmp.pack（with_extension 替换 .bin 而非追加）。
    assert!(!dir.join("product.tmp.pack").exists(), "tmp residue leaked");
  }
}
