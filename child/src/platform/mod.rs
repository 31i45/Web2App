//! child/src/platform/mod.rs — 平台外壳分发（同签名 `#[cfg]` 模块，**不用 trait**）。
//!
//! 每个平台模块必须提供同名同签名的项（缺一即编译失败，这是刻意设计）：
//! `data_dir` / `fatal` / `quit_watchdog` / `install_close_watchdog` / `install_keys` /
//! `shell_open` / `open_external_if_http` / `create_popup` / `navigate_popup` /
//! `close_all_popups` / `set_fullscreen`。
//! Windows 实现即原 `tail_core` 的 OS 触碰代码（阶段 0a 机械迁移）+ 原
//! `accelerator.rs` / `popup.rs` 的裸 Win32 管线（阶段 0b 迁移，零行为变更）。

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub(crate) use windows::*;

// Linux 实现见 platform/linux.rs（阶段 0b 由第二实现逼出接缝，见设计文档 §7 T-0b-1）。
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub(crate) use linux::*;
