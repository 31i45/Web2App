# 平台外壳接缝笔记（T-0b-2）

> 有效期：平台事实（行为/约束/踩坑）变更时，变更处须同步复核。
> 最后验证：2026-10-18（commit 1add6dd）

> **出处**：`docs/CROSS_PLATFORM_DESIGN.md` §7 阶段 0b / ADR-3（接缝**由第二实现逼出，不预设计**）。
> **用途**：macOS（阶段 1.5/2）实施前的必读件——本文件是**接缝面的唯一事实源**。
> **关系**：设计文档 §3.3 的代码块是**示意草图**（其中含仅 Windows 存在的类型名与不可跨平台
> 的调用约定），0b 由 Windows + Linux 两实现共同确认后定稿于此；不变量与门禁仍以设计文档为准。
> **状态**：阶段 0b 定稿；阶段 1 落地项随任务推进同步更新（T-1-1 … T-1-5 已落，见 §3）。

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
| `popup_ipc` | `fn popup_ipc(handle: isize, action: crate::popup::PopupIpc)` |
| `mount_webview` | `fn mount_webview<'a>(builder: wry::WebViewBuilder<'a>, window: &'a tao::window::Window) -> Result<wry::WebView, wry::Error>` |
| `document_start_script` | `fn document_start_script() -> Option<&'static str>` |

**各平台私有**（不进接缝面，仅在各自 `platform/<os>.rs` 内）：`VK_HOME` /
`is_home_accel`（Windows 四参纯判定）/ `register_home_accel` 与全部裸 Win32 弹窗管线
（`popup_wnd_proc` / `create_window` / `RawWindow` / `SetNewWindow` 契约 / thread_local
`POPUPS`）。Linux 侧同名私有项见 §3.1（签名不同——判定输入是 GDK 事件字段，**不得**
为统一而抽象，ADR-1）。

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
| `mount_webview` | `fn mount_webview<'a>(builder: wry::WebViewBuilder<'a>, window: &'a tao::window::Window) -> Result<wry::WebView, wry::Error>`（T-1-4 落定，child 侧 D8 的 master 版） |

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
| D8 | child 列表未含 `mount_webview` | 已纳入接缝面（T-1-1 运行时验证逼出） | wry 的 `webkitgtk` 后端**通用** `build(window)` 只接受 `RawWindowHandle::Xlib`；tao 在 Wayland 会话下交出 `RawWindowHandle::Wayland` → Linux 上 `build` 必失败（`Error::UnsupportedWindowHandle`），**WebView 根本挂不上**。Linux 唯一可行路径是 `WebViewBuilderExtUnix::build_gtk` 挂到 tao 的 `WindowExtUnix::default_vbox` 容器（不经句柄转换，与 GDK 后端无关）。该选择依据 **tao/wry 运行时类型**做出，**不能**挪到 `webview.rs` 内用 `#[cfg(target_os = "linux")]` 分支——§4.2 明文只豁免两处 cfg，且 §3.4 要求平台判断只在 `platform/`。故以同名接缝项承载：Windows 直通 `build(window)`（行为零变更），Linux 走 `build_gtk`。为让调用点证明「容器窗口借用覆盖 WebContext 借用」，`build_webview` 的两个借用参数统一为 `'a`（不影响 Windows 行为）。 |

---

## 3. 各平台实现策略（Windows 为既有行为基线）

