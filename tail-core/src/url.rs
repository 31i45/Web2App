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
}
