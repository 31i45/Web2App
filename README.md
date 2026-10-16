# Web2App

Web→桌面应用打包器。输入网页 URL（可选自定义图标），生成桌面应用：Windows 单文件 `.exe`，Linux 原生 `.deb`。

受开源项目 https://github.com/tw93/Pake 和小暖科技的 SiteNative 项目启发，但绝对是全网最轻最小最原生的 Web→桌面应用打包器。

| Windows | Linux |
|---|---|
| ![Windows](assets/screenshot_windows.png) | ![Linux](assets/screenshot_linux.png) |

## 快速开始

**Windows**：从 [Releases](https://github.com/31i45/Web2App/releases) 下载 `web2app.exe`（CI 构建，附 SHA256SUMS 校验），或本地构建：

```powershell
.\build.ps1          # 一键构建（child → 测试 → clippy → master → 装配）
.\dist\web2app.exe  # 运行打包器
```

**Linux**（x86_64，需 WebKitGTK 4.1 运行环境）：从 [Releases](https://github.com/31i45/Web2App/releases) 下载 `web2app-v*-x86_64-unknown-linux-gnu`（裸 ELF，与 Windows 裸 exe 同语义），`chmod +x` 后直接运行；或本地构建：

```bash
./build.sh           # 一键构建（同 Windows 门禁口径）
./dist/web2app       # 运行打包器
```

母版界面三元素：**网页 URL 输入框、图标选择（可选）、打包按钮**（回车也可提交）。点击「打包」，产物生成在母版所在目录（文件名 = URL host，如 `example.com.exe` / `example.com.deb`）——master 双平台均为裸可执行分发，下载放哪产物落哪。选图走浏览器原生 `input[type=file]`+`FileReader`，图片字节经 IPC 传给原生，不依赖绝对路径。

## 使用产物

**Windows**：直接双击运行；Explorer 文件列表、标题栏与任务栏均显示用户自定义图标（无图标则显示 w2a 默认图标）。

**Linux**：`sudo apt install ./<host>.deb` 安装后，应用菜单出现启动项（含自定义图标），也可终端直接输入 `<host>` 运行；`sudo apt remove <host>` 卸载。文件名与包名同名，装/用/卸一条命令链。

## 运行时要求

| 平台 | 要求 | 说明 |
|---|---|---|
| Windows | WebView2 Runtime | [下载](https://developer.microsoft.com/microsoft-edge/webview2/)（Win11 自带，Win10 可能需手动安装） |
| Windows | 代码签名 | **可选**：未签名产物可正常运行，但 SmartScreen 会显示警告；签名后无警告 |
| Linux | 无手动步骤 | `.deb` 的 Depends 声明完整依赖链（`libwebkit2gtk-4.1-0`、`libgtk-3-0`、`gstreamer1.0-libav`、`gstreamer1.0-plugins-bad`），`apt install` 自动补齐 |

### 关于代码签名

签名是业务运营问题，不是架构职责。Web2App 预留了干净的签名 hook：

- 设置环境变量 `WEB2APP_SIGN_CERT`（证书路径 .pfx/.p12）后打包，master 自动调用 `signtool` 签名
- 可选 `WEB2APP_SIGN_PASS`（证书密码）、`WEB2APP_SIGN_TSA`（时间戳服务器 URL）
- 签名失败不阻塞打包（仅警告）
- **私钥永不内嵌进二进制**——证书在打包时通过环境变量传入
- 兼容本地 signtool（Windows SDK 自带）与远程签名服务（业务可替换 `master/src/sign.rs`）

未签名产物的实际表现：双击运行时 SmartScreen 显示「Windows 已保护你的电脑」警告，点击「更多信息」→「仍要运行」即可正常运行。签名后无此警告。

## 构建产物指标

| 指标 | 目标 | 实测 |
|---|---|---|
| child 体积（用户分发的产物） | < 1 MB | ~700 KB（随工具链微漂移） |
| Linux .deb 体积 | < 1 MB | ~430 KB（依赖系统 WebKitGTK，零内嵌浏览器） |
| master 体积（打包器） | < 2 MB | ~1.5 MB（内嵌 child 模板） |
| 冷启动 | < 2 s | 秒级（经验值，无自动化基准） |
| 关闭残留 | 无 | 进程/WebView2 子进程全部退出 |
| 运行副作用 | 无 | 无后台控制台；exe 目录零落盘 |

## 架构

v3 采用三 crate workspace 结构，实现「子应用结构性不可繁殖」与「单文件产物」的双重约束：

```
tail-core/        # 纯逻辑层：尾部协议解析与编码、PNG 解码、URL 工具（零 OS 依赖，可任意宿主单测）
├── src/lib.rs    #   AppConfig / read_tail / encode_tail / strip_tail / decode_png / resize_box / sanitize_host / host_of / url_fingerprint
child/           # 子应用运行时（用户分发的精简产物，~700KB）
├── build.rs     #   winres 编译 w2a.ico 进 .rsrc（仅 Windows 目标；文件图标默认值）
├── src/main.rs  #   读尾部配置 → 打开 WebView；无配置 → 报错（不可直接运行）
├── src/icon.rs  #   用户 PNG → tao 窗口图标（用 tail_core::decode_png）
├── src/platform/ #  平台外壳（`#[cfg]` 同名同签名模块，非 trait）：windows.rs / linux.rs =
│                  #  数据目录 / 宿主层按键 / 新窗口弹窗管线 / 外发 / 全屏 / 关闭兜底 / 致命错误
├── src/popup.rs #   新窗口路由判定（同站就地 / 跨站交系统 / 弹窗 / 伪协议忽略；纯逻辑无 OS 依赖）
└── src/webview.rs # 跨平台装配：WebView 构建 + 注入桥 + 全部宿主侧能力接线
master/          # 打包器（编译期内嵌 child 模板，~1.5MB）
├── build.rs     #   定位 child 二进制（OUT_DIR 上溯，兼容 --target）→ 拷贝内嵌；winres w2a 图标+身份
├── src/main.rs  #   表单窗口 + 打包调度
├── src/platform/ #  平台外壳：windows.rs / linux.rs = 数据目录 / verbatim 路径剥离 / 关闭兜底 /
│                  #  致命错误 / 打包平台项（pack / inject_file_icon / maybe_sign）
├── src/ui.rs    #   内嵌 HTML 表单 + IPC 解析
├── src/builder.rs #  CHILD_TEMPLATE + pack 编排骨架（模板 → 注入图标 → 追加尾部 → 签名）
├── src/tail_write.rs # 尾部写入（自 tail-core 外迁：写尾能力不进 child 依赖图，编译期不可繁殖）
├── src/winicon.rs # PNG→ICO + UpdateResource 注入 .rsrc（仅 Windows；经 platform 外壳暴露）
├── src/icon.rs  #   母版窗口图标（与 child 同构，master 不依赖 child crate）
└── src/sign.rs  #   可选 signtool 签名 hook（仅 Windows；经 platform 外壳暴露）
audit.ps1        # 结构性不可繁殖静态审计（CI 门禁：字符串标记 / PE 映像数 / cargo tree 依赖边界）
assets/
├── w2a.ico      # w2a 文件图标源（winres 编译进 child.exe）
└── logo_256.png # w2a 窗口图标源（include_bytes 内嵌进 child + master）
```

### 为什么是两个二进制？

- **child**：精简运行时，用户分发的产物。无打包器代码 → 结构性不可繁殖（无法通过补丁删除命令行参数等方式获得打包能力）。写尾能力（`write_tail`）物理隔离在 master 内——child 的依赖图中不含任何写尾代码，这是**编译期结构性保证**，不依赖 release 优化器剔除。
- **master**：打包器，编译期内嵌 child 模板（`include_bytes!(env!("CHILD_BIN_PATH"))`）。用户下载 master → 填表 → 打包 → 得到 child 产物。

两者通过 `tail-core` 共享尾部协议、PNG 工具与 URL 工具，避免重复代码。`audit.ps1` 在 CI 中机械守护该边界（字符串标记扫描 + PE 映像计数 + `cargo tree` 依赖断言）。

### 尾部协议

产物 = child 模板 + 尾部配置。尾部帧格式（v3）：
`<payload JSON> <MAGIC:16B> <len:u64 LE> <ver:u8> <crc32:u32 LE>`，footer 共 29 字节。
CRC32（IEEE 802.3）覆盖 payload + 长度 + 版本字节，尾部被截断或篡改时产物明确报「文件可能已损坏」而非静默错跑；旧版 v2 帧（24 字节，无 CRC）仍可读，剥离旧尾按结构切片进行——损坏帧亦剥离，重复打包体积稳定不膨胀。JSON 手写转义，零 serde 依赖。

### 图标机制（两条通道）

- **文件图标**（Explorer）：child 编译期由 winres 嵌入 w2a 默认图标；master 打包时通过 `UpdateResource` API 注入用户 PNG→ICO 覆盖 .rsrc 节。
- **窗口图标**（标题栏/任务栏）：用户 PNG 内嵌尾部配置，产物运行时经 tao `with_window_icon` 设置；未选图回落内嵌 w2a 默认图。
- **关键事实**：Windows 窗口类图标从不自动回落 exe .rsrc 资源——未显式设置时显示系统默认图标。故「未选用户图」必须主动设为内嵌 w2a 图标。

### 打包顺序

1. 写 CHILD_TEMPLATE → 临时文件
2. 注入用户图标到 .rsrc（UpdateResource，非阻塞）
3. 追加尾部配置（config 携带 icon_png 供窗口图标）
4. 原子 rename 临时文件 → 产物
5. 可选签名（非阻塞）

注入改写 PE .rsrc，必须在尾部追加之前完成，否则尾部会被 `EndUpdateResource` 丢弃。

### 关闭的确定性

实测（tao 0.37 + wry 0.57/WebView2）：单条 `WM_CLOSE` 路径下（Alt+F4、任务栏「关闭窗口」、`taskkill` 无 `/F`、UI 自动化），tao 的 `CloseRequested` 事件可能永不派发——消息被 WebView2 嵌套 COM 消息泵吞没，进程 30s+ 不自退。

v3 采用两层确定性兜底：
- **WNDPROC 哨兵**（`child|master::platform::install_close_watchdog`）：在 WebView 构建前替换窗口过程，`WM_CLOSE` 在消息进入框架之前即被捕获，武装看门狗后无条件透传 tao 原过程，事件链无损。
- **看门狗线程**（`child|master::platform::quit_watchdog`）：独立线程 sleep 300ms 后 `process::exit(0)`，绕开消息泵竞争，给出确定退出上限。

鼠标点 X 因伴随消息风暴通常无感；但「关闭即退出、零残留」不依赖运气。

### 宿主侧能力

wry 默认关闭了一批 WebView2 原生特性，产物按「桌面浏览器等效体验」逐项补齐，全部零新增 crate：

- **新窗口请求路由**（`child/src/popup.rs`）：`target=_blank` / `window.open` 原先被 WebView2 静默丢弃——这正是部分站点「链接点了没反应」的根因。现按 `classify_new_window` 四向分流：空白窗（`window.open()` 无 URL）与带尺寸/位置特征的 `window.open` 在应用内建独立原生窗口（共享发起方 WebView2 环境，OAuth 等会话得以延续）；同站（含互为子域）就地导航；跨站普通链接交系统默认程序；`blob:`/`data:`/`javascript:` 等浏览器内部伪协议忽略（外发无处理器、内载不可靠）。
- **HTML5 全屏**：注入脚本监听 `fullscreenchange` 上报，由宿主窗口 `set_fullscreen` 完成真全屏（WebView2 内部全屏只填满 webview，并非真全屏）。弹窗对等具备该能力（Win32 原生切换窗口样式）。
- **导航与缩放热键**：`with_hotkeys_zoom` / `with_back_forward_navigation_gestures`（触摸板横扫）。WebView2 的浏览器快捷键保持默认开启，故 **Alt+←/→ 后退/前进、鼠标侧键** 均原样生效；历史栈完整，可连续回退。
- **回首页**：Alt+Home 回到打包时配置的起始 URL（WebView2 无「主页」概念，由注入脚本补上）。双层送达：页面层 `keydown`（含判定、BFCache 回跳、在途守卫）+ 宿主层 `AcceleratorKeyPressed`（`webview2-com` 直连，按键送达内容层之前回调）——焦点落在跨源 iframe 内时页面层收不到按键，由宿主层兜底送达，两层以 200ms 时间戳去重。判定经 `URL` 规范化比较（含 `hash`），已在首页不重复导航；历史中存在首页条目时经 Navigation API `traverseTo` 回跳、命中 BFCache 瞬时生效，否则回落整页导航。只注册在最顶层帧；主窗口与弹窗共用同一首页。
- **剪贴板读取**：`with_clipboard(true)`（WebView2 默认关闭）。取值是「全局放行任意站点读取」——wry 的权限回调不携带请求 origin，无法按站点收敛，未收敛会牺牲「粘贴」类交互；此为已知边界。
- **下载**：保持 WebView2 原生下载面板与落点（不注册「开始下载」处理器，一注册即会关掉原生面板），仅在页面内补一条「下载完成 / 下载失败」轻提示。
- **标题同步**：页面 `document.title` 变化回流主循环——主窗口保留用户配置的品牌标题，弹窗（无配置标题）跟随文档标题。
- **`window.close` 守卫**：页面脚本对顶层窗口的 `window.close()` 调用被降级为无操作（浏览器对「非脚本打开的顶层窗」同样忽略，此处对齐该语义）；弹窗的 `window.close()` 仍正常关窗。
- **主题跟随系统**：页面配色（`prefers-color-scheme`）与窗口标题栏均跟随系统深浅色，与浏览器一致。
- **profile 隔离**：数据目录为 `%LOCALAPPDATA%\Web2App\apps\<host>_<URL 指纹>`——同一 host 下的不同应用 profile 相互隔离（cookie/存储/缓存不互污），同一 URL 的多次打包共享同一 profile。

## 平台策略

| 平台 | 产物 | 状态 |
|---|---|---|
| Windows | 单文件 `.exe` | 主线，完整支持 |
| Linux | 原生 `.deb`（Debian 系） | v3 已实现，为国产桌面操作系统服务 |
| macOS | `.app` | 无真机环境，暂不实现，等待社区支持 |

### Windows：单文件主线

Web2App 的核心承诺（单文件、自定义图标、零依赖）在 Windows 上完美实现：WebView2 在 Win11 预装、微软官方维护、API 稳定；PE + 尾部追加配置 = 真单文件；文件图标经 `UpdateResource` 注入 `.rsrc` 节，产物仍是单文件。关闭确定性哨兵、winres 资源编译等均为 Windows 原生能力，做深做透。

### Linux：.deb 为国产桌面操作系统服务

Linux 无原生单文件方案（AppImage 依赖 FUSE 且部分发行版默认禁用），但 Linux 生态的惯例本就是包管理器安装——`.deb` 是 Debian 系的原生形态。v3 实现了完整的 Linux 打包链路：

- **系统 WebKitGTK，零内嵌浏览器**：产物仅 ~430 KB，Depends 声明完整依赖链（`libwebkit2gtk-4.1-0`、`gstreamer1.0-libav` 等），`apt install` 自动补齐
- **原生集成**：`.desktop` 启动项 + hicolor 图标进应用菜单；文件名与包名同名（`baidu.com.deb` → 包 `baidu.com`），装/用/卸一条命令链
- **目标生态**：deepin、UOS、openKylin、麒麟等国产桌面操作系统，及 Ubuntu/Debian 全系

### macOS：暂不实现，等待社区

`.app` 必须是 bundle 目录结构（`Contents/MacOS/`、`Info.plist`、`.icns`），与单文件承诺本质冲突；签名、公证与窗口行为均需真机验证。当前无 macOS 真机环境，暂不实现——欢迎社区基于现有 `platform/` 外壳抽象（`#[cfg]` 同名同签名模块）贡献 macOS 后端。

## 开发

**Windows**：

```powershell
cargo test --workspace    # 84 个单元测试（tail-core 42 + child 19 + master 23）
.\build.ps1               # 一键发布构建（child → 测试 → clippy → master → 装配）
.\audit.ps1               # 结构性不可繁殖静态审计（CI 同款门禁：字符串标记 / PE 映像数 / cargo tree）
.\e2e.ps1                 # 端到端：身份/资源元数据、产物启动、零残留、裸模板不可运行、字节级模板一致性
cargo clippy --workspace --all-targets -- -D warnings   # 零告警门禁（build.ps1 已含）
cargo build -p child --release   # 构建 child（master 的前置依赖）
cargo build -p master --release  # 构建 master（内嵌 child 模板）
```

**Linux**（Debian 系；构建依赖 `libwebkit2gtk-4.1-dev libgtk-3-dev`，e2e 另需 `xvfb xdotool gstreamer1.0-libav`）：

```bash
./build.sh    # 一键发布构建（与 Windows 同门禁口径：child → 测试 → clippy → master → 装配）
./e2e.sh      # 端到端 39 项断言：deb 构建 / 安装 / 图标 / 运行 / 右键菜单 / 卸载全链路
```

依赖：`wry 0.57`（default-features=false, features=`devtools`、`os-webview`、`x11`）、`tao 0.37`、`webview2-com 0.39`（宿主层 Alt+Home，版本对齐 wry 传递依赖）、`png 0.18`、`windows-sys 0.59`（仅 Windows 目标）；构建期 `winres`（仅 Windows 目标，不进运行时）。JSON/base64/CRC32 手写，零 serde。
