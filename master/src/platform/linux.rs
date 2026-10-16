//! master/src/platform/linux.rs — Linux 平台外壳（master 侧）。
//!
//! 阶段 0b 补齐与 Windows 侧**同名同签名**的外壳项；阶段 1 T-1-4 落地 `.deb`
//! 打包（§6.2 原生 dpkg-deb 链路）与表单 WebView 挂载修复（`mount_webview`）。

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
// 打包器平台项（阶段 1 T-1-4：`.deb` 原生链路）
// ============================================================================
//
// 产物链（§6.2）：`builder::pack`（共享编排骨架，§4.1 禁止改写）产出「模板 ELF +
// 尾部配置」→ 组装 staging 目录（DEBIAN/control + usr/bin + .desktop + hicolor +
// doc）→ `dpkg-deb --build --root-owner-group` 原生封装。**不手搓 ar/tar**——
// Debian 打包的全部格式规则交给系统 dpkg-deb（不造轮子）。

use std::io;
use std::path::Path;
use std::process::Command;

/// Debian 包名 = `<host>`（用户决定：安装/卸载与文件名同名，不加品牌前缀）。
///
/// dpkg 包名规则 `[a-z0-9][a-z0-9+.-]+` 允许点分形态（`baidu.com` 合法，
/// 与系统包重名实际不可能——Ubuntu 包名无完整域名形态）。sanitize_host 的
/// 下划线映射为连字符、大写小写化；仅当结果不以字母数字开头（首个标签非
/// ASCII 的 host，sanitize 成连字符开头）时 dpkg 拒绝 → 回退 `web2app-`
/// 前缀保名称合规且自解释。
/// `pub(crate)`：产物文件名（`ui::product_filename`）复用同一函数——
/// 文件名 = 包名 + `.deb`，`apt install ./<文件名>` 与 `apt remove <包名>`
/// 同名贯穿（实测踩坑：文件名与包名不一致，卸载时 `dpkg -r <文件名>` 报
/// 「illegal package name」）。
pub(crate) fn package_name(url: &str) -> String {
  let n = tail_core::sanitize_host(url).to_lowercase().replace('_', "-");
  if n.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit()) {
    n
  } else {
    format!("web2app-{n}")
  }
}

/// Rust target arch → Debian architecture 名。未知架构显式报错——静默错档
/// 会产出装不上的包。
fn dpkg_arch() -> Result<&'static str, String> {
  match std::env::consts::ARCH {
    "x86_64" => Ok("amd64"),
    "aarch64" => Ok("arm64"),
    other => Err(format!("unsupported architecture: {other}")),
  }
}

/// DEBIAN/control 文本。Debian Policy §5.2 必填五字段；Depends 为设计文档 §6.2
/// 依赖集（Web 运行库 + H.264/AAC 解码 + WebVTT 字幕/fakevideosink 由
/// gst-plugins-bad 提供，见 §5.3 B 级披露）。Maintainer 用项目仓库地址
/// （符合事实，不编造邮箱）。Description 取单行摘要（合法且不引入续行缩进的脆弱性）。
fn control_text(pkg: &str, arch: &str, url: &str) -> String {
  format!(
    "Package: {pkg}\n\
     Version: {}\n\
     Architecture: {arch}\n\
     Maintainer: Web2App Packager <https://github.com/31i45/Web2App>\n\
     Depends: libwebkit2gtk-4.1-0, libgtk-3-0, gstreamer1.0-libav, gstreamer1.0-plugins-bad\n\
     Description: Web2App desktop app for {} (system WebKitGTK, no bundled browser)\n",
    env!("CARGO_PKG_VERSION"),
    tail_core::default_title(url),
  )
}

