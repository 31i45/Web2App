# 平台外壳接缝笔记（T-0b-2）

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
| `mount_webview` | `fn mount_webview<'a>(builder: wry::WebViewBuilder<'a>, window: &'a tao::window::Window) -> Result<wry::WebView, wry::Error>` |

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
| `set_fullscreen` | Win32 窗口样式 + 矩形切换 | **已落（T-1-1）**：克隆窗口后 `gtk Window fullscreen/unfullscreen`；ipc `w2a:fullscreen:` → 主窗走 tao `set_fullscreen` | V-4 视频全屏实测 |
| `mount_webview` | 直通 `builder.build(window)`（等价于接线前） | `builder.build_gtk(window.default_vbox())`；`default_vbox` 为 `None` 时回落直通 | 无（T-1-1 已落地：两 GDK 后端实测通过） |
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

### 3.4 T-1-5 落定：audit.ps1 ELF 判据（结构性不可繁殖跨平台）

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
