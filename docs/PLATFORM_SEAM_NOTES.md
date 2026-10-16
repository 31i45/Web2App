# 平台外壳接缝笔记（T-0b-2）

> **出处**：`docs/CROSS_PLATFORM_DESIGN.md` §7 阶段 0b / ADR-3（接缝**由第二实现逼出，不预设计**）。
> **用途**：macOS（阶段 1.5/2）实施前的必读件——本文件是**接缝面的唯一事实源**。
> **关系**：设计文档 §3.3 的代码块是**示意草图**（其中含仅 Windows 存在的类型名与不可跨平台
> 的调用约定），0b 由 Windows + Linux 两实现共同确认后定稿于此；不变量与门禁仍以设计文档为准。
> **状态**：阶段 0b 定稿（Linux 侧为最小实现，行为在阶段 1 落地）。

---

## 1. 接缝面（定稿签名）

`#[cfg(target_os = "…")]` 同名同签名模块，**不用 trait**（ADR-1）。以下项**每个平台模块必须
逐一提供**（缺一即编译失败，这是刻意设计）。

### 1.1 `child/src/platform`（全部 `pub(crate)`）

| 项 | 定稿签名 |
|---|---|
| `data_dir` | `fn data_dir(sub: &str) -> PathBuf` |
| `fatal` | `fn fatal(msg: &str) -> !` |
| `quit_watchdog` | `fn quit_watchdog()` |
| `install_close_watchdog` | `fn install_close_watchdog(window: &tao::window::Window)` |
| `install_keys` | `fn install_keys(webview: &wry::WebView, proxy: EventLoopProxy<ChildEvent>)` |
| `shell_open` | `fn shell_open(url: &str)` |
| `open_external_if_http` | `fn open_external_if_http(url: &str)` |
| `create_popup` | `fn create_popup(url: &str, features: NewWindowFeatures, home: &str) -> Option<NewWindowResponse>` |
| `navigate_popup` | `fn navigate_popup(handle: isize, url: &str)` |
| `close_all_popups` | `fn close_all_popups()` |
| `set_fullscreen` | `fn set_fullscreen(handle: isize, on: bool)` |

**Windows 私有**（不需其他平台提供，仅在 `platform/windows.rs` 内）：`VK_HOME` /
`is_home_accel` / `register_home_accel` 与全部裸 Win32 弹窗管线（`popup_wnd_proc` /
`create_window` / `RawWindow` / `SetNewWindow` 契约 / thread_local `POPUPS`）。

### 1.2 `master/src/platform`（全部 `pub(crate)`）

| 项 | 定稿签名 |
|---|---|
| `data_dir` | `fn data_dir(sub: &str) -> PathBuf` |
| `strip_verbatim_prefix` | `fn strip_verbatim_prefix(p: PathBuf) -> PathBuf` |
| `fatal` | `fn fatal(msg: &str) -> !` |
| `quit_watchdog` | `fn quit_watchdog()` |
| `install_close_watchdog` | `fn install_close_watchdog(window: &tao::window::Window)` |
| `pack` | `fn pack(out_path: &Path, url: &str, icon_png: Option<Vec<u8>>) -> io::Result<BuildOutput>` |
| `inject_file_icon` | `fn inject_file_icon(exe: &Path, png: &[u8]) -> Result<(), String>` |
| `maybe_sign` | `fn maybe_sign(exe: &Path) -> Result<(), String>` |

---

## 2. 与设计文档 §3.3 的偏差（逐条）

| # | §3.3 示意 | 0b 定稿 | 理由 |
|---|---|---|---|
| D1 | `install_close_watchdog(handle: isize)` | `install_close_watchdog(window: &tao::window::Window)` | `isize` 迫使调用点自取 HWND（`tao::platform::windows::WindowExtWindows` 是 **Windows-only** import）→ 会把 `#[cfg]` 逼进 `webview.rs` / `main.rs`，违反 §3.4「平台判断只在 `platform/`」。取 `&Window` 后调用点平台无关，Unix 侧 no-op 不需任何 `#[cfg]`。 |
| D2 | `install_keys(webview, proxy, hwnd: RawWindowHandleInfo)` | `install_keys(webview, proxy)` | `RawWindowHandleInfo` 是本项目**不存在**且仅 Windows 概念的类型名，不能出现在跨平台签名。Windows 仅需 `webview.controller()`。若 GTK 阶段 1 确需窗口（GTK 加速键组挂在 `GtkWindow` 上），**两边同时**加参（ADR-3 允许 0b→1 之间收敛）。 |
| D3 | `maybe_sign(path) -> io::Result<()>` | `Result<(), String>` | 保住 Windows 现有签名（G-0a 零行为变更）；`String` 可携带 signtool / 证书错误文本，比 `io::Error` 更有诊断价值。`pack` 的 `io::Result<BuildOutput>` 与 §3.3 一致。 |
| D4 | child 列表未含 `quit_watchdog` / `open_external_if_http` | 已纳入接缝面 | `quit_watchdog` 系 0a 自 `tail-core` 迁出（`CloseRequested` 分支与 `webview.rs` 共享调用点）；`open_external_if_http` 系原 `popup.rs` 弹窗降级助手，0b 随弹窗管线迁入平台层（判定 + 外发，两平台语义一致）。 |
| D5 | 未提及 `inject_file_icon` | 追加为 master 接缝项 | 图标注入发生在**共享编排骨架**（`builder.rs::pack`）第 2 步，且必须在尾部追加之前（§3.2 打包顺序 CRITICAL）；§4.1 禁止改写该骨架 → 只能以同名平台项留在骨架内。 |
| D6 | §3.3 图示三平台 `mod.rs` | 0b **只登记 windows / linux** | macOS 阶段 1.5/2 已推迟且无实机（禁止盲写）→ 不引用尚不存在的模块。实施 macOS 时按 §6 清单补上。 |
| D7 | `master` 侧 `pack` | 已落位：Windows = `builder.rs` 编排骨架 re-export | §4.1 规定 `builder.rs` 编排骨架 100% 复用（禁止改写）→ §4.3 的「master 打包器」平台重写只落在**平台特定步骤**（图标 / 签名 / `.deb` 封装）上；`pack` 作为 §3.3 明列的 master 平台项以 re-export 暴露，Linux 侧成为 `.deb` 封装的唯一插入点。 |

