# Web2App 跨平台设计定稿（Linux `.deb` / macOS `.app`）

> 状态：**设计定稿（Design Freeze）**。本文档是实施的唯一权威来源（single source of truth）。
> 本文档仅描述**要做什么、按什么顺序做、如何验收**，不包含代码改动。
> 适用对象：实现者（人或 AI coding agent）。

---

## 0. 元信息

| 项 | 内容 |
|---|---|
| 目标 | 在 Windows 之外，原生实现 Linux（`.deb`）与 macOS（`.app`）版 Web2App |
| 硬约束 | Windows 主线**零行为回归**；不变量三平台一致；产物轻量（不含 WebView 引擎） |
| 非目标 | ① 不追求"完美打开所有网站"；② 不做跨平台打包（各平台在自己 runner 产物）；③ 不做 AppImage / 免 root 旁路；④ 不把项目全流程外包给 AI |
| 当前版本 | workspace `2.1.1`；crates：`tail-core` / `child` / `master` |
| 文档语言 | 中文正文；标识符、路径、命令保持英文 |

**术语**

- **不变量（invariant）**：与 OS 无关、必须三平台同样成立的性质。
- **偶然实现（incidental）**：为满足不变量而采用的平台特定手段，允许各平台不同。
- **接缝（seam）**：纯逻辑与平台外壳之间的边界。
- **门禁（gate）**：可机械判定（exit code）或明示的人工 checklist，通过才允许进入下一阶段。

---

## 1. 第一性原理：不变量 vs 偶然实现

### 1.1 不变量（♻️ 必须三平台 100% 成立）

| ID | 不变量 | 判据 |
|---|---|---|
| INV-1 | 配置与二进制合一（单产物交付） | 产物 = 模板 + 尾部配置；运行时 `read_tail()` 即可获得 |
| INV-2 | **结构性不可繁殖** | 打包能力物理隔离在 `master`；`child` 依赖图不含任何写尾/打包代码 |
| INV-3 | 关闭确定性、零残留 | 关闭后进程 / WebView 子进程 / 临时文件全部释放 |
| INV-4 | 站点**原样呈现** | 不改造 DOM；仅注入"宿主能力桥"（全屏上报、`window.close` 拦截、回首页） |
| INV-5 | 轻量 | 产物不含 WebView 引擎，依赖系统引擎 |

### 1.2 偶然实现（🔧 各平台各写各的，写薄、可弃）

建窗方式、按键拦截机制、关闭钩子、文件图标注入、打包格式与签名。

> **设计铁律**：不变量必须跨平台完全保持；偶然实现**禁止**为统一而抽象——见 §11 ADR-1/ADR-2。

---

## 2. 目标产物形态

| 平台 | 产物 | 说明 |
|---|---|---|
| Windows | 单 `.exe` | 现状不变，尾追加配置 |
| Linux | `.deb` | 安装到系统；配置仍尾追加进 **ELF**（加载器忽略尾随字节）；图标走 `.desktop` + hicolor |
| macOS | `.app`（bundle） | `Contents/MacOS/<bin>` + `Info.plist` + `.icns`；**尾追加后必须重签** |

---

## 3. 架构

### 3.1 分层

```
tail-core   纯逻辑层（协议 / JSON / base64 / PNG / URL）——零 OS 依赖，可任意宿主单测
child       运行时：入口 + 注入桥(纯 JS) + 路由判定(纯) + platform/{win,linux,mac} 同名外壳
master      打包器：编排(纯) + platform/{win,linux,mac} 打包器 + 表单(纯 HTML)
```

### 3.2 crate 布局（⚠️ 不新增 crate）

```
tail-core/src/lib.rs          # 纯化：移除所有 OS 触碰项（见 T-0a-1）
child/src/
  main.rs                     # 入口：read_tail → open_webview
  webview.rs                  # 跨平台装配 + 注入桥 JS（ChildEvent 总线）
  popup.rs                    # 路由判定（纯）保留；窗口管线移入 platform/
  platform/mod.rs            # 分发（见 §3.3）
  platform/windows.rs|linux.rs|macos.rs
master/src/
  ui.rs / builder.rs / tail_write.rs   # 跨平台
  platform/mod.rs            # 打包器分发
  platform/windows.rs|linux.rs|macos.rs
```

### 3.3 平台外壳接口（同签名 `#[cfg]` 模块 —— **不用 trait**）

