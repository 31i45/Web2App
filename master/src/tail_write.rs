//! master/src/tail_write.rs — 尾部写入（自 tail-core 外迁）。
//!
//! 为什么不在 tail-core：child 依赖 tail-core。若写尾逻辑留在其中，child 二进制
//! 就**结构性地**带有「向任意文件追加尾部配置」的能力——即便无人调用，能否被
//! release 优化器剔除也依赖运气。外迁后编译期即不可能：child 的依赖图中没有本
//! 模块（`cargo tree -p child` 不含 master，见 audit.ps1）。
//!
//! **不要**改用 `#[cfg(feature)]` 门控替代本拆分：resolver=2 下
//! `cargo build --workspace` 会把 master 启用的 feature 统一到 tail-core，
//! 门控会被静默撤销，形同虚设。
//!
//! 帧格式由 [`tail_core::encode_tail`] 单点持有（v3：`payload | MAGIC | len |
//! ver | crc32`），本模块只负责「剥离旧尾 + 追加新尾 + 落盘」。

use std::fs;
use std::io;
use std::path::Path;
use tail_core::AppConfig;

/// 把配置追加到目标文件：先剥离旧尾部，再写入新尾部（v3 帧）。
/// 返回追加后的文件总大小。
///
/// 剥离用 [`tail_core::strip_tail`]（结构合法即剥离，**损坏帧亦剥离**），
/// 保证重复打包体积稳定、不随次数累积（见 `pack_overwrites_existing_tail`）。
pub fn write_tail(path: &Path, config: &AppConfig) -> io::Result<u64> {
  let bytes = fs::read(path)?;
  let base = tail_core::strip_tail(&bytes);
  let mut out = base.to_vec();
  out.extend_from_slice(&tail_core::encode_tail(config.to_json().as_bytes()));
  fs::write(path, &out)?;
  Ok(out.len() as u64)
}

#[cfg(test)]
mod tests {
  use super::*;

  fn sample() -> AppConfig {
    AppConfig::new("https://example.com/app".into(), "My App".into(), None)
  }

  /// 自 tail-core 随迁：写尾覆盖旧尾、裸主体不受损。
  #[test]
  fn write_tail_overwrites_old_tail() {
    let dir = std::env::temp_dir().join("web2app_tail_write_tests");
    fs::create_dir_all(&dir).unwrap();
    let f = dir.join("fake.exe");
    fs::write(&f, b"MZ base program 0123456789").unwrap();

    let a = sample();
    write_tail(&f, &a).unwrap();
    assert_eq!(tail_core::read_tail_from(&f).unwrap().unwrap(), a);

    let b = AppConfig::new("https://other.io".into(), "B".into(), None);
    write_tail(&f, &b).unwrap();
    assert_eq!(tail_core::read_tail_from(&f).unwrap().unwrap(), b);

    // 裸程序主体未被破坏
    let raw = fs::read(&f).unwrap();
    assert!(raw.starts_with(b"MZ base program"));
    assert!(tail_core::strip_tail(&raw).len() < raw.len());
  }

  /// 自 tail-core 随迁：同配置重复写入体积稳定（旧尾被剥离，不累积）。
  #[test]
  fn write_tail_is_idempotent_size_stable() {
    let dir = std::env::temp_dir().join("web2app_tail_write_tests");
    fs::create_dir_all(&dir).unwrap();
    let f = dir.join("stable.exe");
    fs::write(&f, b"MZ base 0123456789").unwrap();
    let cfg = sample();
    let s1 = write_tail(&f, &cfg).unwrap();
    let s2 = write_tail(&f, &cfg).unwrap();
    let s3 = write_tail(&f, &cfg).unwrap();
    assert_eq!(s1, s2);
    assert_eq!(s2, s3);
  }

  /// 自 tail-core 随迁：CRC 损坏的旧尾亦须被剥离，否则与新增尾部叠加膨胀。
  #[test]
  fn write_tail_strips_corrupt_frame_then_appends() {
    let dir = std::env::temp_dir().join("web2app_tail_write_tests");
    fs::create_dir_all(&dir).unwrap();
    let f = dir.join("corrupt.exe");
    let base = b"MZ base 0123456789".to_vec();
    let cfg = sample();
    let payload = cfg.to_json();

    let mut frame = tail_core::encode_tail(payload.as_bytes());
    let last = frame.len() - 1;
    frame[last] ^= 0xFF; // 破坏 CRC
    let mut bytes = base.clone();
    bytes.extend_from_slice(&frame);
    fs::write(&f, &bytes).unwrap();

    let size = write_tail(&f, &cfg).unwrap();
    let raw = fs::read(&f).unwrap();
    // 旧损坏帧已剥离：体积 = 裸主体 + 恰好一个 v3 帧
    assert_eq!(size, raw.len() as u64);
    assert_eq!(raw.len(), base.len() + payload.len() + tail_core::TAIL_V3_FOOTER_LEN);
    assert_eq!(tail_core::read_tail_from(&f).unwrap().unwrap(), cfg);
  }
}