| 项 | Windows | Linux（0b 现状） | 阶段 1 待办 |
|---|---|---|---|
| `data_dir` | `%LOCALAPPDATA%\Web2App\<sub>`（temp 兜底） | XDG：`$XDG_DATA_HOME`（须绝对）→ `$HOME/.local/share` → temp | 无（T-1-3 已落：隔离实测见 §3.2） |
| `fatal` | `MessageBoxW`（GUI 子系统下 stderr 不可见） | `eprintln!` + `exit(1)`（UNIX 惯例，stderr 可见） | 可选 GTK dialog |
| `quit_watchdog` | 看门狗线程 300ms 后 `exit(0)` | **no-op**（GTK 无「WebView2 嵌套 COM 消息泵吞掉 WM_QUIT」的结构性问题；`ControlFlow::Exit` 即确定退出） | T-1-1：实测若出现残留再补 |
| `install_close_watchdog` | WNDPROC 哨兵（替换窗口过程捕获 `WM_CLOSE`） | **no-op**（纯 Win32 机制，GTK 无对应物） | 同上 |
| `install_keys` | `ICoreWebView2AcceleratorKeyPressed`（按键送达内容层**之前**回调，覆盖跨源 iframe 焦点边界） | **已落（T-1-2）**：组件级 `key-press-event`，见 §3.1 | 无（V-3 三候选实测后落定） |
| `shell_open` | `ShellExecuteW`（非阻塞） | `xdg-open`（`spawn`，非阻塞，语义对等） | 无（可选 `gio open` 细化） |
| `open_external_if_http` | 仅 http(s) 外发 | 同（共享语义） | 无 |
| `create_popup` | 裸 Win32 顶层窗 + **同 Environment** 的 WebView2（`NewWindowResponse::Create`） | **已落（T-1-1）**：GTK 顶层窗 + `NewWindowResponse::Create { webview }`（载荷 `webkit2gtk::WebView`，`build_gtk` 挂自建容器） | 无 |
| `navigate_popup` | 弹窗内「站内就地导航」路由调用 | **已落（T-1-1）**：`POPUPS[id].webview.load_url` | 无 |
| `close_all_popups` | 回收 `thread_local` 弹窗 WebView 表 | **已落（T-1-1）**：取窗口快照后 `close()`（防借用重入），`connect_destroy` 摘表 | 无 |
| `popup_ipc` | 不可达 no-op（弹窗各有独立 controller 与 ipc 处理器） | **已落（批次 2）**：按消息内嵌 id 回投——`Close → close_popup`、`Fullscreen → set_fullscreen`，见 §3.7 | 无 |
| `set_fullscreen` | Win32 窗口样式 + 矩形切换 | **已落（T-1-1）**：克隆窗口后 `gtk Window fullscreen/unfullscreen`；ipc `w2a:fullscreen:` → 主窗走 tao `set_fullscreen` | 无（V-4 收口见 §3.8） |
| `mount_webview` | 直通 `builder.build(window)`（等价于接线前） | `builder.build_gtk(window.default_vbox())`；`default_vbox` 为 `None` 时回落直通 | 无（T-1-1 已落地：两 GDK 后端实测通过） |
| `document_start_script` | `None`（WebView2 的 HTML5 全屏链路正常，无需守卫） | **已落（B1）**：`Some(FULLSCREEN_GUARD)`——四个全屏入口置 no-op + `fullscreenEnabled=false`，见 §3.8 | 无（V-4 收口） |
| `strip_verbatim_prefix` | 剥离 `\\?\` 形态 | **恒等**（Unix 无该前缀） | 无 |
| `pack` | `builder.rs` 编排骨架 re-export | **已落（T-1-4）**：staging 组装 → 同一骨架产 ELF+tail → `dpkg-deb` 原生封装，见 §3.3 | 无 |
| `inject_file_icon` | `UpdateResource` 注入 `.rsrc` | `Ok(())`（无此机制：ELF 无 `.rsrc`；图标经 staging 落 `/usr/share/icons/hicolor/256x256/apps/`，见 §3.3） | 无 |
| `maybe_sign` | signtool hook（环境变量传证书，私钥不内嵌） | `Ok(())`（无 signtool；签名 hook 点已预留，证书/仓库签名属业务运营） | 无（debsig / Release.gpg 按需，业务层） |

**「no-op 非近似」的三条判据**（其余 no-op 均标了阶段 1 待办，属**未接线**而非**行为退化**）：
1. `quit_watchdog` / `install_close_watchdog`：被绕开的是 **Win32 独有缺陷**，GTK 无此缺陷 → no-op 是正确表达。
2. `inject_file_icon`：被绕开的是 **PE 资源节**，ELF 无此概念 → `Ok(())` 是正确表达（不应打印伪警告）。
3. `install_keys`：0b 的页面层 `keydown` 仍生效 → 能力不缺，仅少覆盖「跨源 iframe 焦点」这一边界（与 0b 前一致）。

### 3.1 T-1-2 落定：Linux 按键拦截 = 组件级 `key-press-event`（V-3 收口）

**三候选 × 三焦点实测**（Zorin VM 无头 Xvfb + `GDK_BACKEND=x11` + xdotool，探针不进仓库）：
a `gtk::AccelGroup` / b `gtk_key_snooper_install` / c 组件级 `key-press-event` 在
「页面 / 跨源 iframe / 可编辑框」三焦点下**全部命中** Alt+Home。落定 **c**：作用域恰为该
WebView（对齐 Windows `AcceleratorKeyPressed` 的 per-controller 语义）、无全局状态、无废弃
API；§5.1 亦明示优先组件级。a 需把加速组挂到**窗口**（作用域过宽），b 是 GTK3 已废弃的
进程级全局 raw FFI。

**语义对等表**（Windows → Linux）：

| Windows | Linux | 备注 |
|---|---|---|
| `KEY_DOWN/SYSTEM_KEY_DOWN` 才算 | `key-press-event` 本就只有按下 | 无 KeyUp 需排除 |
| `VK_HOME(0x24)`（KP_Home 不命中） | `gdk::keys::constants::Home`（`KP_Home` 是独立 keyval，不命中） | 语义一致 |
| `IsMenuKeyDown`（Alt） | `MOD1_MASK`（GDK 跨后端统一映射 Alt） | 只看 Alt，叠加修饰不拦 |
| `WasKeyDown`（自动重复位） | **无对应物**：重复时间戳递增会被放行 | 不外显：页面层 `busy`/`isHome` 守卫使重复触发幂等 |
| 命中且 `HOST_ONLY` 才 `SetHandled(true)` | 命中且 `HOST_ONLY` 才返回 `Propagation::Stop` | 阶段 1 均 `Proceed`/不吞，两层并存 |
| 回调内不同步 `evaluate_script`（COM 重入）→ 经 proxy 回主循环 | 同：主窗经 `proxy.send_event(GoHome)`；弹窗 `evaluate_script` 走 WebKit 异步 `run_javascript`（不在 GTK 信号内重入），直接执行 | 分层职责一致 |

**Linux 特有：同键双路径派发去重**——实测同一物理按键的 GdkEvent 被 GTK 派发到客户端两
次（两条日志 `time` **完全相同**；X 层仅 1 次 KeyPress；纯 tao 窗口无此现象；页面打字不
翻倍）→ `register_home_accel` 按 **`GdkEventKey::time()` 时间戳**去重（同一事件的两份拷
贝时间戳相同；独立按键必不相同，X 毫秒时钟单调）。无去重则一次 Alt+Home 送达两次
（幂等守卫兜底，但不应依赖）。

**弹窗宿主层对等**：Windows = `register_home_accel` + `PostMessageW(WM_APP_HOME)`；
Linux = `create_popup` 内直接 `register_home_accel(&webview, …)`，命中按 id 取
`POPUPS[id].webview.evaluate_script(HOME_GO_SCRIPT)`（决策单点仍在页面层 `homeAction`）。

### 3.2 T-1-3 落定：`data_directory` 隔离（V-6 实测）

WebKitGTK（webkit2gtk-4.1 / 2.4x）实测布局：`WebContext.data_directory` 生效后站点数据
全部落 `<data_dir>/<host>_<fingerprint>/`（`cookies`、`localstorage/`、`hsts-storage.sqlite`、
`WebKitCache/`…）。未生效时引擎写默认 `$XDG_DATA_HOME/webkitgtk/` → 门禁判据（e2e.sh [2]）：
app 数据目录存在且含 localStorage/cookies 文件，且 `$XDG_DATA_HOME/webkitgtk/` **不存在**
（零泄漏）。

### 3.3 T-1-4 落定：master Linux 打包器（dpkg-deb 原生链路）

**不造轮子**：staging 目录组装 + `dpkg-deb --build --root-owner-group`（Debian 原生工具），
零第三方 crate；`pack` 仍是 §4.1 编排骨架的唯一平台插入点（Windows 侧 re-export 同一骨架）。

- **包名**：Debian Policy `[a-z0-9][a-z0-9+.-]+`（禁下划线/大写）→ 包名 = **host 本身**
  （用户决定：`baidu.com` 点分形态合法，Ubuntu 无完整域名形态的包名，重名实际不可能；
  仅首字符非字母数字的退化 host——首个标签非 ASCII sanitize 成连字符开头——回退
  `web2app-` 前缀）。产物文件名 = **包名 + `.deb`**（同名贯穿装/用/卸：
  `apt install ./baidu.com.deb` ↔ `apt remove baidu.com`，无需名称换算。实测踩坑：
  两者不一致时 `dpkg -r <文件名>` 报 illegal package name）；Windows 侧仍为 `<host>.exe`。
- **control**：五必填字段 + `Depends: libwebkit2gtk-4.1-0, libgtk-3-0, gstreamer1.0-libav`
  （§6.2）。`Description` 必须**单行**——Rust 字符串 `\` 续行会吃掉下一行全部前导空白，
  多行缩进即碎（实测踩坑）。
- **payload**（§6.2 四项）：`/usr/bin/<pkg>`（ELF+tail，加载器忽略尾随字节）、
  `/usr/share/applications/<pkg>.desktop`（`Exec=/usr/bin/<pkg>`、`Icon=<pkg>`）、
  `/usr/share/icons/hicolor/256x256/apps/<pkg>.png`、`/usr/share/doc/<pkg>/copyright`。
- **编排顺序**：staging 组装（消费 `&icon_png` 借用）→ `builder::pack` 产 ELF+tail（move）
  → 拷入 payload + `chmod 0755` → `dpkg-deb --build`；无论成败清理 staging 与 elf_tmp。
- **无头入口**：`web2app --pack <url> [--icon <png>] [--out <file>]`（CLI 位置参数解析，
  退出码 0/1/2；GUI 表单行为不变）——门禁与脚本化打包的机械入口。
- **mount_webview（master 版，D8 同型）**：Windows 直通 `build(window)`；Linux 走
  `build_gtk(default_vbox())`（Wayland 句柄问题同 child）→ §7 对应遗留项收口。
- **架构映射**：`dpkg_arch` 仅映射本机原生目标（x86_64→amd64、aarch64→arm64），其余报错
  ——不做交叉打包（§10）。

### 3.4 T-1-5 落定：audit.ps1 ELF 判据（依赖边界审计跨平台）

**合法 ELF 映像判据**（与 PE 判据同构）：`\x7fELF` 魔数 + 节头表指针（`e_shoff` QWORD@+0x28）
指向文件内完整节头表（`e_shnum × e_shentsize` 全部落界）。裸魔数会偶然出现在机器码/数据里
（PE 侧实测裸 `'PE\0\0'` 计数=3 而真映像仅 1），「魔数 + 指向结构的指针落界」才与
「真实映像」同义。仅 ELF64 小端（本仓库产物 x86_64/aarch64 均为此）。

**平台分支**（同一脚本 `audit.ps1`，pwsh 双平台可跑）：

| 检查 | Windows | Linux |
|---|---|---|
| 标记扫描（child 缺 / master 含 `UpdateResourceW` 等） | ✔ | **跳过**（winicon/sign 仅 Windows 编译，Linux master 无此符号面，正向对照无从谈起） |
| 映像计数（child==1 / master≥2 正向对照） | PE | ELF |
| 依赖树（child 排除 master） | ✔ | ✔ |
| 依赖树（master 含 windows-sys） | ✔ | **跳过**（Linux 目标本不编译 windows-sys，断言无意义） |

平台判定用 `.NET OSVersion` 而非 pwsh 的 `$IsWindows`——Windows PowerShell 5.1 未定义该
变量（引用静默得 `$null`），会无声走错分支。CI 跑法见 `ci.yml` verify-linux（release 产物，
与 G-0a「审计对象 = 交付形态」口径一致）。

### 3.5 T-1-4 补遗：GUI 打包线程化 + e2e [4] 主循环响应性门禁

**缺陷**（Zorin 桌面实测，用户截图「not responding」）：GUI 打包在事件循环闭包内同步执行
→ 阻塞 GDK 主循环 → 停止回应 `_NET_WM_PING` → GNOME/mutter 判「未响应」弹 Force Quit。
debug 产物（89MB debug child 被 xz 30–90s）把偶发放大为必现。

**修复**（`master/src/main.rs`）：打包移后台线程——IPC → `FormEvent::Pack` → `start_pack`
spawn 线程执行 `platform::pack`，完成经 `EventLoopProxy` 回投 `FormEvent::PackDone` →
`on_pack_done` 回写状态并解锁按钮。防重复提交仍由表单按钮 `disabled` 承担，PackDone 是
唯一解锁点。Linux 完成状态追加 `；安装：sudo apt install ./<file>`（deb 是软件包不是
可执行文件，`chmod +x` 直跑报 `!<arch>`——用户实测踩坑）。

**门禁判据**（e2e.sh [4]，红对照标定）：
- 探针 = mutter 同款：向 master 窗口发 `_NET_WM_PING` ClientMessage；GDK 的回应是向 root
  **重发同一消息**——`message_type=WM_PROTOCOLS` 不变、`data.l[0]=_NET_WM_PING`、
  `l[1]=我方 token 原样带回`、`l[2]=被探测窗口`。判定三元组 = (message_type, l0, l1/l2)；
  首版拿 message_type 与 `_NET_WM_PING` 比较 → 永不匹配 → 回执在场也判死（探针自误，
  三段分离试验 A/B/C 逐段暴露）。
- 窗口界定（零竞态）：staging 目录 `<tmp>/w2a-deb-<pkg>` 由 `pack` 第一步创建、`dpkg-deb`
  返回后才删除；deb 渐进写入期间 staging 一直在 →「staging 存在」= 打包确实进行中
  （采样窗口），「staging 消失 && deb 非空」= 完成。打包起跑前的空闲回应**不采样**
  （Return → IPC 起跑间隙的回应曾把阻塞版污染成绿）。
- 判据 = **最大连续未回应 < 10**（≈2s 静默，早于桌面会话 ~5s 判死），不是「至少一发有
  回应」——红对照实测：阻塞版 150 连发无回应（35s 死锁），但完成边界的一发回应会把
  「任一回应」判据污染成绿。
- 自检前置：空闲 ping 必须有回执（证明探针自身有效），红绿才可信。
- ctypes X11 探针逐函数声明 argtypes（默认 c_int 截断 64 位 Display*/Window → 段错误）。

**尺寸口径**（回应「包太大」）：release deb **427KB**（payload 四项，`Depends` 系统
WebKitGTK——**不内嵌 webview**）；13MB 是 debug master 内嵌 89MB debug child 的调试符号
被 xz 后仍残留的体积。`build.sh`（build.ps1 的 Linux 镜像）锁定 release 口径：
child → test → clippy → master → `dist/web2app`。

### 3.6 下载落盘：行为由 wry 默认处理器决定（VM 实测 + 源码核对）

**结论**：主窗口下载**双平台均正常且行为一致** = 静默落盘**系统默认下载目录** + 页面内
完成/失败轻提示；不出现 WebView2 原生下载面板。

**机制（wry 0.57 源码，三处已核对）**：
1. `WebViewAttributes::default()` 即置 `download_started_handler: Some(|_, _| true)`
   （`wry/src/lib.rs`）——**默认就有一个 allow-all 的「开始下载」处理器**。
2. Windows：只要 `started.is_some() || completed.is_some()` 就注册 `add_DownloadStarting`
   （`wry/src/webview2/mod.rs`）；回调返回 `true` → `SetHandled(true)` + `SetResultFilePath`。
3. Linux：同条件注册（`wry/src/webkitgtk/mod.rs`）；回调内把目的地设为
   `dirs::download_dir()`（`~/Downloads`）并按 ` (n)` 去重后 `set_destination`
   （`wry/src/webkitgtk/web_context.rs`）。

两分支的公共 API 都**只能**设置新处理器、**无法**把该字段清回 `None`——故「不注册
开始下载处理器」在 wry 0.57 下不可达。

**事实修正（WebView2 官方语义）**：`Handled = TRUE` **隐藏**默认下载对话框；`FALSE`（默认值）
才显示。故 wry 的默认处理器意味着 **Windows 原生下载面板被抑制**——`README` 曾表述的
「保持原生下载面板」与实际不符，本轮已按真实行为改写。要恢复面板须清空该处理器，
wry 0.57 无此 API（代价 > 收益，不做）。

**VM 实测（Zorin，Xvfb + xdotool + 本地 `http.server`）**：`http://` 链接与 `data:` URL
两种下载均落盘 `~/Downloads`（wry 对 `data:` 有文件名归一化分支），进程全程存活，
未观察到挂起。

