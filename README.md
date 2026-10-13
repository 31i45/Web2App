# Web2App

Windows 单文件 Web→桌面应用打包器。输入网页 URL（可选自定义图标），生成单文件桌面应用。

受开源项目 https://github.com/tw93/Pake 和小暖科技的 SiteNative 项目启发。

## 快速开始

从 [Releases](https://github.com/31i45/Web2App/releases) 下载 `web2app.exe`（CI 构建，附 SHA256SUMS 校验），或本地构建：

```powershell
.\build.ps1          # 一键构建（child → 测试 → master → 装配）
.\dist\web2app.exe  # 运行打包器
```

母版界面三元素：**网页 URL 输入框、图标选择（可选）、打包按钮**（回车也可提交）。点击「打包」，产物生成在母版所在目录（文件名 = URL host，如 `example.com.exe`）。选图走浏览器原生 `input[type=file]`+`FileReader`，图片字节经 IPC 传给原生，不依赖绝对路径。

## 使用产物

- 直接双击运行 → 打开目标网页的桌面应用窗口
- Explorer 文件列表显示用户自定义图标（无图标则显示 w2a 默认图标）
- 标题栏与任务栏显示用户图标（无图标则显示 w2a 默认图标）

## 运行时要求

| 要求 | 说明 |
|---|---|
| WebView2 Runtime | [下载](https://developer.microsoft.com/microsoft-edge/webview2/)（Win11 自带，Win10 可能需手动安装） |
| 代码签名 | **可选**：未签名产物可正常运行，但 SmartScreen 会显示警告；签名后无警告 |

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
| child 体积（用户分发的产物） | < 1 MB | ~750 KB（随工具链微漂移） |
| master 体积（打包器） | < 2 MB | ~1.5 MB（内嵌 child 模板） |
| 冷启动 | < 2 s | 秒级 |
| 关闭残留 | 无 | 进程/WebView2 子进程全部退出 |
| 运行副作用 | 无 | 无后台控制台；exe 目录零落盘 |

## 架构

v2 采用三 crate workspace 结构，实现「子应用结构性不可繁殖」与「单文件产物」的双重约束：

```
tail-core/        # 共享层：尾部协议、PNG 解码、URL 工具、致命错误呈现（零 tao/wry 依赖）
├── src/lib.rs    #   AppConfig / read_tail / write_tail / decode_png / resize_box / sanitize_host
child/           # 子应用运行时（用户分发的精简产物，~750KB）
├── build.rs     #   winres 编译 w2a.ico 进 .rsrc（文件图标默认值）
├── src/main.rs  #   读尾部配置 → 打开 WebView；无配置 → 报错（不可直接运行）
├── src/icon.rs  #   用户 PNG → tao 窗口图标（用 tail_core::decode_png）
└── src/webview.rs # WebView2 窗口构建（数据目录 LOCALAPPDATA）
master/          # 打包器（编译期内嵌 child 模板，~1.5MB）
├── build.rs     #   定位 child.exe（OUT_DIR 上溯，兼容 --target）→ 拷贝内嵌；winres w2a 图标+身份
├── src/main.rs  #   表单窗口 + 打包调度
├── src/ui.rs    #   内嵌 HTML 表单 + IPC 解析
├── src/builder.rs #  CHILD_TEMPLATE + pack（模板 → 注入图标 → 追加尾部 → 签名）
├── src/winicon.rs # PNG→ICO + UpdateResource 注入 .rsrc（用户自定义文件图标）
├── src/icon.rs  #   母版窗口图标（与 child 同构，master 不依赖 child crate）
└── src/sign.rs  #   可选 signtool 签名 hook（非阻塞）
assets/
├── w2a.ico      # w2a 文件图标源（winres 编译进 child.exe）
└── logo_256.png # w2a 窗口图标源（include_bytes 内嵌进 child + master）
```

### 为什么是两个二进制？

- **child**：精简运行时，用户分发的产物。无打包器代码 → 结构性不可繁殖（无法通过补丁删除命令行参数等方式获得打包能力）。
- **master**：打包器，编译期内嵌 child 模板（`include_bytes!(env!("CHILD_BIN_PATH"))`）。用户下载 master → 填表 → 打包 → 得到 child 产物。

两者通过 `tail-core` 共享尾部协议、PNG 工具与 URL 工具，避免重复代码。

### 尾部协议

产物 = child 模板 + 尾部配置。尾部格式：`<payload JSON> <MAGIC:16B> <len:u64 LE>`。
从文件末尾倒数 24 字节定位，天然支持覆盖旧配置。JSON 手写转义，零 serde 依赖。

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

v2 采用两层确定性兜底：
- **WNDPROC 哨兵**（`tail_core::arm_close_watchdog`）：在 WebView 构建前替换窗口过程，`WM_CLOSE` 在消息进入框架之前即被捕获，武装看门狗后无条件透传 tao 原过程，事件链无损。
- **看门狗线程**（`tail_core::quit_watchdog`）：独立线程 sleep 300ms 后 `process::exit(0)`，绕开消息泵竞争，给出确定退出上限。

鼠标点 X 因伴随消息风暴通常无感；但「关闭即退出、零残留」不依赖运气。

## 平台策略

v2 转向 **Windows-only** 长期商用，Linux/macOS 平台不再维护。社区可基于 v1 自行 fork 移植。

这一决策基于技术可行性与商业性价比的综合评估，而非偏好：

### 1. WebView 后端的成熟度差异

| 平台 | 后端 | 状态 | 问题 |
|---|---|---|---|
| Windows | WebView2 | Win11 预装，微软官方维护，API 稳定 | 无 |
| macOS | WKWebView | 系统自带，Apple 维护 | wry 需额外平台依赖；图标/窗口控制 API 与 Windows 差异大 |
| Linux | WebKitGTK | 需用户自行安装 `libwebkit2gtk-4.1` | 发行版碎片化（Ubuntu/Debian/Fedora/Arch 版本不一），无统一运行时保证 |

Windows 是唯一「运行时开箱即用 + API 稳定」的平台。Linux 上用户可能因缺 WebKitGTK 而无法启动，macOS 上窗口/图标行为需独立适配。

### 2. 单文件产物的可行性

Web2App 的核心承诺是**单文件产物**——用户拿到一个 exe，双击即运行，无安装、无辅助文件。这一承诺在三平台的实现难度天差地别：

- **Windows**：PE 文件 + 尾部追加配置 = 真单文件。文件图标通过 `UpdateResource` API 注入 `.rsrc` 节，产物仍是单文件。
- **macOS**：应用必须是 `.app` bundle（目录结构，含 `Contents/MacOS/`、`Contents/Resources/`、`Info.plist`），无法单文件。自定义图标需替换 bundle 内的 `.icns` 并修改 `Info.plist`，破坏单文件承诺。
- **Linux**：无原生单文件方案。AppImage 需 FUSE 支持且部分发行版默认禁用；`.deb`/`.rpm` 是包而非单文件；图标依赖 `.desktop` 文件 + 图标文件 + 桌面环境解析。要实现「单文件 + 自定义图标」在 Linux 上几乎无解。

### 3. 构建与测试成本

- **CI 矩阵膨胀**：三平台 × 三 crate workspace，构建时间与资源消耗成倍增长。
- **行为差异**：每个平台的 WebView 对窗口生命周期、关闭消息、焦点、IME 的处理不同，关闭确定性等问题需在每个平台独立验证——tao/wry 在 Windows 上的 `WM_CLOSE` 吞没问题在 macOS/Linux 上是另一套机制。
- **Linux 碎片化**：无法穷举所有发行版/桌面环境组合的测试，bug 报告会持续涌入而难以复现。

### 4. 商业性价比

- 桌面应用商用市场 Windows 占绝对多数，Linux/macOS 桌面用户占比低且对「单文件 exe」类工具需求有限。
- 跨平台代码增加的复杂性会拖慢 Windows 端的迭代速度，反而损害主要用户的体验。
- 聚焦单一平台可以做深做透——UpdateResource 图标注入、关闭确定性哨兵、winres 资源编译等都是 Windows 原生能力，在其他平台无法等价实现。

### 结论

Web2App 的核心价值（单文件、自定义图标、零依赖）在 Windows 上可以完美实现，在 Linux/macOS 上要么无法实现（单文件），要么成本极高（WebView 碎片化、构建测试矩阵）。放弃跨平台是为了把有限的精力集中在能把产品做到极致的平台上。

## 开发

```powershell
cargo test --workspace    # 54 个单元测试（tail-core 30 + child 6 + master 18）
.\e2e.ps1                 # 端到端：身份/资源元数据、产物启动、零残留、裸模板不可运行、字节级模板一致性
cargo build -p child --release   # 构建 child（master 的前置依赖）
cargo build -p master --release  # 构建 master（内嵌 child 模板）
```

依赖：`wry 0.57`（default-features=false, features=devtools）、`tao 0.37`、`png 0.18`、`windows-sys 0.59`；构建期 `winres`（仅 Windows，不进运行时）。JSON/base64 手写，零 serde。
