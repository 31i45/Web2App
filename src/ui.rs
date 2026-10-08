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
}
