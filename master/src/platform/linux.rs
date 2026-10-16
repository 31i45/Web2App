//! master/src/platform/linux.rs — Linux 平台外壳（master 侧）。
//!
//! 阶段 0b：按设计文档 §3.3 补齐与 Windows 侧**同名同签名**的项，使 Linux
//! 目标可编译（门禁 G-0b）。每处尚未落地的行为均以「阶段 1」标注，供 §7
//! T-0b-2 的接缝笔记汇总——0b 不预设计（ADR-3）。
//!
//! 与 Windows 侧各 `fn` 的签名逐字一致（缺一或签名不符即编译失败，这是刻意设计）。

use std::path::PathBuf;

// ============================================================================
// 数据目录（XDG Base Directory）
// ============================================================================

/// `<数据基目录>/Web2App/<sub>`：Linux 遵循 XDG Base Directory 规范。
///
/// 探测链（与 Windows 的 `%LOCALAPPDATA%` 同构）：`$XDG_DATA_HOME`（须为绝对路径，
/// 相对路径按规范应被忽略）→ `$HOME/.local/share` → 兜底 temp（极端环境不 panic）。
/// 品牌目录名 `Web2App` 与 Windows 侧一致，便于文档与排障对齐。
/// 隔离有效性（V-6）在阶段 1 T-1-3 由 `WebContext.data_directory` 实测确认。
pub(crate) fn data_dir(sub: &str) -> PathBuf {
  let base = std::env::var_os("XDG_DATA_HOME")
    .map(PathBuf::from)
    .filter(|p| p.is_absolute())
    .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
    .unwrap_or_else(std::env::temp_dir);
  base.join("Web2App").join(sub)
}

/// Unix 无 Win32 verbatim `\\?\` 前缀形态 → **恒等**返回。
///
/// 该函数是为 Windows 的 `current_exe()`（返回 `\\?\C:\...`）而存在的零依赖替代；
/// Unix 的 `current_exe()` 本就返回整洁路径，故此处不剥离任何字符。
pub(crate) fn strip_verbatim_prefix(p: PathBuf) -> PathBuf {
  p
}

// ============================================================================
// 致命错误呈现（stderr + 非零退出码）
// ============================================================================

/// 致命错误的呈现后退出（返回 `!`）。
///
/// Unix 侧不存在 Windows「GUI 子系统下 stderr 无人看」的问题——从终端或桌面启动器
/// 启动时 stderr 可见，故按 UNIX 惯例写 stderr 并 `exit(1)`。
/// 阶段 1 若需 GUI 弹窗（GTK dialog）再行接线。
pub(crate) fn fatal(msg: &str) -> ! {
  eprintln!("Web2App: {msg}");
  std::process::exit(1)
}

// ============================================================================
// 关闭确定性
// ============================================================================

/// no-op：看门狗是为绕开 Windows「WebView2 嵌套 COM 消息泵吞掉 WM_QUIT」而存在
/// （见 Windows 侧同名函数文档）。GTK/wry 无此结构性吞没，`ControlFlow::Exit`
/// 即确定退出，故 0b 不武装看门狗。阶段 1 T-1-1 实测若出现进程残留再按需补。
pub(crate) fn quit_watchdog() {}

/// no-op：关闭哨兵（WNDPROC 替换）是纯 Win32 机制，GTK 侧无对应物。
/// 参数仅用于与 Windows 侧同签名（签名见设计文档 §3.3 / 接缝笔记 T-0b-2）。
pub(crate) fn install_close_watchdog(_window: &tao::window::Window) {}

// ============================================================================
// 打包器平台项
// ============================================================================

/// 打包入口（与 Windows 侧同签名；Windows 侧即 `builder.rs` 编排骨架的 re-export）。
///
/// 阶段 1 T-1-4 接线：本函数是 `.deb` 封装的**唯一插入点**——先由共享编排骨架
/// [`crate::builder::pack`] 产出裸 ELF（模板 + 尾部配置），再用 `dpkg-deb` 连同
/// `control` / `.desktop` / hicolor 图标封装为 `.deb`（`Depends: libwebkit2gtk-4.1-0,
/// libgtk-3-0, gstreamer1.0-libav`，见设计文档 §6.2）。
///
/// 0b 直接委派给共享骨架：Linux 尚未发布，且 `.deb` 的目录布局/依赖清单需按 §6.2
/// 逐项落实（ADR-3 不预设计）。
pub(crate) fn pack(
  out_path: &std::path::Path,
  url: &str,
  icon_png: Option<Vec<u8>>,
) -> std::io::Result<crate::builder::BuildOutput> {
  crate::builder::pack(out_path, url, icon_png)
}

/// no-op（阶段 1 T-1-4 接线）：Linux 的文件图标走 `.deb` 的
/// `/usr/share/icons/hicolor/<sz>/apps/<name>.png`（由打包器写出），
/// 而非向可执行文件注入资源——ELF 无 `.rsrc` 概念。
///
/// 返回 `Ok(())` 而非 `Err`：这是「无此机制」的正确表达，不应在打包时打印
/// 「图标注入失败」的伪警告。用户所选 PNG 仍随尾部配置内嵌（见 `builder::pack`
/// 第 3 步），故运行时窗口图标不受影响。
pub(crate) fn inject_file_icon(_exe: &std::path::Path, _png: &[u8]) -> Result<(), String> {
  Ok(())
}

/// no-op hook（阶段 1 T-1-4 接线）：Linux 无 `signtool`。`.deb` 的完整性校验
/// （`Release.gpg` / `debsig`）属打包器与业务运营职责，非架构职责——本函数即
/// 业务侧替换点（与 Windows 的 `sign.rs` 同角色）。
pub(crate) fn maybe_sign(_exe: &std::path::Path) -> Result<(), String> {
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  /// 与 Windows 侧同名测试同构：品牌目录拼接 + 绝对路径（不 panic）。
  #[test]
  fn data_dir_lives_under_brand_and_is_absolute() {
    let s = data_dir("master").to_string_lossy().replace('\\', "/");
    assert!(s.ends_with("/Web2App/master"), "got: {s}");
    assert!(data_dir("master").is_absolute());
  }

  /// Unix 侧 `\\?\` 剥离为恒等：钉住跨平台契约（Windows 有剥离、Unix 无）。
  #[test]
  fn strip_verbatim_prefix_is_identity_on_unix() {
    for p in ["/tmp/foo/bar", "/root/.local/share/Web2App/master", "relative/path"] {
      assert_eq!(strip_verbatim_prefix(PathBuf::from(p)), PathBuf::from(p));
    }
    // Win32 verbatim 形态在 Unix 不是特殊前缀 → 同样恒等（不误剥离）
    let verbatim = PathBuf::from(r"\\?\C:\foo");
    assert_eq!(strip_verbatim_prefix(verbatim.clone()), verbatim);
  }
}
