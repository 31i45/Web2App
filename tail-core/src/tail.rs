use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::text::{base64_decode, base64_encode};

// ============================================================================
// 尾部协议：尾部追加式单文件配置
// ============================================================================

/// 尾部魔数。选 16 字节保证足够独特、极低误撞率。
pub const TAIL_MAGIC: &[u8; 16] = b"__WEB2APP_TAIL__";

/// v2 帧字节数：`MAGIC:16 | payload_len:u64`（历史格式，仍可读）。
const FOOTER_V2_LEN: usize = 24;
/// v3 帧字节数：`MAGIC:16 | payload_len:u64 | ver:u8 | crc32:u32`。
const FOOTER_V3_LEN: usize = 29;
/// 当前写入格式（v3）的帧字节数——**单一事实源**：master 写尾测试、e2e 体积
/// 断言、审计脚本均以此为准，避免各方硬编码 29 造成漂移。
pub const TAIL_V3_FOOTER_LEN: usize = FOOTER_V3_LEN;
/// 版本字节：`1 = CRC32-IEEE`，校验和覆盖 `payload || payload_len:u64 || ver`
/// （**不含** MAGIC）。将来新增版本必须在 [`locate_in`] 探测处显式登记。
const TAIL_VERSION_CRC32: u8 = 1;
/// 尾部探测窗口：末尾固定读取的字节数。须 ≥ 最长 footer（v3 = 29B）并留余量，
/// 供「窗口内含 MAGIC 但无合法帧」的损坏判定。footer 变长时必须同步此常量。
const TAIL_WINDOW: usize = 64;