```rust
// child/src/platform/mod.rs
#[cfg(target_os = "windows")] mod windows;
#[cfg(target_os = "windows")] pub(crate) use windows::*;

#[cfg(target_os = "linux")]   mod linux;
#[cfg(target_os = "linux")]   pub(crate) use linux::*;

#[cfg(target_os = "macos")]   mod macos;
#[cfg(target_os = "macos")]   pub(crate) use macos::*;
```

每个平台模块**必须提供同名同签名**的下列项（缺一即编译失败，这是刻意设计）：

```rust
pub(crate) fn data_dir(sub: &str) -> PathBuf;          // INV-4 隔离
pub(crate) fn install_keys(webview: &WebView, proxy: EventLoopProxy<ChildEvent>, hwnd: RawWindowHandleInfo);
pub(crate) fn create_popup(url: &str, feat: NewWindowFeatures, home: &str) -> Option<NewWindowResponse>;
pub(crate) fn navigate_popup(handle: isize, url: &str);
pub(crate) fn close_all_popups();
pub(crate) fn set_fullscreen(handle: isize, on: bool);
pub(crate) fn shell_open(url: &str);
pub(crate) fn fatal(msg: &str) -> !;
pub(crate) fn install_close_watchdog(handle: isize);
```

`master` 侧同理：`pack(out: &Path, url: &str, icon_png: Option<Vec<u8>>) -> io::Result<BuildOutput>` + `maybe_sign(path: &Path) -> io::Result<()>`。

### 3.4 编码规范（硬性）

- 平台分叉**只用 `#[cfg(target_os = "…")]`**，**禁止 feature 门控**（resolver=2 下 `cargo build --workspace` 会统一 feature，门控形同虚设）。
- 属性改写：`#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]`。
- 不新增运行时依赖（Linux 侧 `gtk` 为系统库链接，非引擎内嵌——见 R-6）。

---

## 4. 复用清单（逐项，实施时对照）

### 4.1 ♻️ 100% 逐字复用（**禁止改写**）

`tail-core/src/lib.rs`：`TAIL_MAGIC` / v3 帧编解码 / `read_tail` / `extract_tail` / `strip_tail` / CRC32 / `AppConfig`(`url,title,icon_png`) / 手写 JSON / base64 / `js_string_literal` / PNG 解码与 `resize_box` / `default_title` / `sanitize_host` / `host_of` / `url_fingerprint`。

`child/src/webview.rs`：`ChildEvent{OpenInCurrent,Fullscreen,SetTitle,Toast,GoHome}` / `INJECT_SCRIPT` / `CLOSE_GUARD` / `home_hotkey_script`（`homeAction`、`isHome`、hash 判定、在途守卫、200ms 去重、`__w2aHome` 锁定）/ `with_*` 跨平台 builder 装配。

`child/src/popup.rs`：`NewWindowAction{SameTab,External,Popup,Ignore}` / `classify_new_window` / `is_blank` / `is_http_url` / `is_internal_pseudo` / `same_site`。

`master/src`：`ui.rs` 表单与 IPC / `builder.rs` 编排骨架 / `tail_write.rs` 全部。

### 4.2 ⚙️ cfg 门控（同一份代码 + 少量 cfg）

