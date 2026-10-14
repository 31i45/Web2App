//! tail-core — Web2App 共享层。
//!
//! 三方共用：尾部协议（child 读取运行配置；**写尾已外迁 `master/src/tail_write.rs`**，
//! 本 crate 只保留解析与 [`encode_tail`] 编码纯函数——写能力不在 child 的依赖图中，
//! 构成编译期结构性保证）、
//! PNG 解码与缩放（child 把用户图与 w2a 默认图转为像素，供 tao Icon 用）、
//! URL 工具（master 产物文件名与 child 数据目录共用 sanitize_host 单点持有）、
//! 致命错误呈现（Windows GUI 子系统下 stderr 无人看，必须 MessageBox）。
//!
//! 设计红线：零 tao/wry 依赖。本 crate 仅为「数据层」与「系统错误呈现」，
//! 任何引用窗口/WebView 类的代码不得引入此处（避免 child 产物体积膨胀）。
//! 窗口图标转换在 child/icon.rs（依赖 tao::window::Icon），此处只到像素字节为止。

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

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

/// 当前可执行文件路径（Windows 下手动剥离 `\\?\` verbatim 前缀，零依赖替代 dunce）。
///
/// 仅 `\\?\C:\...` 驱动器号形态被剥离为 `C:\...`；`\\?\UNC\server\share`
/// 转写为 `\\server\share`。无需 canonicalize：调用方（read_tail 自读、
/// master 的 dir_of_current_exe 写出目录）都不要求符号链接解析。
pub fn current_exe() -> io::Result<PathBuf> {
  let p = std::env::current_exe()?;
  Ok(strip_verbatim_prefix(p))
}

