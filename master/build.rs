//! master/build.rs — 定位 child 二进制并设置 CHILD_BIN_PATH。
//!
//! child 必须先于 master 构建（见 build.ps1：先 `cargo build -p child --release`）。
//! 定位策略（按序）：
//! 1. 从 OUT_DIR 上溯：OUT_DIR = `<target_dir>/<profile>/build/<pkg>-<hash>/out`，
//!    上溯 3 级即 profile 目录，child 二进制与其内 build 产物同级。此路径对
//!    `cargo build`（target/release）与 `--target <triple>`
//!    （target/<triple>/release）两种布局都成立。
//! 2. 历史兜底：workspace/target 下的 release / debug。
//!
//! 产物名按目标平台取（`CARGO_CFG_TARGET_OS`：Windows = `child.exe`，Unix = `child`）。
//! 找到后拷贝至 OUT_DIR/child.bin，设置 CHILD_BIN_PATH 供 src/builder.rs 的
//! `include_bytes!(env!("CHILD_BIN_PATH"))` 编译期内嵌。

use std::path::PathBuf;

fn main() {
  let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
  let child_bin = if target_os == "windows" { "child.exe" } else { "child" };

  let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set");
  // OUT_DIR/../../../<child_bin>（out → <pkg>-<hash> → build → <profile>）
  let via_out_dir = PathBuf::from(&out_dir)
    .ancestors()
    .nth(3)
    .map(|profile_dir| profile_dir.join(child_bin));

  let workspace_target = format!("{}/../target", env!("CARGO_MANIFEST_DIR"));
  let legacy_release = PathBuf::from(format!("{workspace_target}/release/{child_bin}"));
  let legacy_debug = PathBuf::from(format!("{workspace_target}/debug/{child_bin}"));

  let child: PathBuf = via_out_dir
    .into_iter()
    .chain([legacy_release, legacy_debug])
    .find(|p| p.exists())
    .unwrap_or_else(|| {
      panic!(
        "{child_bin} not found — run build.ps1 first (builds child before master); OUT_DIR={out_dir}"
      )
    });

  let dest = format!("{out_dir}/child.bin");
  std::fs::copy(&child, &dest).expect("failed to copy child binary to OUT_DIR/child.bin");

  println!("cargo:rustc-env=CHILD_BIN_PATH={dest}");
  println!("cargo:rerun-if-changed={}", child.display());

  // winres 仅 Windows 目标（build script 在 host 上运行，故判据取 CARGO_CFG_TARGET_OS）。
  if target_os != "windows" {
    return;
  }

  // 编译 w2a 图标 + 产品身份资源。
  // master 自身也需要文件图标与版本信息（此前缺失：Explorer 显示通用空白图标、
  // 且无 FileDescription）。与 child 使用同一 winres 路径，身份统一为 "Web2App"。
  let icon = format!("{}/../assets/w2a.ico", env!("CARGO_MANIFEST_DIR"));
  let mut res = winres::WindowsResource::new();
  res.set_icon(&icon);
  res.set("FileDescription", "Web2App");
  res.set("ProductName", "Web2App");
  // 事实身份：作者句柄与许可证均取自真实信息，不编造邮箱/公司
  res.set("CompanyName", "31i45 (https://github.com/31i45/Web2App)");
  res.set("LegalCopyright", "MIT License");
  if let Err(e) = res.compile() {
    println!("cargo:warning=winres failed: {e}");
  }
  println!("cargo:rerun-if-changed={icon}");
}