### 3.7 Linux 弹窗：文档承载在发起方 UCM 上（注入通道 + ipc 回投）

**结论**：Linux 弹窗**没有自己的注入与消息通道**——它的文档承载在**发起方（主窗）的
`UserContentManager`** 上。故：

1. 弹窗页的 `window.ipc`、`INJECT_SCRIPT`（全屏上报 / Alt+Home）与主窗的 `CLOSE_GUARD`
   **均来自主窗 UCM**，在弹窗文档中同样生效；
2. 反之，`create_popup` 在弹窗 `WebViewBuilder` 上注册的 UCM /
   `with_initialization_script` / `with_ipc_handler` **从不生效**（实测：弹窗页读到的自建
   标记为 `undefined`）；
3. 弹窗页发出的 ipc 消息因此**由主窗处理器收到**，弹窗自身的处理器从不触发。

**为什么**：`related_view` 是本平台的**硬性契约**——去掉后 WebKit 内部断言
`WindowFeatures` abort（实测 SIGABRT），必须保留；而 `related view` 下 WebKit 以发起方的
UCM 承载弹窗文档，弹窗 `WebViewBuilder` 传入的 UCM 被忽略。

**由此产生的接缝设计**（`platform::popup_ipc` 为新增接缝项）：
- **消息格式**：Linux 弹窗的消息为 `w2a:popup:<id>:<kind>[:<arg>]`，由主窗处理器按 id
  回投（`webview.rs` 的 `POPUP_IPC_PREFIX` + `popup::parse_popup_ipc` →
  `platform::popup_ipc(id, action)` → 本窗 `close_popup` / `set_fullscreen`）。带 id 是
  必需的：一组弹窗并存时无法靠「最近一个」判断该关哪一个。主窗与 Windows 弹窗的消息格式
  不变（无此前缀）。
