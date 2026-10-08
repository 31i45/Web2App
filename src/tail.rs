//! tail.rs — 自繁殖协议：尾部追加式单文件配置。
//!
//! 「一个程序就是一份配置」：URL 与可选图标 PNG 追加在可执行文件尾部，
//! 母版=无尾部配置的裸程序，产物=母版+尾部配置。产物本身就是新的母版，
//! 可再次追加新配置 → 生成即母版，自繁殖闭环。
//!
//! 尾部格式：`<payload JSON> <MAGIC:16> <payload_len:u64 LE>`。
//! 长度字段放在 MAGIC 之后，使「自末尾倒数 24 字节」即可定位，
//! 天然支持「覆盖旧配置再追加」（剥离 = 总长 - 24 - payload_len）。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// 尾部魔数。选 16 字节保证足够独特、极低误撞率。
pub const TAIL_MAGIC: &[u8; 16] = b"__WEB2APP_TAIL__";

/// 单个 Web2App 应用的全部配置（即尾部 payload 的 JSON 形态）。
#[derive(Debug, Clone, PartialEq)]
pub struct AppConfig {
  /// 目标网页 URL。
  pub url: String,
  /// 窗口标题（默认取 URL host）。
  pub title: String,
  /// 用户图标 PNG 原始字节（可选；None = 继承母版 w2a 文件图标）。
  pub icon_png: Option<Vec<u8>>,
}

impl AppConfig {
  pub fn new(url: String, title: String, icon_png: Option<Vec<u8>>) -> Self {
    Self { url, title, icon_png }
  }

  /// 序列化为尾部 payload JSON（手写转义，零 serde 依赖，符合极简红线）。
  /// 图标存在时以 base64 内嵌（IPC 传输与存储共用同一编码）。
  pub fn to_json(&self) -> String {
    match &self.icon_png {
      Some(png) => format!(
        "{{\"url\":{},\"title\":{},\"icon\":\"{}\"}}",
        json_escape(&self.url),
        json_escape(&self.title),
        base64_encode(png)
      ),
      None => format!(
        "{{\"url\":{},\"title\":{}}}",
        json_escape(&self.url),
        json_escape(&self.title)
      ),
    }
  }

  /// 从尾部 payload JSON 解析。
  ///
  /// 隐藏契约：字段按 `url → title → icon` 顺序扫描（`to_json` 固定此序，
  /// 协议闭环内自产自销故安全；若字段顺序变更需同步改两处）。
  /// `icon` 字段可缺失（向后兼容旧产物）→ None。
  pub fn from_json(json: &str) -> Option<Self> {
    let (url, rest) = extract_string_field(json, "url")?;
    let (title, rest) = extract_string_field(&rest, "title")?;
    let icon_png = extract_string_field(&rest, "icon")
      .and_then(|(b64, _)| base64_decode(&b64));
    Some(Self { url, title, icon_png })
  }
}

/// 从自身可执行文件读取尾部配置。无尾部 → None（= 母版模式）。
pub fn read_tail() -> io::Result<Option<AppConfig>> {
  let exe = current_exe()?;
  read_tail_from(&exe)
}

/// 读取指定文件的尾部配置（单测/工具模式共用）。
pub fn read_tail_from(path: &Path) -> io::Result<Option<AppConfig>> {
  let bytes = fs::read(path)?;
  Ok(extract_tail(&bytes))
}

/// 把配置追加到目标文件：先剥离旧尾部，再写入新尾部。
/// 返回追加后的文件总大小。
pub fn write_tail(path: &Path, config: &AppConfig) -> io::Result<u64> {
  let bytes = fs::read(path)?;
  let base = strip_tail(&bytes);
  let payload = config.to_json();
  let mut out = base.to_vec();
  out.extend_from_slice(payload.as_bytes());
  out.extend_from_slice(TAIL_MAGIC);
  out.extend_from_slice(&(payload.len() as u64).to_le_bytes());
  fs::write(path, &out)?;
  Ok(out.len() as u64)
}

/// 从字节流尾部提取配置。无尾部 → None。
pub fn extract_tail(bytes: &[u8]) -> Option<AppConfig> {
  let (payload, _) = split_tail(bytes)?;
  let s = std::str::from_utf8(payload).ok()?;
  AppConfig::from_json(s)
}

