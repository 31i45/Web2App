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

#[cfg(test)]
mod tests {
  use super::*;

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
  fn base64_known_vectors() {
    // RFC 4648 标准向量（覆盖 % 3 = 0/1/2 的 padding 分支）
    assert_eq!(base64_encode(b"Man"), "TWFu");
    assert_eq!(base64_encode(b"Ma"), "TWE=");
    assert_eq!(base64_encode(b"M"), "TQ==");
    assert_eq!(base64_encode(b""), "");
    assert_eq!(base64_decode("TWFu").unwrap(), b"Man");
    assert_eq!(base64_decode("TWE=").unwrap(), b"Ma");
    assert_eq!(base64_decode("TQ==").unwrap(), b"M");
    assert_eq!(base64_decode("").unwrap(), b"");
    // 解码忽略空白；非法输入返回 None
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
}