- **注入通道**：弹窗的「本窗标记」（`window.__w2aIpcTag`）与 `window.close()` 覆盖**不能**
  用 doc-start 注入（本窗 UCM 不生效），改由宿主在**每次文档提交**时对本窗
  `evaluate_script`（`popup_bind_script`，脚本内嵌本窗 id）。wry 的
  `PageLoadEvent::Started` 在 webkitgtk 上映射为 `LoadEvent::Committed`，是本平台能拿到的
  最早注入点；`evaluate_script` 为异步排队，实测在 `load` 之后、用户或定时触发的动作之前
  落地（页面在解析期即自行 `window.close()` 的极端情形不覆盖）。
- **为什么必须覆盖 `window.close()`**：WebKitGTK 拒绝非 `openedByDOM` 视图的关闭请求
  （连 `close` 信号都不发出，见 `create_popup` 内注释），且主窗 `CLOSE_GUARD` 在弹窗文档中
  同样生效（把它降级为无操作）→ 覆盖为「经 IPC 送回宿主」，即 Windows 侧
  `WindowCloseRequested → DestroyWindow` 的 Linux 对等物。
- Windows 侧无对应现象（弹窗各有独立 controller 与 ipc 处理器），故
  `platform::popup_ipc` 在 Windows 是**不可达 no-op 存根**——与 Linux 的
  `install_close_watchdog` 空实现同构。

