//! master/src/sign.rs — 可选代码签名 hook（非阻塞）。
//!
//! 签名是业务运营问题，不是架构职责。本模块提供一个轻量 hook：
//! 若环境变量 WEB2APP_SIGN_CERT 设置，则调用 signtool 签名产物；
//! 失败仅警告不阻塞（无证书 → SmartScreen 警告，但应用仍可运行）。
//!
//! 兼容本地 signtool（Windows SDK 自带）与远程签名服务（业务可替换本模块）。
//! 私钥永不内嵌进二进制——证书路径与密码通过环境变量在打包时传入。

use std::path::Path;
use std::process::Command;

/// 若 WEB2APP_SIGN_CERT 已设置，调用 signtool 签名产物；否则跳过。
///
/// 环境变量：
/// - `WEB2APP_SIGN_CERT`：证书文件路径（.pfx / .p12）。未设置 → 跳过签名。
/// - `WEB2APP_SIGN_PASS`：证书密码（可选，无密码证书可省略）。
/// - `WEB2APP_SIGN_TSA`：时间戳服务器 URL（可选，推荐设置以避免证书过期后签名失效）。
///
/// 失败返回 Err（调用方仅警告，不阻塞打包）。
pub fn maybe_sign(exe: &Path) -> Result<(), String> {
  let cert = match std::env::var("WEB2APP_SIGN_CERT") {
    Ok(c) => c,
    Err(_) => return Ok(()), // 未设置 → 跳过签名（非错误）
  };

  let mut cmd = Command::new("signtool");
  cmd.arg("sign")
    .arg("/f").arg(&cert)
    .arg("/fd").arg("sha256");

  if let Ok(pass) = std::env::var("WEB2APP_SIGN_PASS") {
    cmd.arg("/p").arg(&pass);
  }

  // /tr = RFC 3161 时间戳服务器（推荐）；/td 指定摘要算法
  if let Ok(tsa) = std::env::var("WEB2APP_SIGN_TSA") {
    cmd.arg("/tr").arg(&tsa).arg("/td").arg("sha256");
  }

  cmd.arg(exe);

  let output = cmd.output().map_err(|e| format!("signtool 启动失败：{e}"))?;
  if !output.status.success() {
    let stderr = String::from_utf8_lossy(&output.stderr);
    return Err(format!("signtool 签名失败：{stderr}"));
  }

  Ok(())
}