/// 单个 Web2App 应用的全部配置（即尾部 payload 的 JSON 形态）。
#[derive(Debug, Clone, PartialEq)]
pub struct AppConfig {
  /// 目标网页 URL。
  pub url: String,
  /// 窗口标题（默认取 URL host）。
  pub title: String,
  /// 用户图标 PNG 原始字节（可选；None = 继承 w2a 默认窗口图标）。
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

/// 从自身可执行文件读取尾部配置。无尾部 → None（= 母版 / 运行时组件模式）。
pub fn read_tail() -> io::Result<Option<AppConfig>> {
  let exe = current_exe()?;
  read_tail_from(&exe)
}

/// 读取指定文件的尾部配置（单测/工具模式共用）。
///
/// 只读「末尾窗口 + payload」两段，不做全量 `fs::read`（大 exe 场景显著降低 I/O
/// 与内存峰值）。分类口径与内存解析 [`classify`] 完全一致：
/// 无尾部 / 帧外 → `Ok(None)`；结构损坏或 CRC 不符 → `Err`（child 据此提示
/// 「文件可能已损坏」）；合法帧 → `Ok(Some(config))`。
pub fn read_tail_from(path: &Path) -> io::Result<Option<AppConfig>> {
  let mut file = fs::File::open(path)?;
  let file_len = file.metadata()?.len();
  if file_len < FOOTER_V2_LEN as u64 {
    return Ok(None);
  }
  let win_len = TAIL_WINDOW.min(file_len as usize);
  let win_start = file_len - win_len as u64;
  let mut window = vec![0u8; win_len];
  file.seek(SeekFrom::Start(win_start))?;
  file.read_exact(&mut window)?;

  let (start, len, crc) = match locate_in(&window, file_len) {
    Frame::Absent => return Ok(None),
    Frame::Corrupt => return Err(corrupt_error()),
    Frame::Valid { start, len, crc } => (start, len, crc),
  };

  let mut payload = vec![0u8; len];
  file.seek(SeekFrom::Start(start as u64))?;
  file.read_exact(&mut payload)?;

  if let Some(expected) = crc {
    if payload_crc(&payload) != expected {
      return Err(corrupt_error());
    }
  }
  // 结构合法（且 CRC 通过）却无法解析 JSON：与内存解析口径一致，判为损坏。
  match std::str::from_utf8(&payload).ok().and_then(AppConfig::from_json) {
    Some(cfg) => Ok(Some(cfg)),
    None => Err(corrupt_error()),
  }
}

/// 尾部损坏的 I/O 错误（child 主入口据此显示「文件可能已损坏」）。
fn corrupt_error() -> io::Error {
  io::Error::new(io::ErrorKind::InvalidData, "尾部配置已损坏")
}

/// 把 payload 编码为 v3 尾部帧：
/// `payload | MAGIC:16 | payload_len:u64 | ver:u8 | crc32:u32`。
///
/// 纯编码函数（**不**落盘）；写盘能力在 `master/src/tail_write.rs`，
/// 故 child 的依赖图中不含任何写尾代码。
pub fn encode_tail(payload: &[u8]) -> Vec<u8> {
  let mut out = Vec::with_capacity(payload.len() + FOOTER_V3_LEN);
  out.extend_from_slice(payload);
  out.extend_from_slice(TAIL_MAGIC);
  out.extend_from_slice(&(payload.len() as u64).to_le_bytes());
  out.push(TAIL_VERSION_CRC32);
  out.extend_from_slice(&payload_crc(payload).to_le_bytes());
  out
}

/// 从字节流尾部提取配置。无尾部 / 损坏 → None。
///
/// 只读解析接口（损坏明细被折叠为 None）；需要区分「损坏 vs 无尾」请用
/// [`read_tail_from`]。损坏折叠口径与磁盘读一致：CRC 不符亦视为损坏 → None。
pub fn extract_tail(bytes: &[u8]) -> Option<AppConfig> {
  match classify(bytes) {
    Frame::Valid { start, len, .. } => {
      std::str::from_utf8(&bytes[start..start + len]).ok().and_then(AppConfig::from_json)
    }
    _ => None,
  }
}

/// 尾部帧定位结果（读写两路共用，保证分类一致）。
enum Frame {
  /// 无尾部（母版 / 运行时组件）。
  Absent,
  /// 尾部损坏：存在 MAGIC 但无合法帧，或（经 [`classify`]）CRC 不符。
  Corrupt,
  /// 合法帧：`start` = payload 起点，`len` = payload 长度，
  /// `crc` = Some(v3 校验和) / None(v2 兼容格式)。
  Valid { start: usize, len: usize, crc: Option<u32> },
}

/// 在「文件末尾窗口」内定位尾部帧（纯函数，读写两路共用）。
///
/// `window` 必须是文件的**最后** `min(TAIL_WINDOW, file_len)` 字节，`file_len`
/// 为文件绝对长度（磁盘读取自 metadata、内存解析取 `bytes.len()`）。
///
/// 探测顺序（**顺序写反是本设计最易错处**，见 `crc_mismatch_yields_err` 单测）：
/// 1. 末 29B 成 v3 帧（`MAGIC && ver == 1`）→ 采用（CRC 校验在 [`classify`] 内）；
/// 2. 末 24B 成 v2 帧 → 兼容接受（跳过 CRC）；
/// 3. 窗口内出现 MAGIC 但无合法帧（典型：footer 被截 ≤13 字节）→ 损坏；
/// 4. 窗口内无 MAGIC → 无尾部。
fn locate_in(window: &[u8], file_len: u64) -> Frame {
  let flen = file_len as usize;
  let win_start = flen - window.len();
  // 窗口内取绝对区间 [start, start+len) 的切片；越界（或已被切走）返回 None。
  let at = |start: usize, len: usize| -> Option<&[u8]> {
    if start < win_start || start + len > flen {
      return None;
    }
    Some(&window[start - win_start..start - win_start + len])
  };

  // 1) v3：MAGIC | len | ver | crc
  if flen >= FOOTER_V3_LEN {
    let fs = flen - FOOTER_V3_LEN;
    let magic_ok = at(fs, TAIL_MAGIC.len()) == Some(&TAIL_MAGIC[..]);
    if magic_ok && window[fs + 24 - win_start] == TAIL_VERSION_CRC32 {
      let len = u64::from_le_bytes(at(fs + 16, 8).unwrap().try_into().unwrap()) as usize;
      let crc = u32::from_le_bytes(at(fs + 25, 4).unwrap().try_into().unwrap());
      return if len <= fs {
        Frame::Valid { start: fs - len, len, crc: Some(crc) }
      } else {
        Frame::Corrupt // MAGIC/版本对但长度越界 → 无合法帧
      };
    }
  }
  // 2) v2：MAGIC | len（向后兼容，跳过 CRC）
  if flen >= FOOTER_V2_LEN {
    let fs = flen - FOOTER_V2_LEN;
    if at(fs, TAIL_MAGIC.len()) == Some(&TAIL_MAGIC[..]) {
      let len = u64::from_le_bytes(at(fs + 16, 8).unwrap().try_into().unwrap()) as usize;
      return if len <= fs {
        Frame::Valid { start: fs - len, len, crc: None }
      } else {
        Frame::Corrupt
      };
    }
  }
  // 3) 窗口内含 MAGIC 但无合法帧 → 损坏（footer 被截断的典型特征）
  if window.windows(TAIL_MAGIC.len()).any(|w| w == &TAIL_MAGIC[..]) {
    return Frame::Corrupt;
  }
  Frame::Absent
}

/// 在完整字节流上定位并（对 v3）校验 CRC —— 内存解析的规范分类口径。
///
/// 与 [`read_tail_from`] 共用 [`locate_in`]，二者分类必然一致
/// （见 `read_tail_from_matches_in_memory_parser_on_corpus` 单测）。
fn classify(bytes: &[u8]) -> Frame {
  let win_start = bytes.len().saturating_sub(TAIL_WINDOW);
  match locate_in(&bytes[win_start..], bytes.len() as u64) {
    Frame::Valid { start, len, crc } => {
      if let Some(expected) = crc {
        if payload_crc(&bytes[start..start + len]) != expected {
          return Frame::Corrupt;
        }
      }
      Frame::Valid { start, len, crc }
    }
    other => other,
  }
}

/// 按尾部协议拆分：返回 (payload 切片, 剥离后裸程序切片)。
/// 布局（v3）：`<payload> <MAGIC:16> <payload_len:u64 LE> <ver:u8> <crc32:u32>`。
///
/// 结构合法即拆分（**不**校验 CRC）：剥离是纯切片操作，损坏帧亦须剥离，
/// 否则重打包时旧尾会被保留并累积膨胀（见 builder 的覆盖零膨胀单测）。
fn split_tail(bytes: &[u8]) -> Option<(&[u8], &[u8])> {
  let win_start = bytes.len().saturating_sub(TAIL_WINDOW);
  match locate_in(&bytes[win_start..], bytes.len() as u64) {
    Frame::Valid { start, len, .. } => Some((&bytes[start..start + len], &bytes[..start])),
    _ => None,
  }
}

/// 剥离尾部配置，返回裸程序字节切片。无尾部 / 帧外 → 原样返回。
pub fn strip_tail(bytes: &[u8]) -> &[u8] {
  match split_tail(bytes) {
    Some((_, base)) => base,
    None => bytes,
  }
}

/// CRC32（IEEE 802.3，反射多项式 `0xEDB88320`）——位运算无查表实现。
///
/// payload 仅数百字节（约 4800 次内层迭代），无需查表；零表零静态、零新依赖，
/// 契合极简红线。向量由 `crc32_known_vectors` 单测钉死（`b"123456789"` 的权威值）。
struct Crc32(u32);

impl Crc32 {
  fn new() -> Self {
    Self(0xFFFF_FFFF)
  }

