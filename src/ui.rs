//! ui.rs — 母版启动界面（内嵌 HTML，零外部资源文件）。
//!
//! 验收红线：界面仅 URL 输入框、图标选择、打包按钮三个核心元素。
//! 图标选图用浏览器原生 input[type=file]+FileReader（W3C 标准，
//! 三平台同一实现），图片字节经 base64 由 IPC 传给原生，无需绝对路径。

use crate::builder;

/// 母版表单页 HTML（纯静态）。全部样式内联，无网络请求、无外部依赖。
pub fn form_html() -> &'static str {
  r#"<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<title>Web2App 打包器</title>
<style>
  * { box-sizing: border-box; margin: 0; padding: 0; }
  body {
    font-family: "Segoe UI", system-ui, sans-serif;
    background: #0f1117; color: #e6e8ee;
    display: flex; justify-content: center; align-items: center;
    height: 100vh; overflow: hidden;
  }
  .card {
    width: 520px; padding: 36px 40px;
    background: #171a23; border: 1px solid #262b38; border-radius: 14px;
    box-shadow: 0 10px 40px rgba(0,0,0,.45);
  }
  h1 { font-size: 22px; margin-bottom: 6px; }
  p.sub { color: #8b93a7; font-size: 13px; margin-bottom: 26px; }
  label { display: block; font-size: 13px; color: #aab2c5; margin: 14px 0 6px; }
  input[type=text] {
    width: 100%; padding: 11px 13px; font-size: 14px;
    background: #0f1117; color: #e6e8ee;
    border: 1px solid #2b3142; border-radius: 8px; outline: none;
  }
  input:focus { border-color: #4f8cff; }
  input[type=file] {
    width: 100%; padding: 9px 11px; font-size: 13px; color: #aab2c5;
    background: #0f1117; border: 1px solid #2b3142; border-radius: 8px; outline: none;
  }
  button {
    width: 100%; margin-top: 26px; padding: 13px;
    font-size: 15px; font-weight: 600; cursor: pointer;
    color: #fff; background: #2f6bff; border: none; border-radius: 9px;
  }
  button:hover { background: #2a5fe8; }
  .status { margin-top: 16px; font-size: 13px; color: #7ee0a3; min-height: 18px; word-break: break-all; }
</style>
</head>
<body>
<div class="card">
  <h1>Web2App 打包器</h1>
  <p class="sub">输入网页地址，生成单文件桌面应用（产物自带打包能力）</p>

  <label for="url">网页 URL</label>
  <input type="text" id="url" placeholder="https://example.com" autofocus>

  <label for="icon">应用图标（PNG，可选，缺省用 w2a）</label>
  <input type="file" id="icon" accept="image/png">

  <button id="pack">打 包</button>
  <div class="status" id="status"></div>
</div>
<script>
  const $ = id => document.getElementById(id);
  let iconB64 = '';

  // 选图：浏览器原生控件 + FileReader 读字节（无需绝对路径）
  $('icon').addEventListener('change', ev => {
    const f = ev.target.files[0];
    if (!f) { iconB64 = ''; return; }
    if (f.size > 2 * 1024 * 1024) {
      // 图标上限 2MB：base64 膨胀 ~33%，超过会显著推大产物体积
      iconB64 = '';
      ev.target.value = '';
      $('status').style.color = '#ff8f8f';
      $('status').textContent = '图标过大（>2MB），请换小图';
      return;
    }
    const r = new FileReader();
    r.onload = () => {
      // dataURL: data:image/png;base64,<payload>
      iconB64 = String(r.result).split(',')[1] || '';
      $('status').style.color = '#7ee0a3';
      $('status').textContent = '已选图标：' + f.name;
    };
    r.readAsDataURL(f);
  });

  $('pack').addEventListener('click', () => {
    // URL 清洗：剥除全部空白（含换行——防止伪造 IPC 的第二行图标字段）
    const url = $('url').value.replace(/\s+/g, '');
    if (!url) { $('status').style.color = '#ff8f8f'; $('status').textContent = '请输入 URL'; return; }
    $('status').style.color = '#7ee0a3';
    $('status').textContent = '打包中…';
    $('pack').disabled = true; // 防重复提交（set_status 回写时恢复）
    window.ipc.postMessage('pack:' + url + '\n' + iconB64);
  });

  // 回车提交
  $('url').addEventListener('keydown', ev => {
    if (ev.key === 'Enter' && !$('pack').disabled) $('pack').click();
  });
</script>
</body>
</html>"#
}

/// 从表单 URL 推导产物文件名：安全 host + 平台可执行后缀（Windows .exe / Unix 无扩展）。
pub fn product_filename(url: &str) -> String {
  let ext = if cfg!(target_os = "windows") { ".exe" } else { "" };
  format!("{}{}", builder::sanitize_host(url), ext)
}

/// 解析母版表单的 IPC 消息：`pack:<url>\n<base64 图标>`（图标行可缺失）。
///
/// 与 JS 端发送约定（`'pack:' + url + '\n' + iconB64`）配对；本函数是
/// Rust 侧协议契约的行为级规格持有者。返回 None = 非打包消息（忽略）。
/// URL 内空白由 JS 端剥除（防伪造第二行）；此处 splitn 使任何额外换行
/// 天然落入图标区，不会污染 URL 字段。
pub fn parse_pack_message(body: &str) -> Option<(String, Option<Vec<u8>>)> {
  let rest = body.strip_prefix("pack:")?;
  let mut lines = rest.splitn(2, '\n');
  let url = lines.next().unwrap_or("").to_string();
  let b64 = lines.next().unwrap_or("");
  let icon = if b64.is_empty() { None } else { crate::tail::base64_decode(b64) };
  Some((url, icon))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn form_html_contains_core_elements_only() {
    let h = form_html();
    assert!(h.contains(r#"id="url""#));
    assert!(h.contains(r#"id="icon""#));
    assert!(h.contains(r#"id="pack""#));
    assert!(h.contains(r#"id="status""#));
    // 选图走 FileReader 字节传输，不依赖文件绝对路径
    assert!(h.contains("readAsDataURL"));
  }

  #[test]
  fn form_html_hardened_against_injection_and_oversize() {
    let h = form_html();
    // URL 剥除全部空白（含换行）——阻断伪造 IPC 图标行
    assert!(h.contains("replace(/\\s+/g, '')"));
    // 图标 2MB 上限
    assert!(h.contains("2 * 1024 * 1024"));
    // 打包期间按钮禁用（防重复提交）
    assert!(h.contains("disabled = true"));
    assert!(h.contains("disabled"));
  }

  #[test]
  fn parse_pack_message_with_icon() {
    let msg = format!("pack:https://example.com\n{}", crate::tail::base64_encode(b"\x89PNG-icon"));
    let (url, icon) = parse_pack_message(&msg).unwrap();
    assert_eq!(url, "https://example.com");
    assert_eq!(icon.as_deref(), Some(b"\x89PNG-icon".as_slice()));
  }

  #[test]
  fn parse_pack_message_without_icon() {
    // 无图标：第二行为空 → icon None
    let (url, icon) = parse_pack_message("pack:https://example.com\n").unwrap();
    assert_eq!(url, "https://example.com");
    assert!(icon.is_none());
    // 完全无换行分隔（仅 URL）→ 同样 None
    let (url2, icon2) = parse_pack_message("pack:https://example.com").unwrap();
    assert_eq!(url2, "https://example.com");
    assert!(icon2.is_none());
  }

  #[test]
  fn parse_pack_message_rejects_non_pack() {
    assert!(parse_pack_message("hello:https://x.com\nQUJD").is_none());
    assert!(parse_pack_message("").is_none());
    assert!(parse_pack_message("pack").is_none());
  }

  #[test]
  fn parse_pack_message_invalid_base64_degrades_to_no_icon() {
    // 非法 base64 → 解码失败 → icon None（不阻塞打包，回落默认图标）
    let (url, icon) = parse_pack_message("pack:https://example.com\n!!!!invalid!!!!").unwrap();
    assert_eq!(url, "https://example.com");
    assert!(icon.is_none());
  }

  #[test]
  fn parse_pack_message_forged_second_line_stays_in_icon_field() {
    // 伪造防御规格：URL 若含换行（绕过 JS 剥空白），第二段永远归图标区，
    // URL 字段只取第一行——图标解析失败回落默认，URL 不被注入污染。
    let (url, icon) = parse_pack_message("pack:https://evil.com\nAAAAnot-base64###").unwrap();
    assert_eq!(url, "https://evil.com");
    assert!(icon.is_none());
  }
}