/// Windows verbatim `\\?\` 前缀剥离。
fn strip_verbatim_prefix(p: PathBuf) -> PathBuf {
  let s = match p.to_str() {
    Some(s) => s,
    None => return p, // 非 UTF-8 路径，原样返回（Windows 上罕见但保守）
  };
  if let Some(rest) = s.strip_prefix(r"\\?\") {
    // 驱动器号形态：\\?\C:\foo → C:\foo
    let b = rest.as_bytes();
    if b.len() >= 2 && b[1] == b':' {
      return PathBuf::from(rest);
    }
    // UNC 形态：\\?\UNC\server\share → \\server\share
    if let Some(unc_rest) = rest.strip_prefix("UNC\\") {
      return PathBuf::from(format!(r"\\{unc_rest}"));
    }
  }
  p
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

// ============================================================================
// 字符串 → JS 字面量（转义单点：child 注入脚本与 master 状态回写共用）
// ============================================================================

/// Rust 字符串 → JS 字符串字面量（含定界引号）。
///
/// 非 ASCII 原样输出（WebView2 的 JS 引擎按 UTF-8 解码注入源文本）。
/// 转义 `"` `\` 与全部控制字符，以及 U+2028/2029——后者是 JS 行终止符，
/// 直接放进字符串字面量会截断语句。
pub fn js_string_literal(s: &str) -> String {
  let mut out = String::with_capacity(s.len() + 2);
  out.push('"');
  for c in s.chars() {
    match c {
      '"' => out.push_str("\\\""),
      '\\' => out.push_str("\\\\"),
      '\n' => out.push_str("\\n"),
      '\r' => out.push_str("\\r"),
      '\t' => out.push_str("\\t"),
      '\u{2028}' | '\u{2029}' => out.push_str(&format!("\\u{:04x}", c as u32)),
      c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
      c => out.push(c),
    }
  }
  out.push('"');
  out
}

// ============================================================================
// base64（图标字节与 JSON 的互转，纯手写零依赖）
// ============================================================================

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

// ============================================================================
// PNG 解码与 box 缩放（窗口图标转换的纯数据层；tao::Icon 在 child/icon.rs）
// ============================================================================

/// PNG 像素总量上限（解压炸弹防御）：4096×4096 = 16.7M 像素 ≈ 67MB RGBA。
/// 图标场景远用不到；超限直接拒绝，避免「小文件大画布」拖垮内存。
const MAX_PIXELS: u64 = 4096 * 4096;

/// 尺寸是否在安全限内（含 u32 乘法溢出防御）。
fn dims_within_limit(w: u32, h: u32) -> bool {
  u64::from(w) * u64::from(h) <= MAX_PIXELS
}

/// PNG → RGBA8（仅 RGB/RGBA 且位深 8bit，其余返回 None；尺寸超限返回 None）。
pub fn decode_png(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
  let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
  let mut reader = decoder.read_info().ok()?;
  // 炸弹防御：read_info 已解析 IHDR，在分配像素缓冲前拒绝超限画布
  if !dims_within_limit(reader.info().width, reader.info().height) {
    return None;
  }
  // 位深必须为 8：png 解码器默认 Transformations::IDENTITY，16bit 不会自动降位深，
  // 若放行则 buffer 呈 8 字节/像素，却被按 4 字节/像素 RGBA 返回（尺寸语义自相矛盾）。
  if reader.info().bit_depth != png::BitDepth::Eight {
    return None;
  }
  let mut buf = vec![0u8; reader.output_buffer_size().unwrap_or(0)];
  let info = reader.next_frame(&mut buf).ok()?;
  let rgba = match info.color_type {
    png::ColorType::Rgba => buf[..info.buffer_size()].to_vec(),
    png::ColorType::Rgb => {
      let n = info.buffer_size();
      let mut out = Vec::with_capacity(n / 3 * 4);
      for px in buf[..n].as_chunks::<3>().0 {
        out.extend_from_slice(&[px[0], px[1], px[2], 0xFF]);
      }
      out
    }
    _ => return None,
  };
  Some((info.width, info.height, rgba))
}

/// box filter 缩放（目标像素 = 源对应矩形区域的平均值），任意比例通用。
pub fn resize_box(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
  if dw == 0 || dh == 0 || sw == 0 || sh == 0 {
    return Vec::new();
  }
  if dw == sw && dh == sh {
    return src.to_vec();
  }
  // u64 域运算后再截断为 usize：与 dims_within_limit 的溢出防御口径一致。
  let mut out = Vec::with_capacity((u64::from(dw) * u64::from(dh) * 4) as usize);
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

// ============================================================================
// URL 工具（master 产物文件名 + child 数据目录共用 sanitize_host 单点持有）
// ============================================================================

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

/// URL → host → 安全字符序列（单点持有：master 产物文件名与 child 数据目录共用）。
/// 非 `[A-Za-z0-9.-]` 字符替换为下划线。两类非法名被修正，否则写盘/建目录会失败：
/// - 结尾的点/空格：Windows 静默剥除，令实际落盘名与预期不符 —— 先剥除；
/// - `..` / `.`（路径遍历）与保留设备名（CON/PRN/AUX/NUL/COM1-9/LPT1-9）——
///   前者回落默认名，后者前置下划线破坏匹配（保留名判定取首个点之前的段，
///   故 `con.com` 亦命中）。
pub fn sanitize_host(url: &str) -> String {
  let s: String = default_title(url)
    .chars()
    .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' })
    .collect();
  let s = s.trim_end_matches(['.', ' ']);
  if s.is_empty() || s == ".." || s == "." {
    return "Web2App".to_string();
  }
  if is_reserved_device_name(s) {
    return format!("_{s}");
  }
  s.to_string()
}

/// 是否为 Windows 保留设备名（CON/PRN/AUX/NUL/COM1-9/LPT1-9，大小写不敏感）。
/// 判定取首个点之前的段：`con.com` 的段为 `con`，属保留。
fn is_reserved_device_name(s: &str) -> bool {
  let base = s.split('.').next().unwrap_or(s).to_ascii_uppercase();
  if matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL") {
    return true;
  }
  // COM/LPT 仅 1-9 为保留（COM0 合法）。
  if let Some(n) = base.strip_prefix("COM").or_else(|| base.strip_prefix("LPT")) {
    return matches!(n, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9");
  }
  false
}

/// URL → host（去协议前缀、userinfo 与端口），无 host 时返回空串。
///
/// 仅按 `://host[:port][/path]` 的宽松语法切分，不做完整 URL 解析
/// （尾部 payload 的 URL 由打包器产出，形态可控）。
pub fn host_of(url: &str) -> String {
  let rest = match url.split_once("://") {
    Some((_, rest)) => rest,
    // 无 scheme 定界（如 `mailto:` / 裸 host / 空串）不构成 `://host` 语法，无 host。
    None => return String::new(),
  };
  let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
  let host_port = authority.rsplit('@').next().unwrap_or(authority);
  let host = host_port.split(':').next().unwrap_or(host_port);
  host.to_ascii_lowercase()
}

/// URL → 稳定短指纹（8 位十六进制，FNV-1a 64 截断为低 32 位）。
///
/// 用途：数据目录按「host + URL 指纹」分区——同一 host 下打包的多个不同应用
/// （如 `https://a.com/x` 与 `https://a.com/y`）各自持有独立 profile，
/// cookie/存储/缓存互不污染。同一 URL 的多次打包共享同一 profile。
/// 片段标识（`#...`）不改变实际加载的页面，故先剥离再取指纹。
pub fn url_fingerprint(url: &str) -> String {
  let canonical = url.trim().split('#').next().unwrap_or("");
  let mut h: u64 = 0xcbf2_9ce4_8422_2325;
  for b in canonical.as_bytes() {
    h ^= u64::from(*b);
    h = h.wrapping_mul(0x0000_0100_0000_01b3);
  }
  format!("{:08x}", (h & 0xffff_ffff) as u32)
}

// ============================================================================
// 应用数据目录（Windows-only；child 数据目录与 master 数据目录共用同一探测链）
// ============================================================================

/// 系统应用数据基目录探测：%LOCALAPPDATA% → 兜底 temp。
///
/// Windows 上 LOCALAPPDATA 几乎总存在；缺失（极端环境）回落 temp 保证不 panic。
/// 单点持有：child 的 WebView 数据目录与 master 的数据目录共用本函数。
pub fn data_base_dir() -> PathBuf {
  std::env::var_os("LOCALAPPDATA")
    .map(PathBuf::from)
    .unwrap_or_else(std::env::temp_dir)
}

/// `<数据基目录>/Web2App/<sub>`：品牌目录与子系统名的拼接收敛于此，
/// 保证 child（apps）与 master（master）落在同一棵树下，且互不污染 exe 目录。
pub fn data_dir(sub: &str) -> PathBuf {
  data_base_dir().join("Web2App").join(sub)
}

// ============================================================================
// 致命错误呈现（Windows-only，无 Unix 分支）
// ============================================================================

/// 致命错误的用户可见呈现后退出（返回 `!`）。
///
/// 动机：release 为 Windows GUI 子系统且 `panic = "abort"`，`eprintln` 无人看见、
/// panic 无输出——错误必须以用户可感知的方式呈现，否则双击即"静默死亡"。
/// Windows 用 MessageBoxW（`extern` 直接声明，零新依赖）。
pub fn show_fatal_and_exit(msg: &str) -> ! {
  #[link(name = "user32")]
  extern "system" {
    fn MessageBoxW(
      hwnd: *mut core::ffi::c_void,
      text: *const u16,
      caption: *const u16,
      utype: u32,
    ) -> i32;
  }
  const MB_OK: u32 = 0x0;
  const MB_ICONERROR: u32 = 0x10;
  let text: Vec<u16> = msg.encode_utf16().chain(std::iter::once(0)).collect();
  let caption: Vec<u16> = "Web2App".encode_utf16().chain(std::iter::once(0)).collect();
  unsafe {
    MessageBoxW(
      std::ptr::null_mut(),
      text.as_ptr(),
      caption.as_ptr(),
      MB_OK | MB_ICONERROR,
    );
  }
  std::process::exit(1)
}

/// 关闭看门狗：300ms 后若进程仍存活，以退出码 0 结束进程。
///
/// 与 tao 事件处理里的 `ControlFlow::Exit` 配对使用。实测（tao 0.37 +
/// wry 0.57/WebView2）：单条 WM_CLOSE 路径下（Alt+F4、任务栏「关闭窗口」、
/// taskkill 无 /F、UI 自动化），即使投递 WM_QUIT 也会被 Chromium 嵌套 COM
/// 消息泵在 tao 外层 GetMessage 之前吞掉——进程要等下一条消息才退出；
/// 鼠标点 X 因伴随消息风暴通常无感，但「关闭即退出」不能依赖运气。
///
/// 看门狗在独立线程睡眠，不参与消息泵竞争，给出 300ms 的确定退出上限。
/// 正常路径几乎总先于它结束（tao run() 末尾自身即 process::exit，本就不做
/// 栈展开），线程随进程消失，零副作用；仅在框架卡死时兜底。WebView2 按
/// 浏览器进程被杀的常态设计，硬退出不损坏数据目录。
///
/// 幂等：多次调用仅多 spawn 无害线程，exit 先到先得。配套：
/// [`arm_close_watchdog`]——在 WNDPROC 层面保证本函数必然被武装。
pub fn quit_watchdog() {
  std::thread::spawn(|| {
    std::thread::sleep(std::time::Duration::from_millis(300));
    std::process::exit(0);
  });
}

// 已武装窗口的「原窗口过程」注册表（hwnd → prev WNDPROC）。
//
// 按 hwnd 分槽而非全局单值：进程内可能存在多个窗口（主窗口 + 新窗口路由
// 产生的弹窗），单槽会在第二次武装时覆盖前一个窗口的链尾，令其哨兵调用错
// 窗口过程。窗口消息只在创建窗口的线程派发，故 thread_local 足够，
// 无需全局锁与跨线程可见性。
thread_local! {
  static PREV_WNDPROCS: RefCell<HashMap<isize, isize>> = RefCell::new(HashMap::new());
}

/// 在窗口上安装关闭哨兵：替换窗口过程，捕获 WM_CLOSE 并武装 [`quit_watchdog()`]。
///
/// 动机（实测 tao 0.37 + wry 0.57/WebView2）：单条 WM_CLOSE 路径下
/// （Process.CloseMainWindow、Alt+F4、任务栏「关闭窗口」、taskkill 无 /F），
/// tao 的 CloseRequested 事件可能**永不派发**——消息被 WebView2 嵌套 COM
/// 消息泵吞没，进程 30s+ 不自退（本仓库 e2e 实测复现）。事件层无法自救，
/// 必须在消息进入框架之前拦截。
///
/// 本函数在 WNDPROC 层面捕获 WM_CLOSE——先于一切框架事件派发，不受消息泵
/// 吞没影响——收到即武装看门狗（300ms 确定退出），随后无条件
/// `CallWindowProcW` 透传给原过程，正常事件链完全不受影响。
///
/// 调用约束：
/// - 必须在**创建 WebView 之前**调用：wry 挂接其 subclass 时以当前 WNDPROC
///   为链尾，先挂哨兵可保证 wry 的 WM_SIZE/WM_SETFOCUS 等处理仍在链上。
/// - 须在窗口所属线程调用（child/master 均在主线程窗口创建后立即调用）。
///
/// 无竞态：挂接发生在事件循环启动前、hwnd 尚未暴露给任何其他线程，
/// 消息不可能在 store 原过程之前到达哨兵 proc。
pub fn arm_close_watchdog(hwnd: isize) {
  #[link(name = "user32")]
  extern "system" {
    fn SetWindowLongPtrW(hwnd: isize, index: i32, new_value: isize) -> isize;
    fn CallWindowProcW(prev: isize, hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize;
    fn DefWindowProcW(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize;
  }
  const GWLP_WNDPROC: i32 = -4;
  const WM_CLOSE: u32 = 0x0010;

  unsafe extern "system" fn close_watch_proc(
    hwnd: isize,
    msg: u32,
    wparam: usize,
    lparam: isize,
  ) -> isize {
    if msg == WM_CLOSE {
      quit_watchdog();
    }
    let prev = PREV_WNDPROCS.with(|m| m.borrow().get(&hwnd).copied()).unwrap_or(0);
    if prev == 0 {
      // 未登记（武装失败或已被替换）：退化为默认处理，避免向空过程转发。
      return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    unsafe { CallWindowProcW(prev, hwnd, msg, wparam, lparam) }
  }

  // SAFETY: hwnd 为调用方持有的有效窗口句柄。SetWindowLongPtrW 原子换入
  // 哨兵过程并返回原过程；失败返回 0 时不保存（哨兵永不生效，行为等同
  // 未安装，退化为仅 tao handler 路径的看门狗）。
  let prev =
    unsafe { SetWindowLongPtrW(hwnd, GWLP_WNDPROC, close_watch_proc as *const () as isize) };
  if prev != 0 {
    PREV_WNDPROCS.with(|m| {
      m.borrow_mut().insert(hwnd, prev);
    });
  }
}

// ============================================================================
// 测试（44 个：tail 协议 25 + PNG/缩放 6 + URL 工具 6 + 边界补充 7）
// ============================================================================

#[cfg(test)]
mod tests {
  use super::*;

  // ---------- tail.rs 测试（25 个） ----------

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

  // ---------- icon.rs PNG/缩放 测试（6 个） ----------

  /// 测试用 PNG 构造器（仅测试需要编码能力，cfg 收敛避免 dead_code）。
  fn encode_png(w: u32, h: u32, rgba: &[u8]) -> Option<Vec<u8>> {
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
  fn decode_png_rgb_expands_to_rgba() {
    // RGB PNG → RGBA（补 0xFF alpha）
    let mut out = Vec::new();
    {
      let mut enc = png::Encoder::new(&mut out, 4u32, 1u32);
      enc.set_color(png::ColorType::Rgb);
      enc.set_depth(png::BitDepth::Eight);
      let mut writer = enc.write_header().unwrap();
      writer.write_image_data(&[10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120]).unwrap();
    }
    let (w, h, rgba) = decode_png(&out).unwrap();
    assert_eq!((w, h), (4, 1));
    assert_eq!(rgba, vec![10, 20, 30, 255, 40, 50, 60, 255, 70, 80, 90, 255, 100, 110, 120, 255]);
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
  fn dims_within_limit_rejects_bomb() {
    // 正常图标尺寸
    assert!(dims_within_limit(256, 256));
    assert!(dims_within_limit(4096, 4096)); // 恰好等于上限
    // 超限（炸弹画布）：5000×5000 = 25M 像素
    assert!(!dims_within_limit(5000, 5000));
    assert!(!dims_within_limit(30000, 30000));
    // 极端长宽比：u32 乘法溢出防御
    assert!(!dims_within_limit(u32::MAX, u32::MAX));
    assert!(!dims_within_limit(u32::MAX, 2));
    // 1×u32::MAX 不溢出 u64 但超限
    assert!(!dims_within_limit(1, u32::MAX));
    // 零尺寸：0 像素 ≤ 上限（后续 window_icon 另行拒绝）
    assert!(dims_within_limit(0, 0));
  }

  // ---------- builder.rs URL 工具 测试（6 个） ----------

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
  fn sanitize_host_rejects_dot_path_components() {
    // 路径组件安全：host 恰为 .. / . 时会令数据目录上移或指向当前目录
    // （手工构造的尾部配置可触发），必须回落默认名而非原样输出。
    assert_eq!(sanitize_host("https://.."), "Web2App");
    assert_eq!(sanitize_host("https://."), "Web2App");
    // 恰好是两个点以外的点形态不构成路径遍历（a..b / ... 均为普通目录名）
    assert_eq!(sanitize_host("https://a..b"), "a..b");
    assert_eq!(sanitize_host("https://...com"), "...com");
  }

  #[test]
  fn host_of_strips_scheme_port_userinfo_and_path() {
    assert_eq!(host_of("https://example.com/x?q=1#frag"), "example.com");
    assert_eq!(host_of("http://a.b.org:8080/p"), "a.b.org");
    assert_eq!(host_of("https://user:pw@Example.COM/p"), "example.com");
    assert_eq!(host_of("mailto:x@y.com"), "");
    assert_eq!(host_of(""), "");
  }

  #[test]
  fn url_fingerprint_is_stable_and_distinguishes_urls() {
    // 同输入 → 同输出（稳定）；格式恒为 8 位小写十六进制
    let a = url_fingerprint("https://example.com");
    assert_eq!(a, url_fingerprint("https://example.com"));
    assert_eq!(a.len(), 8);
    assert!(a.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    // 同 host 不同路径 / 不同 query 必须区分（数据目录隔离的前提）
    assert_ne!(a, url_fingerprint("https://example.com/other"));
    assert_ne!(url_fingerprint("https://a.com/x?p=1"), url_fingerprint("https://a.com/x?p=2"));
    // 首尾空白与片段不改变指纹（片段不影响实际加载的页面）
    assert_eq!(url_fingerprint(" https://example.com/ "), url_fingerprint("https://example.com/#top"));
  }

  // ---------- 边界补充测试（7 个：路径 / 转义 / 位深 / 保留名 / 数据目录） ----------

  #[test]
  fn strip_verbatim_prefix_handles_win32_forms() {
    // 驱动器号形态：\\?\C:\foo → C:\foo
    assert_eq!(strip_verbatim_prefix(PathBuf::from(r"\\?\C:\foo\bar")), PathBuf::from(r"C:\foo\bar"));
    // UNC 形态：\\?\UNC\server\share → \\server\share
    assert_eq!(strip_verbatim_prefix(PathBuf::from(r"\\?\UNC\srv\share")), PathBuf::from(r"\\srv\share"));
    // 无前缀：原样返回
    assert_eq!(strip_verbatim_prefix(PathBuf::from(r"C:\plain")), PathBuf::from(r"C:\plain"));
    // 仅 `\\?\` 但既非驱动器也非 UNC：保守原样返回
    assert_eq!(strip_verbatim_prefix(PathBuf::from(r"\\?\weird")), PathBuf::from(r"\\?\weird"));
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

  #[test]
  fn js_string_literal_escapes_literals() {
    assert_eq!(js_string_literal("plain"), "\"plain\"");
    assert_eq!(js_string_literal("a\"b"), "\"a\\\"b\"");
    assert_eq!(js_string_literal("a\\b\n"), "\"a\\\\b\\n\"");
    assert_eq!(js_string_literal("中"), "\"中\"");
    // U+2028/2029 是 JS 行终止符，必须转义
    assert_eq!(js_string_literal("\u{2028}"), "\"\\u2028\"");
  }

  #[test]
  fn decode_png_rejects_sixteen_bit() {
    // 16bit RGBA：png 解码器默认不降位深 → 必须拒绝（否则 buffer 呈 8 字节/像素，
    // 却被按 4 字节/像素 RGBA 返回，尺寸语义自相矛盾）。
    let mut out = Vec::new();
    {
      let mut enc = png::Encoder::new(&mut out, 2u32, 2u32);
      enc.set_color(png::ColorType::Rgba);
      enc.set_depth(png::BitDepth::Sixteen);
      let mut w = enc.write_header().unwrap();
      w.write_image_data(&[0u8; 2 * 2 * 4 * 2]).unwrap();
    }
    assert!(decode_png(&out).is_none());
  }

  #[test]
  fn sanitize_host_avoids_reserved_device_names() {
    // 保留设备名（判定取首个点之前的段）：前置下划线破坏匹配
    assert_eq!(sanitize_host("https://con.com"), "_con.com");
    assert_eq!(sanitize_host("https://nul"), "_nul");
    assert_eq!(sanitize_host("https://COM1.dev"), "_COM1.dev");
    assert_eq!(sanitize_host("https://lpt9.io"), "_lpt9.io");
    // COM0 非保留
    assert_eq!(sanitize_host("https://com0.org"), "com0.org");
    // 结尾点（Windows 会静默剥除）：先剥除，落盘名与预期一致
    assert_eq!(sanitize_host("https://example.com."), "example.com");
  }

  #[test]
  fn data_dir_lives_under_brand_and_is_absolute() {
    // 探测链不 panic 且返回绝对路径（LOCALAPPDATA 或 temp 均为绝对路径）
    assert!(data_base_dir().is_absolute());
    let s = data_dir("apps").to_string_lossy().replace('\\', "/");
    assert!(s.ends_with("/Web2App/apps"), "got: {s}");
  }
}