**VM 实测（Zorin，Xvfb + openbox + xdotool + 本地 `http.server`）**：
1. 弹窗页 `window.close()` → GTK 顶层窗消失、主窗与进程存活、stderr 无 GTK critical
   （对照：修复前弹窗存活、进程存活）；
2. 弹窗页发出的 `fullscreen:1` → **弹窗**全屏（1280x800@0,0），主窗保持 1100x750
   （对照：修复前主窗被全屏、弹窗不动）。

### 3.8 Linux HTML5 全屏：请求即 SIGABRT → 文档起始置 no-op（B1 收口）

**结论**：Linux 侧**不提供** HTML5 全屏能力。接缝项 `document_start_script()`
（`platform/linux.rs`）返回 `FULLSCREEN_GUARD`，在**文档起始**把全屏入口覆写为「返回
rejected promise 的 no-op」，并把 `document.fullscreenEnabled` / `webkitFullscreenEnabled`
读作 `false`。

**为什么（平台事实）**：WebKitGTK 2.50.4 处理 HTML5 全屏请求时走 WebKit 内部一条
`RELEASE_ASSERT` 路径 → **UI 进程 SIGABRT**（整个应用闪退）。**embedder 无法拦截**：与有无
窗口管理器、是否挂 `enter-fullscreen` 处理器（返回 `true` / `false`）均无关（三组对照实测）。
即 V-4 的答案是「消灭请求入口」，而非「处理事件」。