---

## 3. 各平台实现策略（Windows 为既有行为基线）

| 项 | Windows | Linux（0b 现状） | 阶段 1 待办 |
|---|---|---|---|
| `data_dir` | `%LOCALAPPDATA%\Web2App\<sub>`（temp 兜底） | XDG：`$XDG_DATA_HOME`（须绝对）→ `$HOME/.local/share` → temp | T-1-3：`WebContext.data_directory` 隔离实测（V-6） |
| `fatal` | `MessageBoxW`（GUI 子系统下 stderr 不可见） | `eprintln!` + `exit(1)`（UNIX 惯例，stderr 可见） | 可选 GTK dialog |
| `quit_watchdog` | 看门狗线程 300ms 后 `exit(0)` | **no-op**（GTK 无「WebView2 嵌套 COM 消息泵吞掉 WM_QUIT」的结构性问题；`ControlFlow::Exit` 即确定退出） | T-1-1：实测若出现残留再补 |
| `install_close_watchdog` | WNDPROC 哨兵（替换窗口过程捕获 `WM_CLOSE`） | **no-op**（纯 Win32 机制，GTK 无对应物） | 同上 |
| `install_keys` | `ICoreWebView2AcceleratorKeyPressed`（按键送达内容层**之前**回调，覆盖跨源 iframe 焦点边界） | **no-op**（页面层 `keydown` 仍覆盖顶层帧，非退化） | T-1-2 / V-3：GTK 加速键 vs 组件级 `key-press-event` 实测选定 |
| `shell_open` | `ShellExecuteW`（非阻塞） | `xdg-open`（`spawn`，非阻塞，语义对等） | 无（可选 `gio open` 细化） |
| `open_external_if_http` | 仅 http(s) 外发 | 同（共享语义） | 无 |
| `create_popup` | 裸 Win32 顶层窗 + **同 Environment** 的 WebView2（`NewWindowResponse::Create`） | 返回 `None` → 调用方降级 `open_external_if_http` | T-1-1：GTK 顶层窗 + `NewWindowResponse::Create { webview }`（载荷 `webkit2gtk::WebView`） |
| `navigate_popup` | 弹窗内「站内就地导航」路由调用 | 无调用方（`#[allow(dead_code)]`） | T-1-1 接线 |
| `close_all_popups` | 回收 `thread_local` 弹窗 WebView 表 | **no-op**（0b 无弹窗） | T-1-1 |
| `set_fullscreen` | Win32 窗口样式 + 矩形切换 | 无调用方（`#[allow(dead_code)]`；主窗全屏走 tao `Window::set_fullscreen`） | T-1-1 + WebKitGTK `enter-fullscreen`（V-4） |
| `strip_verbatim_prefix` | 剥离 `\\?\` 形态 | **恒等**（Unix 无该前缀） | 无 |
| `pack` | `builder.rs` 编排骨架 re-export | 委派同一骨架 | T-1-4：`dpkg-deb` 封装（唯一插入点） |
| `inject_file_icon` | `UpdateResource` 注入 `.rsrc` | `Ok(())`（无此机制：ELF 无 `.rsrc`） | T-1-4：`/usr/share/icons/hicolor/<sz>/apps/` PNG |
| `maybe_sign` | signtool hook（环境变量传证书，私钥不内嵌） | `Ok(())`（无 signtool；`.deb` 完整性属业务） | T-1-4：`debsig` / `Release.gpg` 按需 |

**「no-op 非近似」的三条判据**（其余 no-op 均标了阶段 1 待办，属**未接线**而非**行为退化**）：
1. `quit_watchdog` / `install_close_watchdog`：被绕开的是 **Win32 独有缺陷**，GTK 无此缺陷 → no-op 是正确表达。
2. `inject_file_icon`：被绕开的是 **PE 资源节**，ELF 无此概念 → `Ok(())` 是正确表达（不应打印伪警告）。
3. `install_keys`：0b 的页面层 `keydown` 仍生效 → 能力不缺，仅少覆盖「跨源 iframe 焦点」这一边界（与 0b 前一致）。

---

## 4. 平台无关层（不得出现平台判断）

以下文件**不得** import `tao::platform::*` / `wry::WebViewExt*` / `winapi` / `windows_sys`，
不得出现 `#[cfg(target_os = …)]`：

