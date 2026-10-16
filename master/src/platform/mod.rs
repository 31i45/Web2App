//! master/src/platform/mod.rs — 平台外壳分发（同签名 `#[cfg]` 模块，**不用 trait**）。
//!
//! 每个平台模块必须提供同名同签名的项（缺一即编译失败，这是刻意设计）：
//! `data_dir` / `strip_verbatim_prefix` / `fatal` / `quit_watchdog` /
//! `install_close_watchdog`，以及打包器平台项 `pack` / `inject_file_icon` /
//! `maybe_sign`（`pack` 为 §3.3 明列的 master 侧平台项；Windows 侧即
//! `builder.rs` 的编排骨架 re-export，见 platform/windows.rs）。
//! Windows 实现即原 `tail_core` 的 OS 触碰代码（阶段 0a 机械迁移，零行为变更）
//! 与原 `winicon.rs` / `sign.rs`（阶段 0b 经 re-export 汇入本外壳）。

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub(crate) use windows::*;

// Linux 实现见 platform/linux.rs（阶段 0b 由第二实现逼出接缝，见设计文档
// §7 T-0b-1；`pack` 的 `.deb` 打包属阶段 1 T-1-4）。
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub(crate) use linux::*;