**为何拦在「文档起始 / JS 原型」**：请求由页面 JS 发起（`Element.requestFullscreen`），
宿主侧 `enter-fullscreen` 发生在 WebKit **已决定全屏之后**，拦不到；从原型覆写是唯一在请求
进入 WebKit 之前的位置。覆写**不触碰 DOM**，站点原样呈现（INV-4）。

**覆盖面**：`Element.prototype` 的 `requestFullscreen` / `webkitRequestFullscreen` /
`webkitRequestFullScreen`，加 `HTMLVideoElement.prototype.webkitEnterFullscreen`（遗留入口）；
`fullscreenEnabled` 读 `false` 使守规范的站点自隐藏全屏按钮（不调用即不崩）。

**通道**：经接缝项追加进**主窗** builder 的初始化脚本（与 `INJECT_SCRIPT` / `CLOSE_GUARD`
同一 UCM）→ 主窗与 Linux 弹窗文档同样生效（弹窗文档承载在发起方 UCM 上，见 §3.7）。

**残余（诚实登记，未覆盖）**：WebKitGTK **原生视频控件**自带的全屏按钮在 WebKit 内部（C++）
发起，不经 JS 原型 → 本守卫不可达（同「embedder 无法拦截」）。实测代价（真实可解码视频 +
定位原生控件按钮）与收益不成比例，故**不实测**、不宣称覆盖。