/// 按尾部协议拆分：返回 (payload 切片, 剥离后裸程序切片)。
/// 布局：`<payload> <MAGIC:16> <payload_len:u64 LE>`。
fn split_tail(bytes: &[u8]) -> Option<(&[u8], &[u8])> {
  if bytes.len() < TAIL_MAGIC.len() + 8 {
    return None;
  }
  let tail_len_end = bytes.len();
  let magic_end = tail_len_end - 8;
  let magic_start = magic_end - TAIL_MAGIC.len();
  if &bytes[magic_start..magic_end] != TAIL_MAGIC {
    return None;
  }
  let len_bytes: [u8; 8] = bytes[magic_end..tail_len_end].try_into().ok()?;
  let payload_len = u64::from_le_bytes(len_bytes) as usize;
  let payload_end = magic_start;
  if payload_len > payload_end {
    return None;
  }
  let payload_start = payload_end - payload_len;
  Some((&bytes[payload_start..payload_end], &bytes[..payload_start]))
}

/// 剥离尾部配置，返回裸程序字节切片。
pub fn strip_tail(bytes: &[u8]) -> &[u8] {
  match split_tail(bytes) {
    Some((_, base)) => base,
    None => bytes,
  }
}

/// 当前可执行文件路径（Windows 下自动去除 UNC `\\?\` 前缀）。
pub fn current_exe() -> io::Result<PathBuf> {
  let p = std::env::current_exe()?;
  #[cfg(target_os = "windows")]
  let p = dunce::canonicalize(&p).unwrap_or(p);
  Ok(p)
}

// ---------- 手写 JSON 工具（零依赖红线） ----------
/// JSON 字符串转义。
fn json_escape(s: &str) -> String {
  let mut out = String::with_capacity(s.len() + 2);
  out.push('"');
  for c in s.chars() {
    match c {
      '"' => out.push_str("\\\""),
      '\\' => out.push_str("\\\\"),
      '\n' => out.push_str("\\n"),
      '\r' => out.push_str("\\r"),
      '\t' => out.push_str("\\t"),
      c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
      c => out.push(c),
    }
  }
  out.push('"');
  out
}

/// 提取 `"key":"value"` 字符串字段，返回 (value, 该字段之后的剩余串)。
///
/// 已知取舍：剩余串逐字段整体拷贝是 O(n²)，payload 仅百字节级且字段仅两个，
/// 按「禁止过度优化」红线保持简单实现。
fn extract_string_field(json: &str, key: &str) -> Option<(String, String)> {
  let (raw, rest) = extract_raw_field(json, key)?;
  // raw 必形如 "\"...\""：扫描已保证首字符是开定界引号、末字符是未转义的
  // 终止引号。必须精确剥这一对——trim_matches 不理解转义，会把值内结尾
  // 转义引号（如 url "...?q=\"x\""）的引号字符也剥掉，造成数据损坏。
  let inner = raw.strip_prefix('"')?.strip_suffix('"')?;
  Some((unescape(inner), rest))
}

/// 提取字段原始文本（含外层引号），返回 (带引号 value, 该字段之后的剩余串)。
fn extract_raw_field<'a>(json: &'a str, key: &str) -> Option<(&'a str, String)> {
  let pat = format!("\"{key}\":");
  let start = json.find(&pat)? + pat.len();
  let rest = &json[start..];
  let rest_trim = rest.trim_start();
  if !rest_trim.starts_with('"') {
    return None;
  }
  // 扫描到未被转义的终止引号
  let bytes = rest_trim.as_bytes();
  let mut end = 1;
  while end < bytes.len() {
    if bytes[end] == b'\\' {
      end += 2; // 跳过转义对
      continue;
    }
    if bytes[end] == b'"' {
      break;
    }
    end += 1;
  }
  if end >= bytes.len() {
    return None;
  }
  let value = &rest_trim[..end + 1]; // 含两端引号
  let after = rest_trim[end + 1..].to_string();
  Some((value, after))
}

/// JSON 字符串反转义。
fn unescape(s: &str) -> String {
  let mut out = String::with_capacity(s.len());
  let mut chars = s.chars();
  while let Some(c) = chars.next() {
    if c != '\\' {
      out.push(c);
      continue;
    }
    match chars.next() {
      Some('n') => out.push('\n'),
      Some('r') => out.push('\r'),
      Some('t') => out.push('\t'),
      Some('"') => out.push('"'),
      Some('\\') => out.push('\\'),
      Some('u') => {
        let hex: String = chars.by_ref().take(4).collect();
        if let Some(n) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
          out.push(n);
        }
      }
      Some(other) => out.push(other),
      None => {}
    }
  }
  out
}

// ---------- base64（图标字节与 JSON 的互转，纯手写零依赖） ----------

/// 标准 base64 编码（带 padding）。
pub fn base64_encode(data: &[u8]) -> String {
  const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
  for chunk in data.chunks(3) {
    let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
    let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
    out.push(TABLE[(n >> 18) as usize & 63] as char);
    out.push(TABLE[(n >> 12) as usize & 63] as char);
    out.push(if chunk.len() > 1 { TABLE[(n >> 6) as usize & 63] as char } else { '=' });
    out.push(if chunk.len() > 2 { TABLE[n as usize & 63] as char } else { '=' });
  }
  out
}