- `tail-core/` 全部（零 OS 依赖，0a 已保证）
- `child/`：`main.rs` / `popup.rs` / `icon.rs`
- `master/`：`main.rs` / `ui.rs` / `builder.rs` / `tail_write.rs`

**两处明文豁免**（§4.2 ⚙️「同一份代码 + 少量 cfg」）：
1. `child/src/webview.rs` 的 `set_theme`：`WebView2` 专有（`WebViewExtWindows`），§4.2 明列；
   不得向 Unix 宣称平价。
2. WebView 构建失败提示：`if cfg!(target_os = ...)` 构造提示文案（`cfg!` 为编译期常量，
   不引平台类型；`child/src/webview.rs` 与 `master/src/main.rs` 各一处）。

---

## 5. 依赖边界（Cargo 目标平台拆分规则）

**规则**：只下沉「**作用于产物**的运行时依赖」；build-dependency **不下沉**。

| crate | 下沉 | 保留无条件 | 依据 |
|---|---|---|---|
| `child` | `webview2-com` → `[target.'cfg(windows)'.dependencies]` | `png`（dev）、`winres`（build） | 若不下沉，Unix 目标会去编译 Windows 专属 crate；`webview2-com` 仅供 `AcceleratorKeyPressed` |
| `master` | `windows-sys` → `[target.'cfg(windows)'.dependencies]` | `png`、`winres`（build） | `windows-sys` 仅供 `UpdateResource`；Unix 目标不得引入 |

**`winres` 为何保持无条件**（child + master 一致）：build script 在 **host** 上编译，其 Windows
分支虽被 `CARGO_CFG_TARGET_OS` **运行时**门控，但代码仍需可编译；`winres` 本身支持非 Windows
host 交叉编译；build-dependency **不进产物** → 不触轻量红线。若改为
`[target.'cfg(windows)'.build-dependencies]`，跨编译（Linux host → Windows target）时该 cfg 与
`CARGO_CFG_TARGET_OS` 门控的语义不一致，易碎。

---

## 6. macOS 消费清单（阶段 1.5/2）

- [ ] 新建 `child/src/platform/macos.rs` / `master/src/platform/macos.rs`，**逐项照抄 §1 两张表**
      的签名（编译器强制，缺一即失败）
- [ ] `platform/mod.rs` 补 `#[cfg(target_os = "macos")] mod macos; pub(crate) use macos::*;`
- [ ] `create_popup`：返回 `NewWindowResponse::Create`（载荷与 Linux 的 `webkit2gtk::WebView`
      **不同**）→ **不得**为统一而抽象（ADR-1）
- [ ] `install_keys`：主菜单 `NSMenuItem` ⌘⇧H；**Edit 菜单是强制基础设施**（缺失则 ⌘C/V 完全
      失效，见 §5.2）
- [ ] `data_dir`：`~/Library/Application Support/Web2App/<sub>`
- [ ] `fatal`：`NSAlert`（GUI 子系统下 stderr 不可见，不同于 Linux 的判断）
- [ ] `install_close_watchdog` / `quit_watchdog`：先 no-op，实测残留再补（Cocoa 无 Win32 消息泵吞没）
- [ ] `inject_file_icon`：`.app` 的 `Contents/Resources/*.icns`（无需向可执行文件注入）
- [ ] `maybe_sign`：`codesign` + 公证（**真机硬门控**）
- [ ] `pack`：产出 `.app` bundle（唯一插入点）
- [ ] `strip_verbatim_prefix`：恒等
- [ ] 依赖下沉：macOS 目标不得引入 `webview2-com` / `windows-sys`

---

## 7. 遗留（0b 明确未做）

- Linux：`.deb` 打包（T-1-4）、GTK 弹窗（T-1-1）、按键拦截（T-1-2）、`data_directory` 隔离实测
  （T-1-3）、`audit.ps1` 的 ELF 判据（T-1-5）、无头门禁（G-1）
- macOS：全部（阶段 1.5/2，禁止盲写）
- `README.md`：目前只更新了代码结构树；**平台矩阵与天花板披露**（B/C 级）待阶段 1 一并如实
  更新（§12 DoD）
- CI：`verify-linux` 目前只跑 `build` / `test` / `clippy`，**不产出 `.deb`**，release job 仍只
  依赖 Windows `verify`（§10「不做跨平台打包」）
