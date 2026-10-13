//! tail-core — Web2App 共享层。
//!
//! 三方共用：尾部协议（child 读取运行配置 / master 写入打包配置）、
//! PNG 解码与缩放（child 把用户图与 w2a 默认图转为像素，供 tao Icon 用）、
//! URL 工具（master 产物文件名与 child 数据目录共用 sanitize_host 单点持有）、
//! 致命错误呈现（Windows GUI 子系统下 stderr 无人看，必须 MessageBox）。
//!
//! 设计红线：零 tao/wry 依赖。本 crate 仅为「数据层」与「系统错误呈现」，
//! 任何引用窗口/WebView 类的代码不得引入此处（避免 child 产物体积膨胀）。
//! 窗口图标转换在 child/icon.rs（依赖 tao::window::Icon），此处只到像素字节为止。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

// ============================================================================
// 尾部协议：尾部追加式单文件配置
// ============================================================================

/// 尾部魔数。选 16 字节保证足够独特、极低误撞率。
pub const TAIL_MAGIC: &[u8; 16] = b"__WEB2APP_TAIL__";

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

/// PNG → RGBA8（仅 RGB/RGBA 8bit，其余返回 None；尺寸超限返回 None）。
pub fn decode_png(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
  let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
  let mut reader = decoder.read_info().ok()?;
  // 炸弹防御：read_info 已解析 IHDR，在分配像素缓冲前拒绝超限画布
  if !dims_within_limit(reader.info().width, reader.info().height) {
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
  let mut out = Vec::with_capacity((dw * dh * 4) as usize);
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
/// 非 `[A-Za-z0-9.-]` 字符替换为下划线。
/// 恰为 `..` / `.` 的段回落默认名：此类 host（仅手工伪造的尾部可产生）
/// 会令数据目录上移或指向当前目录，构成路径遍历面。
pub fn sanitize_host(url: &str) -> String {
  let s: String = default_title(url)
    .chars()
    .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' })
    .collect();
  if s == ".." || s == "." {
    "Web2App".to_string()
  } else {
    s
  }
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

/// 原窗口过程（tao 的），挂接哨兵时保存。单窗口进程故全局单值。
static PREV_WNDPROC: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

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
/// `CallWindowProcW` 透传给 tao 原过程，正常事件链完全不受影响。
///
/// 调用约束：
/// - 必须在**创建 WebView 之前**调用：wry 挂接其 subclass 时以当前 WNDPROC
///   为链尾，先挂哨兵可保证 wry 的 WM_SIZE/WM_SETFOCUS 等处理仍在链上。
/// - 须在窗口所属线程调用（child/master 均在主线程窗口创建后立即调用）。
/// - 面向单窗口进程设计（PREV_WNDPROC 为全局单值）。
///
/// 无竞态：挂接发生在事件循环启动前、hwnd 尚未暴露给任何其他线程，
/// 消息不可能在 store 原过程之前到达哨兵 proc。
pub fn arm_close_watchdog(hwnd: isize) {
  #[link(name = "user32")]
  extern "system" {
    fn SetWindowLongPtrW(hwnd: isize, index: i32, new_value: isize) -> isize;
    fn CallWindowProcW(prev: isize, hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize;
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
    let prev = PREV_WNDPROC.load(std::sync::atomic::Ordering::Acquire);
    CallWindowProcW(prev, hwnd, msg, wparam, lparam)
  }

  // SAFETY: hwnd 为调用方持有的有效窗口句柄。SetWindowLongPtrW 原子换入
  // 哨兵过程并返回原过程；失败返回 0 时不保存（哨兵永不生效，行为等同
  // 未安装，退化为仅 tao handler 路径的看门狗）。
  let prev =
    unsafe { SetWindowLongPtrW(hwnd, GWLP_WNDPROC, close_watch_proc as *const () as isize) };
  if prev != 0 {
    PREV_WNDPROC.store(prev, std::sync::atomic::Ordering::Release);
  }
}

// ============================================================================
// 测试（30 个：tail 协议 20 + PNG/缩放 6 + URL 工具 4）
// ============================================================================

#[cfg(test)]
mod tests {
  use super::*;

  // ---------- tail.rs 测试（20 个） ----------

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

  // ---------- builder.rs URL 工具 测试（4 个） ----------

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
}