/// 标准 base64 解码（忽略空白，支持 padding）。非法输入返回 None。
pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
  fn val(c: u8) -> Option<u32> {
    match c {
      b'A'..=b'Z' => Some(u32::from(c - b'A')),
      b'a'..=b'z' => Some(u32::from(c - b'a') + 26),
      b'0'..=b'9' => Some(u32::from(c - b'0') + 52),
      b'+' => Some(62),
      b'/' => Some(63),
      _ => None,
    }
  }
  let s: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
  let s: Vec<u8> = s.iter().copied().take_while(|b| *b != b'=').collect();
  if s.len() % 4 == 1 {
    return None;
  }
  let mut out = Vec::with_capacity(s.len() * 3 / 4);
  for chunk in s.chunks(4) {
    let mut n: u32 = 0;
    for (i, c) in chunk.iter().enumerate() {
      n |= val(*c)? << (18 - 6 * i);
    }
    // 4 字符组→3字节；2字符→1字节；3字符→2字节
    let produced = chunk.len().saturating_sub(1);
    let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
    out.extend_from_slice(&bytes[..produced]);
  }
  Some(out)
}

#[cfg(test)]
mod tests {
  use super::*;

  fn sample() -> AppConfig {
    AppConfig::new("https://example.com/app".into(), "My App".into(), None)
  }

  #[test]
  fn json_roundtrip() {
    let c = sample();
    let parsed = AppConfig::from_json(&c.to_json()).unwrap();
    assert_eq!(parsed, c);
  }

  #[test]
  fn json_roundtrip_with_icon() {
    let c = AppConfig::new(
      "https://example.com".into(),
      "example.com".into(),
      Some(vec![1, 2, 3, 255, 0, 128, 64]),
    );
    let parsed = AppConfig::from_json(&c.to_json()).unwrap();
    assert_eq!(parsed, c);
    // payload 确实含 icon 字段
    assert!(c.to_json().contains("\"icon\":"));
  }

  #[test]
  fn json_missing_icon_field_backcompatible() {
    // 旧产物（两字段）→ icon 为 None
    let json = r#"{"url":"https://x.com","title":"T"}"#;
    let c = AppConfig::from_json(json).unwrap();
    assert_eq!(c.url, "https://x.com");
    assert_eq!(c.title, "T");
    assert!(c.icon_png.is_none());
  }

  #[test]
  fn base64_known_vectors() {
    assert_eq!(base64_encode(b"Man"), "TWFu");
    assert_eq!(base64_encode(b"Ma"), "TWE=");
    assert_eq!(base64_encode(b"M"), "TQ==");
    assert_eq!(base64_encode(b""), "");
    assert_eq!(base64_decode("TWFu").unwrap(), b"Man");
    assert_eq!(base64_decode("TWE=").unwrap(), b"Ma");
    assert_eq!(base64_decode("TQ==").unwrap(), b"M");
    assert_eq!(base64_decode("").unwrap(), b"");
    // 忽略空白
    assert_eq!(base64_decode("TW\nFu").unwrap(), b"Man");
    assert!(base64_decode("A").is_none());
    assert!(base64_decode("!!!").is_none());
  }

  #[test]
  fn base64_roundtrip_random_lengths() {
    for len in 0..64usize {
      let data: Vec<u8> = (0..len as u8).map(|i| i.wrapping_mul(37).wrapping_add(11)).collect();
      let enc = base64_encode(&data);
      assert_eq!(base64_decode(&enc).unwrap(), data, "len={len}");
    }
  }

  #[test]
  fn json_escapes_special_chars() {
    let c = AppConfig::new("https://a.com/?q=\"x\"\\n".into(), "标\"题\\".into(), None);
    let parsed = AppConfig::from_json(&c.to_json()).unwrap();
    assert_eq!(parsed.url, "https://a.com/?q=\"x\"\\n");
    assert_eq!(parsed.title, "标\"题\\");
  }

  #[test]
  fn magic_is_stable_and_unique() {
    assert_eq!(TAIL_MAGIC.len(), 16);
    assert_eq!(TAIL_MAGIC, b"__WEB2APP_TAIL__");
  }

  #[test]
  fn extract_tail_none_without_magic() {
    assert!(extract_tail(b"plain executable bytes").is_none());
  }