/// XDG `.desktop` 文本（Desktop Entry Specification 必填项）。`Name` 与子应用
/// 窗口标题同源（`default_title`），`Exec`/`Icon` 与包内路径一致。
fn desktop_text(pkg: &str, url: &str) -> String {
  format!(
    "[Desktop Entry]\n\
     Type=Application\n\
     Name={}\n\
     Exec=/usr/bin/{pkg}\n\
     Icon={pkg}\n\
     Terminal=false\n\
     Categories=Network;\n",
    tail_core::default_title(url),
  )
}

/// 打包入口（与 Windows 侧同签名）：先由共享编排骨架产出裸 ELF（模板 + 尾部，
/// INV-1 配置与二进制合一），再经 dpkg-deb 封装为 `.deb`。
///
/// 中间物（裸 ELF、staging 目录）一律落系统 temp，产物目录只出现最终 `.deb`；
/// 无论成败全部清理（与 Windows 侧 `pack` 的零残渣约定一致）。
pub(crate) fn pack(
  out_path: &Path,
  url: &str,
  icon_png: Option<Vec<u8>>,
) -> std::io::Result<crate::builder::BuildOutput> {
  let elf_tmp = out_path.with_extension("tmp.elf");
  let staging = std::env::temp_dir().join(format!("w2a-deb-{}", package_name(url)));
  let result = build_deb(out_path, url, icon_png, &elf_tmp, &staging);
  let _ = std::fs::remove_file(&elf_tmp);
  let _ = std::fs::remove_dir_all(&staging);
  result
}

fn build_deb(
  out_path: &Path,
  url: &str,
  icon_png: Option<Vec<u8>>,
  elf_tmp: &Path,
  staging: &Path,
) -> std::io::Result<crate::builder::BuildOutput> {
  use std::os::unix::fs::PermissionsExt;

  let pkg = package_name(url);
  let arch = dpkg_arch()
    .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;

  // 1) staging 布局（§6.2 payload：/usr/bin + applications + hicolor + doc）
  //    与 ELF 产出互不依赖，先组装（图标在此消费 icon_png 的借用）
  let bin = staging.join("usr/bin");
  let apps = staging.join("usr/share/applications");
  let icons = staging.join("usr/share/icons/hicolor/256x256/apps");
  let doc = staging.join(format!("usr/share/doc/{pkg}"));
  for d in [&bin, &apps, &icons, &doc] {
    std::fs::create_dir_all(d)?;
  }
  std::fs::create_dir_all(staging.join("DEBIAN"))?;
  std::fs::write(apps.join(format!("{pkg}.desktop")), desktop_text(&pkg, url))?;
  let icon: &[u8] = match &icon_png {
    Some(png) => png,
    None => include_bytes!("../../../assets/logo_256.png"),
  };
  std::fs::write(icons.join(format!("{pkg}.png")), icon)?;
  std::fs::write(doc.join("copyright"), "Web2App Packager <https://github.com/31i45/Web2App>\nMIT License\n")?;
  std::fs::write(staging.join("DEBIAN/control"), control_text(&pkg, arch, url))?;

  // 2) 共享编排骨架：模板 + 尾部配置 → 裸 ELF（签名/图标注入的 hook 点同 Windows）
  crate::builder::pack(elf_tmp, url, icon_png)?;

  // 3) payload：ELF+tail（755）
  let installed_bin = bin.join(&pkg);
  std::fs::copy(elf_tmp, &installed_bin)?;
  std::fs::set_permissions(&installed_bin, std::fs::Permissions::from_mode(0o755))?;

  // 4) 原生封装：dpkg-deb --build（--root-owner-group 使包内文件归 root，
  //    与 root 安装的运行时一致；打包进程无需 root）
  let out = Command::new("dpkg-deb")
    .arg("--build")
    .arg("--root-owner-group")
    .arg(staging)
    .arg(out_path)
    .output()?;
  if !out.status.success() {
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    return Err(io::Error::other(format!("dpkg-deb --build 失败：{stderr}")));
  }

  Ok(crate::builder::BuildOutput {
    exe: out_path.to_path_buf(),
    size: std::fs::metadata(out_path)?.len(),
  })
}

