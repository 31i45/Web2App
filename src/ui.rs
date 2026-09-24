//! ui.rs — 母版启动界面（内嵌 HTML，零外部资源文件）。
//!
//! 验收红线：界面仅 URL 输入框、打包按钮两个核心元素（logo 功能已裁剪）。

use crate::builder;

/// 母版表单页 HTML。全部样式内联，无网络请求、无外部依赖。
pub fn form_html() -> String {
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

  <button id="pack">打 包</button>
  <div class="status" id="status"></div>
</div>
<script>
  const $ = id => document.getElementById(id);

  $('pack').addEventListener('click', () => {
    const url = $('url').value.trim();
    if (!url) { $('status').style.color = '#ff8f8f'; $('status').textContent = '请输入 URL'; return; }
    $('status').style.color = '#7ee0a3';
    $('status').textContent = '打包中…';
    window.ipc.postMessage('pack:' + url);
  });

  // 回车提交
  $('url').addEventListener('keydown', ev => {
    if (ev.key === 'Enter') $('pack').click();
  });
</script>
</body>
</html>"#
  .to_string()
}

/// 从表单 URL 推导产物文件名：host → 安全文件名 + 平台后缀。
pub fn product_filename(url: &str) -> String {
  let host = builder::default_title(url)
    .chars()
    .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' })
    .collect::<String>();
  let base = if host.is_empty() { "web2app" } else { &host };
  format!("{base}.exe")
}