  #[test]
  fn extract_tail_rejects_corrupt_length() {
    let c = sample();
    let mut bytes = b"MZ body".to_vec();
    bytes.extend_from_slice(c.to_json().as_bytes());
    bytes.extend_from_slice(TAIL_MAGIC);
    bytes.extend_from_slice(&u64::MAX.to_le_bytes()); // 篡改长度
    assert!(extract_tail(&bytes).is_none());
  }

  #[test]
  fn extract_tail_ignores_magic_in_payload() {
    // 魔数出现在正文/负载中不应干扰解析（尾部倒数定位免疫）
    let c = AppConfig::new("https://x.io".into(), "T".into(), None);
    let mut bytes = b"MZ body __WEB2APP_TAIL__ body".to_vec();
    bytes.extend_from_slice(c.to_json().as_bytes());
    bytes.extend_from_slice(TAIL_MAGIC);
    bytes.extend_from_slice(&(c.to_json().len() as u64).to_le_bytes());
    assert_eq!(extract_tail(&bytes).unwrap(), c);
  }

  #[test]
  fn extract_tail_roundtrip() {
    let c = sample();
    let payload = c.to_json();
    let mut bytes = b"MZ fake exe body...".to_vec();
    bytes.extend_from_slice(payload.as_bytes());
    bytes.extend_from_slice(TAIL_MAGIC);
    bytes.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    assert_eq!(extract_tail(&bytes).unwrap(), c);
  }

  #[test]
  fn strip_tail_removes_payload_and_magic() {
    let c = sample();
    let payload = c.to_json();
    let mut bytes = b"MZ body".to_vec();
    bytes.extend_from_slice(payload.as_bytes());
    bytes.extend_from_slice(TAIL_MAGIC);
    bytes.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    assert_eq!(strip_tail(&bytes), b"MZ body");
    assert_eq!(strip_tail(b"MZ body"), b"MZ body");
  }

  #[test]
  fn write_tail_overwrites_old_tail() {
    let dir = std::env::temp_dir().join("web2app_tail_tests");
    fs::create_dir_all(&dir).unwrap();
    let f = dir.join("fake.exe");
    fs::write(&f, b"MZ base program 0123456789").unwrap();

    let a = sample();
    write_tail(&f, &a).unwrap();
    assert_eq!(read_tail_from(&f).unwrap().unwrap(), a);

    let b = AppConfig::new("https://other.io".into(), "B".into(), None);
    write_tail(&f, &b).unwrap();
    assert_eq!(read_tail_from(&f).unwrap().unwrap(), b);

    // 裸程序主体未被破坏
    let raw = fs::read(&f).unwrap();
    assert!(raw.starts_with(b"MZ base program"));
    assert!(strip_tail(&raw).len() < raw.len());
  }

  #[test]
  fn extract_raw_field_handles_nested_json() {
    let json = r#"{"url":"https://x.com","title":"T"}"#;
    let (v, _) = extract_string_field(json, "url").unwrap();
    assert_eq!(v, "https://x.com");
    let (v2, _) = extract_string_field(json, "title").unwrap();
    assert_eq!(v2, "T");
  }

  #[test]
  fn extract_field_missing_key_returns_none() {
    assert!(extract_string_field(r#"{"a":"b"}"#, "url").is_none());
  }

  #[test]
  fn json_roundtrip_value_ending_with_quote() {
    // 回归：值以引号结尾（真实场景：URL 带引号查询参数）。
    // 旧实现 trim_matches('"') 不理解转义，会把 \" 的引号字符也剥掉，
    // `?q="x"` 损坏为 `?q="x`。
    let c = AppConfig::new(
      "https://example.com/?q=\"x\"".into(),
      "example.com".into(),
      None,
    );
    let parsed = AppConfig::from_json(&c.to_json()).unwrap();
    assert_eq!(parsed.url, "https://example.com/?q=\"x\"");
    assert_eq!(parsed.title, "example.com");
  }

  #[test]
  fn json_roundtrip_quote_only_and_leading_quote() {
    // 值本身就是单个引号 / 以引号开头：精确定界符剥离的极端边界
    let c = AppConfig::new("\"".into(), "\"lead".into(), None);
    let parsed = AppConfig::from_json(&c.to_json()).unwrap();
    assert_eq!(parsed.url, "\"");
    assert_eq!(parsed.title, "\"lead");
    // 空值 roundtrip
    let e = AppConfig::new("".into(), "".into(), None);
    assert_eq!(AppConfig::from_json(&e.to_json()).unwrap(), e);
  }

  #[test]
  fn extract_field_out_of_order_returns_none() {
    // 隐藏契约：字段顺序必须 url → title（to_json 固定此序，协议闭环自产自销）
    assert!(AppConfig::from_json(r#"{"title":"t","url":"u"}"#).is_none());
  }

}