**VM 实测（Zorin，Xvfb + xdotool + 真实 `tail` 产物；媒体已加载 `readyState=4`）**：
1. 载入期 `document.body.requestFullscreen()` → 拒绝于 `Web2App: fullscreen is unavailable…`
   （守卫自有错误，非 WebKit 的 `NotAllowedError`）；`document.fullscreenEnabled === false`；
2. **用户手势点击** element 全屏按钮（历史崩溃路径）→ 进程存活、stderr 无 GTK critical / abort；
3. 媒体已加载时点击 `video.webkitEnterFullscreen()` → 返回 Promise（即本守卫）、进程存活；
4. **红对照**：同一点击、同一 `--pack` 流程，但内嵌的是**未含本守卫**的 child 模板 → `SIGABRT`
   （stderr `Aborted`）。故本条 3 不是「无从证伪」的孤证。

> **构建注意（实测踩坑）**：`master` 经 `build.rs` 在**构建期**把 `child` 内嵌为
> `CHILD_TEMPLATE`。只跑 `cargo build --workspace` 时 cargo 不保证 child 先于 master 的
> build script 完成 → 产物可能内嵌**陈旧 child**。做产物级验证必须
> `cargo build -p child && cargo build -p master`（`build.ps1` 即此序）。

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

## 7. 遗留

- Linux 阶段 1 **已收口**：GTK 外壳与弹窗（T-1-1）、按键拦截（T-1-2，§3.1）、数据隔离
  （T-1-3，§3.2）、`.deb` 打包器（T-1-4，§3.3）、ELF 审计判据（T-1-5，§3.4）。G-1 无头
  门禁固化于 `e2e.sh`（按键矩阵 / 隔离 / deb 可装可跑，全 exit code）+ `audit.ps1` Linux
  分支，CI 接线见 `.github/workflows/ci.yml` verify-linux。
- macOS：全部（阶段 1.5/2，禁止盲写）
- `README.md`：目前只更新了代码结构树；**平台矩阵与天花板披露**（B/C 级）待阶段 1 一并如实
  更新（§12 DoD）
- CI：release job 仍只依赖 Windows `verify`——Linux **不产出发布 `.deb`**（§10「不做跨平台
  打包」；verify-linux 的 G-1 仅验证能力与行为，发布产物属阶段 2 决策）

---

## 8. HOST_ONLY 翻转条件（回首页按键的两层并存收口）

`HOST_ONLY`（`child/src/webview.rs:29`，当前 `false` = 页面层 `keydown` 与宿主层
`AcceleratorKeyPressed` 并存）是回首页按键的阶段开关，**不是** 配置项。

**翻转条件（进入阶段 2 的唯一门径）**：宿主层按键路径在 Windows + Linux 双平台 e2e
中验证稳定（按键矩阵全绿、跨源 iframe 焦点兜底实测有效）后，才允许置 `true` 并删除
页面层 `keydown` 注册。

**此前保持双层并存**：两层同达时由注入脚本内 200 ms 时间戳去重，宁可偶发去重命中、
不可单层失守（页面层在跨源 iframe 焦点下收不到按键；宿主层是各平台新路径，未达双
平台稳定验证前不作唯一依赖）。

---

## 9. 架构决策记录约定

- 今后重大架构决策以 ADR 形式记录于 `docs/adr/NNNN-标题.md`（序号递增，单个决策一个文件）。
- 本约定之前的设计文档（`docs/CROSS_PLATFORM_DESIGN.md`、本文件）视为约定前史，原地保留、不回拆。