`current_exe` 去 `\\?\` 前缀；`set_theme`（Windows-only，Unix 多半 no-op，**不得宣称平价**）；数据目录；`build.rs` 产物名（`child.exe` → `child`）；缩放热键接线；审计脚本判据。

### 4.3 🔨 平台重写（同签名 cfg 模块）

弹窗窗口管线及其导航/关闭；宿主层按键机制；致命错误呈现；关闭看门狗；文件图标与签名；master 打包器。

> **重要**：重写后的 Unix 侧应比 Windows 侧**更短**——Windows 的裸 Win32 消息泵（`PostMessageW`/`WM_APP_HOME`/`DefWindowProcW` 子类化）是 Win32 约束的产物，在 GTK/Cocoa 上退化为普通闭包。**移植 ≠ 照搬**。

---

## 5. 能力与快捷键覆盖矩阵

### 5.1 快捷键矩阵（**逐条重建，非"引擎白送"**）

> 事实：Windows 的一大堆快捷键来自 **WebView2/Chromium 默认加速键**，并非本代码实现；WebKit 系**不内置**浏览器级快捷键 → 必须显式接线。

| 按键 | Windows 机制 | Linux 机制 | macOS 机制 |
|---|---|---|---|
| 回首页 | `AcceleratorKeyPressed` | `gtk::AccelGroup`/snooper（见 V-3） | `NSMenuItem` ⌘⇧H |
| 后退 / 前进 | 引擎默认（须实测 V-1） | **必须接线**（优先组件级 `key-press-event`） | View 菜单 ⌘[ / ⌘] |
| 刷新 | 引擎默认（V-1） | **必须接线** | View 菜单 ⌘R |
| 缩放 ±/0 | `with_hotkeys_zoom` | `WebViewExtUnix::zoom` + 接线 | `setPageZoom` + 菜单 |
| 查找 / 打印 | 须实测 V-1 | 自建 | 菜单项 |
| 复制/粘贴/全选/撤销 | 内容层 | 内容层 | **依赖标准 Edit 菜单**（否则 ⌘C/V 失效） |
| 视频全屏 | 注入桥 + 宿主全屏 | 需处理 WebKitGTK `enter-fullscreen`（V-4） | 原生支持 |

**铁律**：**绝不注册编辑类键**（Ctrl/⌘ + C/V/X/A、撤销/重做）——app 级加速器抢占即毁掉输入。

### 5.2 macOS 主菜单（**强制基础设施，非可选**）

```
App     : About / Quit
Edit    : cut/copy/paste/selectAll/undo/redo      ← 缺失则 ⌘C/V 完全失效
View    : ⌘R 刷新 / ⌘[ 后退 / ⌘] 前进 / ⌘⇧H 首页 / ⌘+ ⌘- ⌘0 缩放
Window  : Minimize / Zoom
```

弹窗内菜单快捷键可能因 wry 的 child-webview `performKeyEquivalent` 局限失效 → **必须有 `NSEvent` local monitor 兜底**（V-5）。

### 5.3 网站兼容性分级（**替代"完美打开所有网站"这一过度承诺**）

| 级别 | 内容 | 结论 |
|---|---|---|
| **A** | 静态站、SPA（React/Vue）、hash/history 路由、多页站、OAuth 弹窗 | ✅ 可行 |
| **B** | H.264/AAC 视频 | ⚠️ Linux 依赖 GStreamer 插件；deb 必须声明 `Depends: gstreamer1.0-libav`（+ plugins good/bad），否则**能打开但不放视频** |
| **C** | DRM（Widevine/EME） | ❌ **引擎级天花板**：WKWebView 基本无；WebKitGTK 需 CDM；WebView2 情况以实测为准 |

**另外两项须实测**：Linux 侧 `WebContext.data_directory` 是否真隔离（否则跨应用共享 cookie/localStorage，破坏 INV-4，见 V-6）；`home_hotkey_script` 的 Navigation API `traverseTo`/BFCache 路径在旧 WebKitGTK 缺失时能否优雅回退（V-7）。

---

## 6. 打包

### 6.1 Windows（不变）
`CHILD_TEMPLATE` → 图标注入（`UpdateResource`，**必须在尾追加之前**）→ `write_tail` → 可选 `signtool`。

### 6.2 Linux `.deb`
```
master(Linux CI) → 内嵌 child ELF → 尾追加(target/child) → dpkg-deb 产包
payload: /usr/bin/<name>(ELF+tail)  /usr/share/applications/<name>.desktop
         /usr/share/icons/hicolor/<sz>/apps/<name>.png  /usr/share/doc/<name>/