  fn update(&mut self, data: &[u8]) {
    for &b in data {
      self.0 ^= u32::from(b);
      for _ in 0..8 {
        let mask = (self.0 & 1).wrapping_neg();
        self.0 = (self.0 >> 1) ^ (0xEDB8_8320 & mask);
      }
    }
  }

  fn finish(self) -> u32 {
    !self.0
  }
}

/// v3 帧校验和：覆盖 `payload || payload_len:u64 (LE) || ver`。
///
/// 覆盖长度字段可捕获「payload 被改但长度未变」之外的多数篡改；覆盖 ver 使
/// 版本字节本身也受保护。不含 MAGIC（定长常量，改动即意味着整个协议变更）。
fn payload_crc(payload: &[u8]) -> u32 {
  let mut c = Crc32::new();
  c.update(payload);
  c.update(&(payload.len() as u64).to_le_bytes());
  c.update(&[TAIL_VERSION_CRC32]);
  c.finish()
}

/// 当前可执行文件路径。
///
/// 纯 `std::env::current_exe` 转发：平台相关的 verbatim 前缀（`\\?\`）剥离
/// 已外迁各 crate 的 `platform/` 模块（Windows 见
/// `master/src/platform/windows.rs` 的 `strip_verbatim_prefix`），本函数保持零 OS 依赖。
pub fn current_exe() -> io::Result<PathBuf> {
  std::env::current_exe()
}

// ============================================================================
// 手写 JSON 工具（零 serde 红线）
// ============================================================================

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
        match u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
          Some(n) => out.push(n),
          // 非法/不完整转义（非 hex，或孤立代理 \uD800）原样保留，避免静默丢字符。
          None => {
            out.push('\\');
            out.push('u');
            out.push_str(&hex);
          }
        }
      }
      Some(other) => out.push(other),
      None => {}
    }
  }
  out
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
  fn crc32_known_vectors() {
    let crc = |d: &[u8]| {
      let mut c = Crc32::new();
      c.update(d);
      c.finish()
    };
    assert_eq!(crc(b""), 0);
    assert_eq!(crc(b"123456789"), 0xCBF4_3926); // IEEE 802.3 权威向量
    assert_eq!(crc(b"a"), 0xE8B7_BE43);
  }

  #[test]
  fn encode_tail_v3_layout_and_roundtrip() {
    let c = sample();
    let payload = c.to_json();
    let frame = encode_tail(payload.as_bytes());
    assert_eq!(frame.len(), payload.len() + FOOTER_V3_LEN);
    // 布局：payload | MAGIC:16 | len:u64 | ver:u8 | crc32:u32
    let m = payload.len();
    assert_eq!(&frame[m..m + 16], &TAIL_MAGIC[..]);
    assert_eq!(u64::from_le_bytes(frame[m + 16..m + 24].try_into().unwrap()), payload.len() as u64);
    assert_eq!(frame[m + 24], TAIL_VERSION_CRC32);
    assert_eq!(u32::from_le_bytes(frame[m + 25..m + 29].try_into().unwrap()), payload_crc(payload.as_bytes()));
    // 内存读回 + 剥离
    let mut bytes = b"MZ body 0123456789".to_vec();
    bytes.extend_from_slice(&frame);
    assert_eq!(extract_tail(&bytes).unwrap(), c);
    assert_eq!(strip_tail(&bytes), b"MZ body 0123456789");
  }

  #[test]
  fn v2_tail_still_readable() {
    // 历史 v2 格式（无 CRC）仍可读；剥离按 24B footer
    let c = sample();
    let payload = c.to_json();
    let mut bytes = b"MZ legacy".to_vec();
    bytes.extend_from_slice(payload.as_bytes());
    bytes.extend_from_slice(TAIL_MAGIC);
    bytes.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    assert_eq!(extract_tail(&bytes).unwrap(), c);
    assert_eq!(strip_tail(&bytes), b"MZ legacy");
  }

  #[test]
  fn crc_mismatch_yields_err_on_disk_and_none_in_memory() {
    let c = sample();
    let payload = c.to_json();
    let mut frame = encode_tail(payload.as_bytes());
    let last = frame.len() - 1;
    frame[last] ^= 0xFF; // 篡改 CRC 尾字节
    let mut bytes = b"MZ body 0123456789".to_vec();
    bytes.extend_from_slice(&frame);

    // 磁盘路径：损坏必须显式报错（供 child 提示「文件可能已损坏」）
    let dir = std::env::temp_dir().join("web2app_tail_tests");
    fs::create_dir_all(&dir).unwrap();
    let f = dir.join("crc_mismatch.bin");
    fs::write(&f, &bytes).unwrap();
    assert!(read_tail_from(&f).is_err());
    // 内存路径：损坏折叠为 None
    assert!(extract_tail(&bytes).is_none());
    // 未被篡改的对照帧仍是 Some（证明上面的 None 源于 CRC 而非解析失败）
    let mut ok = b"MZ body 0123456789".to_vec();
    ok.extend_from_slice(&encode_tail(payload.as_bytes()));
    assert_eq!(extract_tail(&ok).unwrap(), c);
  }

  #[test]
  fn truncated_footer_classification() {
    // 截断 v3 footer 的三种归口：
    //  - 截 5 字节（恰好去掉 ver+crc）→ 剩余 24 字节恰是合法 v2 帧 → 兼容读回（配置完好）
    //  - 其余 1..=13 字节（MAGIC 仍完整留在文件内）→ 判损坏
    //  - ≥14 字节（MAGIC 自身被截断）→ 归入「无尾部」
    let cfg = sample();
    let payload = cfg.to_json();
    let mut bytes = b"MZ body 0123456789".to_vec();
    bytes.extend_from_slice(&encode_tail(payload.as_bytes()));
    let dir = std::env::temp_dir().join("web2app_tail_tests");
    fs::create_dir_all(&dir).unwrap();

    for cut in 1..=13 {
      let truncated = &bytes[..bytes.len() - cut];
      let f = dir.join(format!("trunc_{cut}.bin"));
      fs::write(&f, truncated).unwrap();
      if cut == 5 {
        // v3 的前 24 字节 == 合法 v2 帧，故此截断按 v2 兼容路径读回同一配置
        assert_eq!(extract_tail(truncated).unwrap(), cfg, "cut={cut} 应按 v2 兼容读回");
        assert_eq!(read_tail_from(&f).unwrap().unwrap(), cfg);
      } else {
        assert!(extract_tail(truncated).is_none(), "cut={cut} 应判损坏");
        assert!(read_tail_from(&f).is_err(), "cut={cut} 磁盘读应报错");
      }
    }
    // MAGIC 自身被截断 → 收不到 → 无尾部
    assert!(extract_tail(&bytes[..bytes.len() - 20]).is_none());
  }

  /// 窗口读与「全量读 + 内存解析」的分类必须逐一相同（守护两条读路径不漂移）。
  #[test]
  fn read_tail_from_matches_in_memory_parser_on_corpus() {
    #[derive(PartialEq, Debug)]
    enum Cls {
      None,
      Some(String),
      Corrupt,
    }

    let classify_mem = |b: &[u8]| -> Cls {
      match classify(b) {
        Frame::Valid { start, len, .. } => std::str::from_utf8(&b[start..start + len])
          .ok()
          .and_then(AppConfig::from_json)
          .map(|c| Cls::Some(c.to_json()))
          .unwrap_or(Cls::Corrupt),
        Frame::Corrupt => Cls::Corrupt,
        Frame::Absent => Cls::None,
      }
    };

    let p1 = AppConfig::new("https://a.com/x".into(), "A".into(), None).to_json();
    let p2 =
      AppConfig::new("https://b.com/?q=\"y\"".into(), "B".into(), Some(vec![0, 1, 2, 255])).to_json();
    let base = || b"MZ fake exe body .......................".to_vec();
    let base_len = base().len();

    let mut corpus: Vec<Vec<u8>> = Vec::new();
    // 合法 v3（含图标）/ v2 / 无尾 / 空 / 极小
    let full1 = {
      let mut b = base();
      b.extend_from_slice(&encode_tail(p1.as_bytes()));
      b
    };
    corpus.push(full1.clone());
    {
      let mut b = base();
      b.extend_from_slice(&encode_tail(p2.as_bytes()));
      corpus.push(b);
    }
    {
      let mut b = base();
      b.extend_from_slice(p1.as_bytes());
      b.extend_from_slice(TAIL_MAGIC);
      b.extend_from_slice(&(p1.len() as u64).to_le_bytes());
      corpus.push(b);
    }
    corpus.push(base());
    corpus.push(Vec::new());
    for n in 0..40usize {
      corpus.push(vec![0xAB; n]);
    }
    // v3 逐截断
    for cut in 0..=full1.len() {
      corpus.push(full1[..full1.len() - cut].to_vec());
    }
    // CRC / ver / payload 篡改
    {
      let mut t = full1.clone();
      let l = t.len();
      t[l - 1] ^= 0xFF;
      corpus.push(t);
    }
    {
      let mut t = full1.clone();
      let l = t.len();
      t[l - 5] ^= 0x01; // ver 字节
      corpus.push(t);
    }
    {
      let mut t = full1.clone();
      t[base_len + 2] ^= 0xFF; // payload 字节
      corpus.push(t);
    }
    // 正文伪魔数 + 合法尾
    {
      let mut t = b"MZ __WEB2APP_TAIL__ body".to_vec();
      t.extend_from_slice(p1.as_bytes());
      t.extend_from_slice(TAIL_MAGIC);
      t.extend_from_slice(&(p1.len() as u64).to_le_bytes());
      corpus.push(t);
    }
    // 长度字段篡改（v2 形态：含 0 / 极小 / 越界 / u64 极值）
    for lv in [0u64, 1, 23, 24, 25, u64::MAX, u64::MAX - 1, 1 << 40] {
      let mut t = base();
      t.extend_from_slice(p1.as_bytes());
      t.extend_from_slice(TAIL_MAGIC);
      t.extend_from_slice(&lv.to_le_bytes());
      corpus.push(t);
    }
    // 确定性 LCG 随机字节
    let mut seed: u64 = 0x1234_5678_9abc_def0;
    for _ in 0..200 {
      seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
      let len = (seed >> 20) as usize % 300;
      corpus.push((0..len).map(|i| (i as u8).wrapping_mul(31).wrapping_add((seed >> 8) as u8)).collect());
    }

    let dir = std::env::temp_dir().join("web2app_tail_corpus");
    fs::create_dir_all(&dir).unwrap();
    for (i, bytes) in corpus.iter().enumerate() {
      let f = dir.join(format!("c{i}.bin"));
      fs::write(&f, bytes).unwrap();
      let disk = match read_tail_from(&f) {
        Ok(None) => Cls::None,
        Ok(Some(c)) => Cls::Some(c.to_json()),
        Err(_) => Cls::Corrupt,
      };
      assert_eq!(disk, classify_mem(bytes), "分类漂移 @ {i} (len={})", bytes.len());
    }
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

  /// 确定性 LCG（零依赖、可复现）。对抗性 fuzz 的最小常驻形态：
  /// 曾在临时 fuzz 中以此输入族抓出 trim_matches 转义引号截断 bug，
  /// 固化为永久回归，守护手写 JSON 转义的每个边界。
  #[test]
  fn fuzz_roundtrip_adversarial_palette() {
    let palette: [&str; 13] = [
      "\"", "\\", "\n", "\r", "\t", "\u{1}", "\u{7F}",
      "中", "\u{10FFFF}", "\",\"icon\":\"", "\",\"title\":\"evil",
      "__WEB2APP_TAIL__", "a\"b\\c\nd",
    ];
    let mut seed: u64 = 0xC0FFEE;
    let mut next = || {
      seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
      (seed >> 33) as usize
    };
    for round in 0..200 {
      let mk = |next: &mut dyn FnMut() -> usize| -> String {
        let n = next() % 12;
        (0..n).map(|_| palette[next() % palette.len()]).collect()
      };
      let url = format!("https://{}.com/{}", mk(&mut next), mk(&mut next));
      let title = mk(&mut next);
      let icon = if next() % 2 == 0 {
        let len = next() % 48;
        Some((0..len).map(|i| (i.wrapping_mul(37).wrapping_add(11)) as u8).collect::<Vec<u8>>())
      } else {
        None
      };
      let cfg = AppConfig::new(url, title, icon);
      let parsed = AppConfig::from_json(&cfg.to_json());
      assert_eq!(parsed, Some(cfg), "adversarial roundtrip failed at round {round}");
    }
  }

  /// 协议帧边界对抗：随机字节、截断、长度篡改、正文伪魔数——零 panic 是底线。
  #[test]
  fn fuzz_tail_frame_never_panics() {
    let mut seed: u64 = 0xBEEF;
    let mut next = || {
      seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
      seed >> 11
    };
    // 随机字节流（含恰好 24 字节帧边界）
    for _ in 0..500 {
      let len = (next() % 300) as usize;
      let bytes: Vec<u8> = (0..len).map(|_| (next() % 256) as u8).collect();
      let _ = extract_tail(&bytes);
      let _ = strip_tail(&bytes);
    }
    // 合法帧 + 逐截断
    let cfg = AppConfig::new("https://x.io".into(), "x.io".into(), Some(vec![1, 2, 3, 255]));
    let payload = cfg.to_json();
    let mut frame = b"MZ exe body..........".to_vec();
    frame.extend_from_slice(payload.as_bytes());
    frame.extend_from_slice(TAIL_MAGIC);
    frame.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    for cut in 0..=frame.len() {
      let _ = extract_tail(&frame[..cut]);
    }
    // 长度字段篡改（含 u64 极值与越界值）
    for len_val in [0u64, 1, 23, 24, 25, u64::MAX, u64::MAX - 1, 1 << 32] {
      let mut b = b"MZ body".to_vec();
      b.extend_from_slice(payload.as_bytes());
      b.extend_from_slice(TAIL_MAGIC);
      b.extend_from_slice(&len_val.to_le_bytes());
      let _ = extract_tail(&b);
    }
  }

  #[test]
  fn read_tail_on_untailed_exe_is_none() {
    // 测试进程自身的 exe 无尾部配置 → Ok(None)（覆盖 read_tail 走 current_exe 的真实 I/O 路径）
    assert!(read_tail().unwrap().is_none());
  }

  #[test]
  fn unescape_preserves_invalid_escapes() {
    // 合法转义正常解码
    assert_eq!(unescape(r"\u0041"), "A");
    assert_eq!(unescape(r"a\nb"), "a\nb");
    // 非法 hex：原样保留，不静默丢字符
    assert_eq!(unescape(r"\uZZZZx"), r"\uZZZZx");
    // 孤立代理（char::from_u32 失败）：原样保留
    assert_eq!(unescape(r"\uD800"), r"\uD800");
    // 未知转义（如 \q）：保留源字符本身
    assert_eq!(unescape(r"\q"), "q");
  }
}
