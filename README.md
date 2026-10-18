# Web2App

<p align="center">
  <img src="assets/logo_256.png" width="96" height="96" alt="Web2App logo">
</p>

<h3 align="center">One URL in, a native desktop app out</h3>

<p align="center">
  Enter a web address (optionally a custom icon) → get a distributable desktop product.<br>
  Uses the OS built-in WebView with zero embedded browser engine: single-file <code>.exe</code> on Windows, native <code>.deb</code> on Linux.
</p>

<p align="center">
  <a href="https://github.com/31i45/Web2App/actions/workflows/ci.yml"><img alt="CI" src="https://img.shields.io/github/actions/workflow/status/31i45/Web2App/ci.yml?style=flat-square&label=CI"></a>
  <a href="https://github.com/31i45/Web2App/releases/latest"><img alt="Latest Release" src="https://img.shields.io/github/v/release/31i45/Web2App?style=flat-square&display_name=tag"></a>
  <a href="https://github.com/31i45/Web2App/releases"><img alt="GitHub Downloads" src="https://img.shields.io/github/downloads/31i45/Web2App/total?style=flat-square"></a>
  <a href="https://github.com/31i45/Web2App/stargazers"><img alt="GitHub Stars" src="https://img.shields.io/github/stars/31i45/Web2App?style=flat-square"></a>
  <a href="https://github.com/31i45/Web2App/forks"><img alt="GitHub Forks" src="https://img.shields.io/github/forks/31i45/Web2App?style=flat-square"></a>
  <a href="https://github.com/31i45/Web2App/issues"><img alt="Issues" src="https://img.shields.io/github/issues/31i45/Web2App?style=flat-square"></a>
  <a href="LICENSE"><img alt="License: MIT" src="https://img.shields.io/github/license/31i45/Web2App?style=flat-square"></a>
  <img alt="Platform: Windows | Linux" src="https://img.shields.io/badge/platform-Windows%20%7C%20Linux-4477A8?style=flat-square">
  <img alt="Rust edition 2021" src="https://img.shields.io/badge/Rust-edition%202021-DEA584?style=flat-square&logo=rust&logoColor=DEA584">
</p>

<p align="center">
  <b>English</b> · <b><a href="README.zh-CN.md">中文</a></b>
</p>

<p align="center">
  <b><a href="#-download">Download</a></b> ·
  <b><a href="#-quick-start">Quick Start</a></b> ·
  <b><a href="#-architecture">Architecture</a></b> ·
  <b><a href="#-quality-metrics--ci-gates">Quality & CI</a></b> ·
  <b><a href="#-build-from-source">Build from Source</a></b>
</p>

| Windows | Linux |
|---|---|
| ![Windows UI](assets/screenshot_windows.png) | ![Linux UI](assets/screenshot_linux.png) |

---

## What is this

Web2App is a **Web → desktop app packager**: it wraps frequently used web pages (admin consoles, internal systems, SaaS, OAuth login sites…) into desktop apps with their own window, custom icon, and isolated profile — instead of yet another browser tab.

At runtime the product reuses the OS browser engine (WebView2 on Windows, WebKitGTK on Linux); page JS, login state, and API calls behave exactly as in a browser. **Whether a product works offline depends on the site's own caching / Service Worker — the shell caches no business resources.**