control: Package/Version/Architecture/Maintainer/Description
Depends: libwebkit2gtk-4.1-0, libgtk-3-0, gstreamer1.0-libav
```
需 root 安装（**已接受的取舍**：拒绝免 root 旁路）。

### 6.3 macOS `.app`
```
Contents/MacOS/<name>  Contents/Resources/<name>.icns  Contents/Info.plist
顺序：装配 bundle → 向 Mach-O 尾追加配置 → codesign --force -s - (或正式证书)
```
**尾追加会破坏 Apple Silicon 链接器自签 → 不重签则 `.app` 无法启动**（V-8）。

### 6.4 明确拒绝
在 Windows 上跨平台打包（无 `dpkg-deb`/`codesign`）。

---

## 7. 实施阶段与任务分解（AI coding 友好）

> 每阶段结束**必须**满足该阶段"门禁"；门禁未通过**不得**进入下一阶段。
> 每任务给出：产出文件 / 验收命令 / DoD。

### 阶段 0a —— 机械迁移（零行为变更）
**原则**：只搬代码、不改逻辑、不发明抽象。

| 任务 | 产出 | 验收 |
|---|---|---|
| T-0a-1 | 把 `show_fatal_and_exit` / `arm_close_watchdog` / `quit_watchdog` / `data_base_dir` / `strip_verbatim_prefix` 从 `tail-core` 移出 | `tail-core` 无 OS 符号（`grep -i "windows\|unix\|gtk\|cocoa"` 无命中，除注释） |
| T-0a-2 | 建立 `child/src/platform/` 与 `master/src/platform/` 骨架，Windows 实现即现有代码 | Windows 编译通过 |
| T-0a-3 | 属性与命名 cfg 化（`windows_subsystem`、`child.exe`） | 同上 |

**门禁 G-0a（硬）**：`.\build.ps1` + `.\audit.ps1` + `.\e2e.ps1` **全绿**；`cargo test` 83 项全过；`clippy -D warnings` 零告警。

### 阶段 0b —— 由 Linux 逼出接缝
**原则**：**不预设计**；接缝在 T-0b-1 过程中显形，由 Windows+Linux 两实现共同确认。
| 任务 | 产出 | 验收 |
|---|---|---|
| T-0b-1 | 按 §3.3 补齐 `platform/linux.rs` 各同名函数 | Linux 可编译（CI ubuntu-latest） |
| T-0b-2 | 记录接缝变更笔记（供 macOS 消费） | 文档更新 |

**门禁 G-0b**：G-0a 保持绿 + Linux 编译绿。

### 阶段 1 —— Linux `.deb`
| 任务 | 内容 |
|---|---|
| T-1-1 | GTK 外壳：建窗 / 外发 / 弹窗 / 全屏 / 关闭（`delete-event` + 看门狗） |
| T-1-2 | **按键拦截**：按 V-3 试三种候选，落定一种 |
| T-1-3 | 数据目录 XDG + `data_directory` 隔离验证（V-6） |
| T-1-4 | master Linux 打包器（`dpkg-deb` + `control` + `.desktop` + hicolor） |
| T-1-5 | 审计脚本 ELF 判据（`\x7fELF` + `e_shoff`，无内嵌可执行体） |

**门禁 G-1（全部 exit code）**：按键 × 焦点矩阵（页面/跨源 iframe/可编辑框）；ELF 审计；`dpkg-deb -I/-c` 结构校验；`data_directory` 隔离；`.deb` 可装可运行。

### 阶段 1.5 —— macOS 探针（**throwaway，禁止写生产代码**）
| 未知数 | 验证方式 |
|---|---|
| 弹窗内菜单快捷键（wry child-webview 局限） | 真机 spike + `NSEvent` local monitor |
| `Edit` 菜单使 ⌘C/V 生效 | 真机 spike |
| 尾追加 + 重签可启动 | 真机 spike（Apple Silicon） |

**门禁 G-1.5**：三个未知数各有**书面结论**（含复现脚本）。

### 阶段 2 —— macOS `.app`
| 任务 | 内容 |
|---|---|
| T-2-1 | Cocoa 外壳（`NSWindow` / `NSWorkspace` / `toggleFullScreen:` / `applicationShouldTerminate:`） |
| T-2-2 | 主菜单（§5.2）+ `NSEvent` 兜底 |
| T-2-3 | master macOS 打包器（`.icns` + `Info.plist` + **尾追加后重签**） |
| T-2-4 | 审计脚本 Mach-O 判据（`0xfeedfacf` / `LC_*`） |

**门禁 G-2**：全部真机黄金场景通过（V-5、V-8 等）。

---

## 8. 测试与门禁总表

| 层 | 手段 | 位置 |
|---|---|---|
| 纯逻辑 | 单测（现有 83 → 只增不减） | `cargo test --workspace` |
| Linux 外壳 | **无头 CI 可测** | 按键矩阵 / ELF 审计 / deb 结构 / 隔离 |
| macOS 外壳 | 真机黄金场景（明示人工门禁） | 复现脚本 + 结论文档 |
| Windows 回归 | audit / e2e | `.\build.ps1`、`.\audit.ps1`、`.\e2e.ps1` |

**门禁原则**：凡不能被机械判定（或有明确人工 checklist）的"正确性"，一律**不算门禁**。

---

## 9. 风险与证伪门禁（未知数台账）

| ID | 未知数 | 验证方式 | 若不成立的对策 |
|---|---|---|---|
| V-1 | Windows 基准快捷键是否真由引擎提供 | 在现有 exe 逐条实测，产出**基准表** | 显式决定"匹配基准"或"刻意超出" |
| V-3 | GTK 按键拦截归属（AccelGroup 可能看不到 F5/Ctrl+R） | 无头 CI 按键 × 焦点矩阵，试三种候选 | 改用组件级 `key-press-event` |
| V-4 | WebKitGTK 视频全屏是否需处理 `enter-fullscreen` | Linux 实测 | 显式接线 |
| V-5 | wry child-webview `performKeyEquivalent` 影响弹窗菜单键 | macOS 真机 spike | `NSEvent` local monitor 兜底 |
| V-6 | Linux `WebContext.data_directory` 是否隔离 | CI 检查 cookie/localStorage 路径 | 显式传 per-app 数据目录 |
| V-7 | WebKitGTK 缺 Navigation API 时回首页回退路径 | Linux 实测 | 确认降级为整页导航，调整声明 |
| V-8 | 尾追加破坏 macOS 自签 | 真机 spike | 打包后强制 `codesign` |

---

## 10. 明确不做（防止 AI 过度设计）

- ❌ 不抽 `trait` 做宿主抽象（ADR-1）
- ❌ 不新增 crate（ADR-2）
- ❌ 不预设计接缝（ADR-3）
- ❌ 不引入 `serde` 等新依赖
- ❌ 不做跨平台打包
- ❌ 不默认伪装 Chrome UA（如需，须单独立项决策）
- ❌ 不宣称"完美打开所有网站"
- ❌ 不从零重写 Windows 已有代码

---

## 11. 决策记录（ADR）

| ID | 决策 | 被击落的替代方案 | 理由 |
|---|---|---|---|
| ADR-1 | 同签名 `#[cfg]` 模块，**不用 trait** | trait 适配层 | `NewWindowResponse::Create` 负载三平台异质（`gtk::Widget`/配置对象/`ICoreWebView2`），trait 必然泄漏平台类型 |
| ADR-2 | **不新增 crate**；`tail-core` 纯化 | 新建 `web2app-core`（含平台半边） | 共享库一旦含 OS 半边就不再是共享库，且污染测试纯度 |
| ADR-3 | 接缝**由第二实现逼出**，不预设计 | 阶段 0 一次性设计契约 | rule-of-two：预设计会被 GTK 塑形、到 Cocoa 返工 |
| ADR-4 | **Linux 先行**，macOS 后 | "macOS 先"或"同时开发" | Linux 是最大差异平台→定义接缝；macOS 有真机 + 重签硬门控；抽象未冻结时并行是负收益 |
| ADR-5 | 阶段 0 拆 **0a/0b** | 一次性大重构 | 保证 Windows 零行为回归（G-0a 全程保绿） |
| ADR-6 | macOS 允许**并行探针**、禁止并行实现 | 完全冻结 macOS | 探针防止 Linux 把 macOS 无关假设固化，且不污染生产码 |
| ADR-7 | 交付形态：单 exe / deb / .app | 三平台都单文件 | Linux/macOS 结构上不可能单文件；已接受 |
| ADR-8 | 承诺改为**"可测覆盖 + 明示天花板"** | "完美覆盖所有快捷键/功能" | DRM、Chrome 专属特性、引擎渲染差异是引擎级天花板，架构不可修复 |

---

## 12. 验收总览（Definition of Done）

- [ ] G-0a：Windows audit/e2e/test/clippy 全绿（零行为回归）
- [ ] G-0b：Linux 编译绿，接缝笔记归档
- [ ] G-1：Linux 四项无头门禁 + deb 可装可跑
- [ ] G-1.5：macOS 三未知数有书面结论
- [ ] G-2：macOS 真机黄金场景全过
- [ ] 覆盖率：单测数 ≥ 83 且只增不减
- [ ] INV-1~INV-5 在三平台各有对应门禁证据
- [ ] 每项天花板（B/C 级）在 README 平台矩阵如实披露
