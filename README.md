# Web2App

自繁殖单文件跨平台 Web→桌面应用打包器。输入网页 URL（可选自定义图标），生成单文件桌面应用；产物本身也是母版，可再次打包新应用（自繁殖闭环）。

受开源项目https://github.com/tw93/Pake和小暖科技的SiteNative项目启发。

## 快速开始

```powershell
# 一键构建（测试 + Release + 母版装配）
.\build.ps1

# 运行母版
.\dist\Web2App.exe
```
<img width="1654" height="1172" alt="image" src="https://github.com/user-attachments/assets/054b657d-88ec-458b-b90b-2335af94ed10" />

母版界面三元素：**网页 URL 输入框、图标选择（可选）、打包按钮**（回车也可提交）。点击「打包」，产物生成在母版所在目录（文件名 = URL host，如 `example.com.exe`）。选图走浏览器原生 `input[type=file]`+`FileReader`，图片字节经 IPC 传给原生，不依赖绝对路径。

## 使用产物

- 直接双击运行 → 打开目标网页的桌面应用窗口
- `product.exe --master` → 重新进入打包器界面，继续繁殖新应用

## 构建产物指标

| 指标 | 目标 | 实测 |
|---|---|---|
| 单文件体积 | < 10 MB | 644 KB |
| 冷启动 | < 2 s | 秒级 |
| 关闭残留 | 无 | 进程/WebView2 子进程全部退出 |
| 运行副作用 | 无 | 无后台控制台；exe 目录零落盘 |

## 架构

```
src/
├── main.rs      # 唯一入口：--master / 尾部配置分发
├── webview.rs   # 渲染层：wry(WebView2/WKWebView/WebKitGTK) + tao 窗口（深色标题栏+窗口图标）
├── ui.rs        # 母版表单（内嵌 HTML，三元素；选图用 FileReader 字节传输）
├── builder.rs   # 打包引擎：复制自身 → 追加尾部配置（含可选图标）
├── tail.rs      # 自繁殖协议：<payload JSON> <MAGIC:16B> <len:u64 LE>
└── icon.rs      # 图标域：PNG 解码与窗口图标转换（tao Icon，跨平台统一）

build.rs         # 母版 w2a 图标经 .rc 编译进 exe（winres，仅 Windows）
assets/w2a.ico   # 母版图标源（深蓝渐变 + 白色 w2a）
```

跨平台策略：同一功能优先同一实现——图标显示拆为两条通道，各自采用最可靠的机制，三平台一套代码，无手写 PE 手术。

### 自繁殖原理

母版 = 无尾部配置的裸 exe；产物 = 母版 + 尾部配置。尾部协议从文件末尾倒数 24 字节定位，天然支持覆盖旧配置。产物含全部母版能力（复制自身打包新应用），实现「生成即母版」。

### 运行时行为

- **无控制台**：release 构建为 Windows GUI 子系统（`windows_subsystem`），双击运行不出现后台终端。
- **零目录污染**：WebView 用户数据统一放系统应用数据区（Windows `%LOCALAPPDATA%`，macOS `~/Library/Application Support`，Linux `~/.local/share`），exe 旁边不产生任何文件夹。
- **深色标题栏**：窗口主题固定 `Theme::Dark`，与 UI 深色主题一致。

### 图标机制（两条通道，各自最可靠的机制）

- **文件图标**（Explorer/任务栏）：母版 w2a 图标经 `.rc` 资源脚本由链接器编译进 exe（`build.rs` + `assets/w2a.ico`）；产物字节级继承母版，零注入代码。
- **窗口图标**（标题栏/运行时任务栏）：用户选图以 base64 内嵌尾部配置，产物启动时经 tao `with_window_icon` 设置（三平台同一 API）；未选图自动回落 exe 资源图标（w2a）。
- **机制边界**：Explorer 文件列表中产物恒显示 w2a 图标（不随选图变化）；运行后标题栏与任务栏显示用户图标。

## 待解决问题

1. **跨平台限制（机制性）**：源码可编译出 Windows / macOS / Linux（x64 / ARM64）各平台母版，但自繁殖仅在单一平台内闭环——产物平台恒等于母版平台，无法由一个母版生成其他 OS 或架构的产物；且 Unix 产物缺省无执行权限位、Apple Silicon 强制签名与尾部追加协议冲突，均待处理。

## 开发

```powershell
cargo test --bin web2app    # 35 个单元测试
cargo build --release       # Release（opt-level=z, lto, strip）
```

依赖：`wry 0.57`（devtools only）、`tao 0.37`、`dunce 1`、`png 0.18`；构建期 `winres`（仅 Windows，不进运行时）。JSON/base64 手写，零 serde。