The design is inspired by [Pake](https://github.com/tw93/Pake) and SiteNative, but takes a different architectural path: no Node/CLI toolchain — **a single GUI packager file produces products offline** (see [Architecture](#-architecture)).

## Fit / Not fit

- **Fit**: give a web page its own entry point and brand shell; distribute internal business systems as desktop clients; deliver lightweight web clients on domestic desktop OSes (deepin / UOS / openKylin / Kylin, plus the whole Ubuntu/Debian family).
- **Not fit**: heavy native apps that need native capabilities (system tray, global hotkeys, deep filesystem integration); scenarios requiring bundled offline resource packs; macOS (no device available — see [Platform Strategy](#platform-strategy)).

## ⬇ Download

The packager (master) itself is a **bare executable on both platforms — download and run, no installation**. Products are written next to the packager; put it wherever you like.

| Platform | File | Requirement |
|---|---|---|
| Windows x86_64 | [web2app-v\*-x86_64-pc-windows-msvc.exe](https://github.com/31i45/Web2App/releases/latest) | WebView2 Runtime (bundled with Win11, usually preinstalled on Win10) |
| Linux x86_64 | [web2app-v\*-x86_64-unknown-linux-gnu](https://github.com/31i45/Web2App/releases/latest) | WebKitGTK 4.1 / GTK 3 (glibc ≥ 2.35, build baseline ubuntu-22.04) |

Every release ships `SHA256SUMS.txt` (CI-generated, covering all artifacts):

```powershell
# Windows PowerShell
Get-FileHash .\web2app-v*-x86_64-pc-windows-msvc.exe -Algorithm SHA256
```

```bash
# Linux
sha256sum -c SHA256SUMS.txt --ignore-missing
```

## 🚀 Quick Start

**Windows**

```powershell
.\web2app.exe   # or download from Releases and double-click
```

**Linux**

```bash
chmod +x web2app-v*-x86_64-unknown-linux-gnu
./web2app-v*-x86_64-unknown-linux-gnu
```

The UI has exactly three elements: **a URL input, an icon picker (optional), and a pack button** (Enter equals submit). After packing, the product is written next to the packager, named after the URL host:

- Windows → `example.com.exe` (single file, double-click to run)
- Linux → `example.com.deb` (`sudo apt install ./example.com.deb`; uninstall with `sudo apt remove example.com` — package name matches the file name)

Icon picking uses the browser-native `input[type=file]` + `FileReader`; image bytes are passed to the native layer over IPC, **never relying on absolute file paths** — moving machines or directories does not affect packing.

**Headless / CI pack mode** (used by the end-to-end tests themselves):

```bash
web2app --pack <url> [--icon <icon.png>] [--out <product path>]
```

## Using the product

**Windows**: just double-click. The custom icon shows in Explorer, the title bar, and the taskbar (falling back to the w2a default icon when none was provided). The exe's own directory stays untouched — data lives in an isolated profile directory. An unsigned product triggers a SmartScreen prompt on first run ("Windows protected your PC"); click "More info" → "Run anyway" (see [Code Signing](#code-signing-optional)).

**Linux**: after installing the `.deb`, a launcher (with icon) appears in the application menu; you can also type `<host>` in a terminal. The data directory follows XDG and is isolated from the system browser and from other Web2App apps. When running `sudo apt install ./<host>.deb` from your home directory, apt prints an `_apt ... Permission denied` notice — this is apt's sandbox user falling back to reading the local file directly as root, and it affects neither installation nor uninstallation.

## 🧠 Architecture

A three-crate workspace serving one hard constraint: **user products are single-file and extremely lightweight**.

```
.
├── tail-core/            # Pure logic layer (zero OS deps, unit-testable anywhere)
│   └── src/              #   lib.rs pure re-export; tail.rs tail protocol + JSON escaping · CRC32;
│                         #   text.rs JS literals · base64; image.rs PNG decode/resize; url.rs URL utils
├── child/                # Child app runtime (the shipped user product, ~700 KiB)
│   ├── build.rs          #   winres compiles w2a.ico into .rsrc (Windows target only)
│   └── src/
│       ├── main.rs       #   Read tail config → open WebView; no config → fatal exit (bare template not runnable)
│       ├── icon.rs       #   PNG → tao window icon (tail_core::decode_png)
│       ├── popup.rs      #   Four-way new-window routing (pure logic, zero OS deps)
│       ├── webview.rs    #   WebView construction + script injection + all host-capability wiring
│       └── platform/     #   #[cfg] same-name same-signature platform shells (mod/windows.rs/linux.rs, no traits)
├── master/               # Packager (product name web2app, embeds the child template at compile time)
│   ├── build.rs          #   Walks up from OUT_DIR to locate the child binary (--target compatible) → include_bytes!
│   └── src/
│       ├── main.rs       #   GUI form dispatch + --pack headless entry
│       ├── ui.rs         #   Embedded HTML form + IPC parsing
│       ├── builder.rs    #   Pack orchestration: template → inject icon → append tail → (optional) sign
│       ├── tail_write.rs #   Tail writing (write capability isolated here; physically absent from child's dep graph)
│       ├── winicon.rs    #   PNG→ICO + UpdateResource .rsrc injection (Windows only)
│       ├── sign.rs       #   Optional signtool signing hook (Windows only)
│       ├── icon.rs       #   Master window icon (isomorphic to child's, no dependency on child crate)
│       └── platform/     #   Data dirs / path normalization / per-platform packing / close fallback / fatal errors
├── assets/               # w2a.ico · logo_256.png · dual-platform screenshots
├── docs/                 # Cross-platform design docs and platform seam notes
├── GLOSSARY.md           # Glossary (unified terminology across docs and code comments)
├── audit.ps1             # Dependency-boundary audit: marker scan / PE·ELF image counting / cargo tree boundary
├── build.ps1 / build.sh  # One-command build (child → tests → clippy → master → assemble)
└── e2e.ps1 / e2e.sh      # End-to-end gates (30 assertions on Windows / 37 on Linux)
```

### Why two binaries

- **child**: the minimal runtime — the product users receive. It contains no packager code: **the tail-writing capability `tail_write` is physically isolated inside the master crate; child's dependency graph contains no tail-writing code**. This is a compile-time structural guarantee, not something left to the release optimizer.
- **master (web2app)**: the packager, embedding the child template via `include_bytes!(env!("CHILD_BIN_PATH"))` at compile time. Fill the form → pack → every output byte comes from child, never from master's own binary (byte-level identity is asserted in e2e).

The two share only the tail protocol and PNG/URL utilities via `tail-core`. `audit.ps1` mechanically guards this boundary in CI: sensitive string-marker scan + PE/ELF image counting + `cargo tree` dependency assertions.

A bare child template is not runnable: with no tail config it shows a fatal error and exits with code 1, never starting any WebView.

### Tail protocol

Product = child template + tail config frame. The config JSON carries the start URL, brand title, and window-icon PNG (base64):

```
<payload JSON> <MAGIC:16B "__WEB2APP_TAIL__"> <len:u64 LE> <ver:u8> <crc32:u32 LE>
```

- The v3 frame trailer is **29 bytes**; CRC32 (IEEE 802.3) covers payload + length + version byte. A truncated or tampered tail produces an explicit "file may be corrupted" error — never a silent mis-run.
- v2 frames (24 bytes, no CRC) are still readable; stripping an old tail slices by structure, and even a corrupted frame strips cleanly — **repeated packing stays size-stable, no bloat**.
- JSON escaping, base64, and CRC32 are all hand-written so the protocol adds zero direct dependencies; `serde` never enters the runtime dependency graph (the serde in the lockfile is pulled host-side by the build-time `winres → toml` chain).

### Icons: two channels

| Channel | Where it shows | Mechanism |
|---|---|---|
| File icon | Explorer / file manager | child embeds the w2a default icon via winres at compile time; at pack time master converts the user PNG to ICO and injects it into the product's `.rsrc` section via `UpdateResource` — the product stays single-file |
| Window icon | Title bar / taskbar | The user PNG travels inside the tail config and is set at runtime via tao `with_window_icon`; falls back to the compile-time `include_bytes!` w2a default when none is provided |

> The Windows window-class icon does **not** automatically fall back to the exe's .rsrc resource — an unset icon shows the system default. So "user picked no image" must still actively set the embedded default; this is measured behavior, not speculation.

### Pack order (Windows)

1. Write the child template to a temp file;
2. Inject the user icon via `UpdateResource`, rewriting the PE `.rsrc` (must precede tail writing, otherwise `EndUpdateResource` would drop the tail);
3. Append the tail config frame (with the window-icon PNG);
4. Atomically rename to the final product;
5. Optional signing (failure only warns, never blocks).

The Linux counterpart assembles a `.deb` (`/usr/bin/<host>` ELF + `.desktop` + hicolor 256px icon + `doc/copyright`); there is no PE resource rewriting step.

### Deterministic shutdown

Measured (tao 0.37 + wry 0.57 / WebView2): on a single `WM_CLOSE` path (Alt+F4, taskbar close, `taskkill` without `/F`, UI automation), tao's `CloseRequested` may never dispatch — the message is swallowed by WebView2's nested COM message pump, leaving the process alive 30s+. Two layers of fallback:

- **WNDPROC sentinel** (`install_close_watchdog`, installed before WebView construction): `WM_CLOSE` is captured before entering the framework, arms the watchdog, then passes through unconditionally — the event chain stays intact;
- **Watchdog thread** (`quit_watchdog`): an independent thread sleeps **300 ms** then calls `process::exit(0)`, sidestepping the message-pump race with a deterministic exit ceiling.

"Close means exit, zero residue" is not luck: e2e asserts no surviving host/WebView child processes for the main window, popups, and master itself.

### Profile isolation

The data directory is `<data root>/Web2App/apps/<host>_<URL fingerprint>` (`%LOCALAPPDATA%` on Windows, XDG on Linux): different apps under the same host never share cookies/storage/cache, while repeated packing of the same URL shares one profile. Zero file drops next to the exe (Linux also never leaks the default webkitgtk directory — e2e asserts this).

### Host-side capability completion

wry disables a batch of native WebView2/WebKitGTK features by default. The product restores them one by one toward "desktop-browser-equivalent experience", adding zero new runtime crates:

| Capability | Background & implementation |
|---|---|
| New-window routing | `target=_blank` / `window.open` are silently dropped by default (the root cause of "links that do nothing" on some sites). `classify_new_window` routes four ways: **popups** (URL-less `window.open()` or size/position-featured) open as independent native windows in-app, sharing the initiator's WebView2 environment so OAuth sessions continue; **same-site** (including mutual subdomains) navigates in place; **cross-site** plain links go to the system default browser; `blob:`/`data:`/`javascript:` and other pseudo-protocols are ignored |
| HTML5 true fullscreen | An injected script listens to `fullscreenchange` and reports to the host, which calls `set_fullscreen`; popups get peer support (Win32 window-style switching). WebView-internal fullscreen only fills the webview — it is not true window fullscreen |
| Navigation & zoom | `with_hotkeys_zoom` for zoom, `with_back_forward_navigation_gestures` for touchpad swipes; Alt+←/→, mouse side buttons, and other WebView2 browser shortcuts keep their defaults — the history stack stays intact for continuous back-navigation |
| Back to home | **Alt+Home** delivered on two layers: page-layer `keydown` (with hit detection, BFCache jump-back, in-flight guard) + host-layer `AcceleratorKeyPressed` (direct `webview2-com`, covering the case where focus sits inside a cross-origin iframe), deduplicated by a 200 ms timestamp. A home entry in history uses the Navigation API `traverseTo` (BFCache instant); otherwise a full navigation. Main window and popups share the same home |
| Clipboard | `with_clipboard(true)` (off by default). **Known limit**: wry's permission callback carries no request origin, so it is all-or-nothing; global allow was chosen to keep paste-style interactions working |
| Downloads | Both platforms behave identically: the file is saved silently to the system default download directory, plus a lightweight in-page success/failure toast. wry 0.57 already registers an allow-all "download started" handler by default, and that handler sets `Handled = TRUE` — which, per WebView2 semantics, **hides** the native download dialog. Its public API offers no way to clear the handler, so the native panel stays unavailable |
| Title sync | `document.title` flows back to the main loop: the main window keeps the brand title, popups follow the document title |
| `window.close` guard | A page's `window.close()` on a top-level window degrades to a no-op (matching browser semantics for "top-level windows not opened by script"); a popup's `window.close()` closes normally |
| Theme follow | Both page `prefers-color-scheme` and the window title bar follow the system light/dark theme |

## Runtime requirements

| Platform | Requirement | Notes |
|---|---|---|
| Windows | WebView2 Runtime | [Official download](https://developer.microsoft.com/microsoft-edge/webview2/); bundled with Win11, preinstalled on most Win10 |
| Windows | Code signing | Optional. Unsigned runs fine with a one-time SmartScreen prompt; signed shows no prompt |
| Linux | No manual steps | The `.deb` Depends declares the full chain (`libwebkit2gtk-4.1-0`, `libgtk-3-0`, `gstreamer1.0-libav`, `gstreamer1.0-plugins-bad`); `apt install` resolves it automatically |

### Code signing (optional)

Signing is an operations concern, not an architecture one. Web2App reserves a clean signing hook (`master/src/sign.rs`):

- Set `WEB2APP_SIGN_CERT` (path to a .pfx/.p12 certificate) and packing automatically invokes `signtool`;
- Optional `WEB2APP_SIGN_PASS` (certificate password) and `WEB2APP_SIGN_TSA` (timestamp server URL);
- Signing failure never blocks packing (warning only);
- **Private keys are never embedded in the binary** — certificates enter only via environment variables at pack time;
- Works with local signtool (ships with the Windows SDK) and with remote signing services (swap out the `sign.rs` implementation).

## 📊 Quality metrics & CI gates

Size numbers are measured from the release profile and drift with the toolchain; **red lines are mechanically enforced by CI — exceeding them fails the build**, never left to human discipline.

| Metric | Red line / target | Current measurement |
|---|---|---|
| child.exe (user-shipped product) | **< 1 MiB, CI hard gate** | ~703 KiB (719,360 B) |
| web2app.exe (Windows packager) | **< 2 MiB, CI hard gate** | ~1.49 MiB (1,560,064 B) |
| Linux `.deb` (user-shipped product) | < 1 MB | ~430 KB (system WebKitGTK, zero embedded engine) |
| Linux packager bare ELF | — | ~1.85 MiB (1,944,880 B) |
| Close residue | Zero processes / zero temp files | e2e asserts: 300 ms watchdog ceiling, all WebView child processes exit |
| Runtime side effects | Zero file drops next to the exe; no console in release | e2e diffs directory file count before/after; `windows_subsystem = "windows"` applies to release only |
| Cold start | Seconds | Empirical (no automated benchmark yet) |

**CI pipeline** ([.github/workflows/ci.yml](.github/workflows/ci.yml) — runs on every push/PR, plus release on `v*` tags):

| Stage | Windows (windows-latest) | Linux (ubuntu-22.04 pinned, glibc 2.35 baseline) |
|---|---|---|
| Unit tests | 84 green (tail-core 42 / child 19 / master 23) | Same 84 (platform shells are cfg-symmetric, same count) |
| Lint | `clippy --all-targets -D warnings`, zero warnings | Same |
| Structural audit | `audit.ps1` (string markers / PE image count / cargo tree) | ELF branch (`\x7fELF` + image counting) |
| End-to-end | `e2e.ps1`, 30 assertions: identity/resource metadata, product launch, zero residue, bare-template non-runnability, byte-level template identity | `e2e.sh`, 37 assertions, headless Xvfb: keys×focus matrix, profile isolation, deb build/install/run/uninstall, UI responsiveness during packing |
| Size red lines | child ≤ 1 MiB, web2app ≤ 2 MiB | — |
| Release | Tag-triggered: dual-platform artifacts + `SHA256SUMS.txt` → GitHub Release (direct API upload, explicit error on failure; idempotent re-publish on same tag) | |

## Platform strategy

| Platform | Product form | Status |
|---|---|---|
| Windows | Single-file `.exe` | Mainline, full support (high WebView2 preinstall rate; PE + tail append = true single file) |
| Linux | Native `.deb` (Debian family) | Full support: system WebKitGTK, `.desktop` + hicolor icon, one-command install/run/uninstall chain; targets deepin, UOS, openKylin, Kylin, and Ubuntu/Debian |
| macOS | `.app` | Not implemented for now: bundle directories conflict with the single-file promise, and signing/notarization plus window behavior need real-device verification; backends based on the existing `#[cfg]` platform shells are welcome |

Linux does not go AppImage: it depends on FUSE, which some distributions disable by default. Package-manager installation is the native convention of the Linux ecosystem, and `.deb` is the native form for the Debian family.

## 🛠 Build from source

Prerequisites: Rust stable (edition 2021). Linux additionally needs `libwebkit2gtk-4.1-dev libgtk-3-dev`; running the Linux e2e additionally needs `xvfb xdotool gstreamer1.0-libav gstreamer1.0-plugins-bad dbus-x11 at-spi2-core`.

**Windows**

```powershell
.\build.ps1                                              # child → tests → clippy → master → assemble to dist\
.\audit.ps1                                              # dependency-boundary audit
.\e2e.ps1                                                # end-to-end gate (30 assertions)
cargo test --workspace                                   # 84 unit tests
cargo clippy --workspace --all-targets -- -D warnings    # zero warnings
cargo build -p child --release                           # build child alone (prerequisite of master)
```

**Linux (Debian family)**

```bash
./build.sh    # same gate caliber as Windows
./e2e.sh      # headless Xvfb end-to-end (37 assertions)
```

Two-stage building is an inherent architectural constraint: master embeds child at compile time, so a clean target directory must build child first (the scripts handle the ordering).

**Dependency profile**: `wry 0.57` (`default-features = false`, enabling `devtools`/`os-webview`/`x11`), `tao 0.37`, `png 0.18`; Windows runtime `webview2-com 0.39` (host-layer Alt+Home, version aligned with wry's transitive dependency), `windows-sys 0.59`; Linux runtime `gtk 0.18` (links system libgtk-3, already pulled transitively by tao/wry — a direct reference adds no new dependency-graph node); build-time `winres 0.1` (never enters runtime). The release profile is uniform across the workspace: `opt-level = "z"` + `lto` + `codegen-units = 1` + `panic = "abort"` + `strip`.

## Acknowledgements

- [Pake](https://github.com/tw93/Pake) (tw93) — the excellent project that made "pack a web page into a desktop app" widely known;
- [tao](https://github.com/tauri-apps/tao) / [wry](https://github.com/tauri-apps/wry) / [WebView2](https://developer.microsoft.com/microsoft-edge/webview2/) / [WebKitGTK](https://webkitgtk.org/) — the shoulders this project stands on.

## License

[MIT](LICENSE) © Web2App Contributors