// ============================================================================
// WebView 挂载（child 侧同名接缝项 D1 的 master 版：Linux 必须走 build_gtk）
// ============================================================================

/// 把表单 WebView 挂到窗口——Linux 走 `build_gtk`（理由与实现同 child 侧
/// `platform::mount_webview`：wry webkitgtk 后端通用 `build()` 只接受 Xlib 句柄，
/// Wayland 会话下直接 `UnsupportedWindowHandle`；GTK 容器路径与 GDK 后端无关）。
pub(crate) fn mount_webview<'a>(
  builder: wry::WebViewBuilder<'a>,
  window: &'a tao::window::Window,
) -> Result<wry::WebView, wry::Error> {
  use tao::platform::unix::WindowExtUnix;
  use wry::WebViewBuilderExtUnix;
  match window.default_vbox() {
    Some(vbox) => builder.build_gtk(vbox),
    None => builder.build(window),
  }
}

/// no-op：Linux 的文件图标走 `.deb` 的
/// `/usr/share/icons/hicolor/256x256/apps/<name>.png`（由 [`pack`] 写出），
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

  /// dpkg 包名规则（禁下划线/大写、字母数字开头）：包名 = host 本身
  /// （不加品牌前缀）；仅首字符非字母数字的退化 host 回退 `web2app-` 前缀。
  /// `127.0.0.1:8100`（本地站）的下划线转连字符是典型样例。
  #[test]
  fn package_name_is_dpkg_safe() {
    assert_eq!(package_name("https://example.com"), "example.com");
    assert_eq!(package_name("http://127.0.0.1:8100"), "127.0.0.1-8100");
    assert_eq!(package_name("https://EXAMPLE.com/path"), "example.com");
    // 非 ASCII 首标签 → sanitize 为连字符开头（dpkg 拒绝）→ 回退品牌前缀
    assert_eq!(package_name("https://中文站.com"), "web2app----.com");
    // 合规形态一律无前缀，且逐字符满足 dpkg 规则
    assert!(!package_name("https://a-b.c.org").starts_with("web2app-"));
    assert!(package_name("https://a-b.c.org")
      .chars()
      .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.'));
  }

  /// control 五必填字段 + §6.2 依赖集逐行钉住（dpkg-deb 的解析以此为契约）。
  #[test]
  fn control_text_contains_policy_fields() {
    let t = control_text("web2app-example.com", "amd64", "https://example.com");
    let lines: Vec<&str> = t.lines().collect();
    assert!(lines[0].starts_with("Package: web2app-example.com"));
    assert!(lines.iter().any(|l| l.starts_with("Version: ")));
    assert!(lines.contains(&"Architecture: amd64"));
    assert!(lines.iter().any(|l| l.starts_with("Maintainer: ")));
    assert!(lines.contains(&"Depends: libwebkit2gtk-4.1-0, libgtk-3-0, gstreamer1.0-libav, gstreamer1.0-plugins-bad"));
    // Description 单行摘要
    assert!(lines
      .iter()
      .any(|l| l.starts_with("Description: Web2App desktop app for example.com")));
  }

  /// .desktop 必填项 + Exec/Icon 与包内路径一致。
  #[test]
  fn desktop_text_has_required_entries() {
    let t = desktop_text("web2app-example.com", "https://example.com");
    assert!(t.starts_with("[Desktop Entry]\n"));
    for key in ["Type=Application", "Name=example.com", "Exec=/usr/bin/web2app-example.com",
                "Icon=web2app-example.com", "Terminal=false"] {
      assert!(t.lines().any(|l| l == key), "missing: {key}");
    }
  }

  /// 架构映射：本机架构必须可映射（CI 与 VM 均为 amd64/arm64 之一）。
  #[test]
  fn dpkg_arch_maps_native() {
    assert!(dpkg_arch().is_ok());
  }
}
