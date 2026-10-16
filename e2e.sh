#!/usr/bin/env bash
# e2e.sh — Linux 无头门禁（G-1，全部以 exit code 判定；镜像 e2e.ps1 的角色）。
#
# 依据 docs/CROSS_PLATFORM_DESIGN.md §7 G-1 / §8「Linux 外壳：无头 CI 可测」。
# 依赖：Xvfb、xdotool、python3（ubuntu-latest 与本仓库 VM 均具备）；被测对象是
# 真实的 tail 追加产物（非测试替身），由本脚本用 python 标准库按尾部协议 v3 现做——
# 与 e2e.ps1::New-Product 同构（CRC32 用 zlib，布局常量以 tail-core::TAIL_V3_FOOTER_LEN
# 为准，此处同样是**漂移点**）。
#
# 测试夹具（页面）由本脚本生成进临时目录，不进仓库——仓库面只增这一个脚本。
# 关闭走 WM_DELETE_WINDOW（xdotool windowclose 会退化为强制销毁，见 T-1-1 探针记录）；
# 按键一律单发（按住式 keydown 会触发 X 自动重复污染断言，见 T-1-2 探针记录）。
set -u
ROOT="$(cd "$(dirname "$0")" && pwd)"
CHILD="${CHILD:-$ROOT/target/debug/child}"
TMP="$(mktemp -d /tmp/w2a-e2e.XXXXXX)"
FAIL=0
APP=""
GUI=""

assert() { # assert <name> <cmd...>
  local name="$1"; shift
  if "$@"; then echo "  PASS  $name"; else echo "  FAIL  $name"; FAIL=$((FAIL + 1)); fi
}
# 轮询窗口标题直到等于期望（约 5s 上限）；回显 0/1。
wait_title() { # wait_title <wid> <expected>
  local wid="$1" want="$2" i t
  for i in $(seq 50); do
    t="$(DISPLAY=:99 xdotool getwindowname "$wid" 2>/dev/null || true)"
    [ "$t" = "$want" ] && return 0
    sleep 0.1
  done
  echo "    (last title: '${t:-<none>}')" >&2
  return 1
}
wait_title_contains() { # 同上但子串匹配（弹窗发现用）
  local wid="$1" want="$2" i t
  for i in $(seq 50); do
    t="$(DISPLAY=:99 xdotool getwindowname "$wid" 2>/dev/null || true)"
    case "$t" in *"$want"*) return 0 ;; esac
    sleep 0.1
  done
  return 1
}
focus_and_click() { # focus_and_click <wid> <x> <y>
  DISPLAY=:99 xdotool windowfocus --sync "$1" 2>/dev/null
  DISPLAY=:99 xdotool mousemove --window "$1" "$2" "$3" 2>/dev/null
  DISPLAY=:99 xdotool click 1 2>/dev/null
}
# 点击导航链接并等待生效（最多 3 次——首击可能被未就绪的 GTK/WebKit 吞掉，
# 实测窗口出现即点会丢首击，dbg1 记录）。
goto_away() { # goto_away <wid>
  local wid="$1" i
  for i in 1 2 3; do
    focus_and_click "$wid" 150 50
    wait_title "$wid" AWAY && return 0
    sleep 0.5
  done
  return 1
}
# 发送 WM_DELETE_WINDOW（真实关闭路径；xdotool windowclose 会退化为强制销毁，
# 见 T-1-1 探针记录）。[1]/[3]/[4] 三处复用。
send_wm_delete() { # send_wm_delete <wid>
  python3 - "$1" <<'EOF'
import ctypes, sys
x11 = ctypes.CDLL("libX11.so.6")
x11.XOpenDisplay.restype = ctypes.c_void_p
x11.XInternAtom.restype = ctypes.c_ulong
x11.XInternAtom.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int]
x11.XSendEvent.restype = ctypes.c_int
x11.XSendEvent.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int,
                           ctypes.c_long, ctypes.c_void_p]
x11.XFlush.argtypes = [ctypes.c_void_p]
d = x11.XOpenDisplay(None)
win = int(sys.argv[1], 0)
wm_protocols = x11.XInternAtom(d, b"WM_PROTOCOLS", 0)
wm_delete = x11.XInternAtom(d, b"WM_DELETE_WINDOW", 0)
ev = ctypes.create_string_buffer(192); base = ctypes.addressof(ev)
def put(off, val, ctype):
    ctypes.memmove(base + off, ctypes.byref(ctype(val)), ctypes.sizeof(ctype))
put(0, 33, ctypes.c_int); put(8, 0, ctypes.c_ulong); put(16, 0, ctypes.c_int)
put(24, d, ctypes.c_void_p); put(32, win, ctypes.c_ulong)
put(40, wm_protocols, ctypes.c_ulong); put(48, 32, ctypes.c_int)
put(56, wm_delete, ctypes.c_long); put(64, 0, ctypes.c_long)
x11.XSendEvent(d, win, 0, 0, base); x11.XFlush(d)
EOF
}
# 发送 _NET_WM_PING 并等待 GDK 回执（[4] 主循环响应性探针，mutter 同款机制）。
# 回执语义：GDK 收到 ping 后向 root 重发同一 ClientMessage——message_type=
# WM_PROTOCOLS 不变，data.l[0]=_NET_WM_PING、l[1]=我方 token 原样带回、l[2]=被探测窗口。
# （首版误把 message_type 与 _NET_WM_PING 原子比较——永不匹配，回执在场也判死，
#  探针自误；空闲自检是红绿可信的前提。）
# 全函数逐个声明 argtypes：缺省时 Python int 按默认 c_int（32 位）传参，64 位
# Display*/Window 被截断 → Xlib 解引用段错误（首版实测踩坑）。
ping_once() { # ping_once <wid>；exit 0 = 主循环存活
  python3 - "$1" <<'EOF'
import ctypes, os, select, sys
x11 = ctypes.CDLL("libX11.so.6")
x11.XOpenDisplay.argtypes = [ctypes.c_void_p]
x11.XOpenDisplay.restype = ctypes.c_void_p
x11.XInternAtom.restype = ctypes.c_ulong
x11.XInternAtom.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int]
x11.XSendEvent.restype = ctypes.c_int
x11.XSendEvent.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int,
                           ctypes.c_long, ctypes.c_void_p]
x11.XFlush.argtypes = [ctypes.c_void_p]
x11.XSelectInput.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_long]
x11.XDefaultRootWindow.argtypes = [ctypes.c_void_p]
x11.XDefaultRootWindow.restype = ctypes.c_ulong
x11.XConnectionNumber.argtypes = [ctypes.c_void_p]
x11.XConnectionNumber.restype = ctypes.c_int
x11.XPending.argtypes = [ctypes.c_void_p]
x11.XPending.restype = ctypes.c_int
x11.XNextEvent.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
d = x11.XOpenDisplay(None)
win = int(sys.argv[1], 0)
token = os.getpid()
root = x11.XDefaultRootWindow(d)
wm_protocols = x11.XInternAtom(d, b"WM_PROTOCOLS", 0)
net_wm_ping = x11.XInternAtom(d, b"_NET_WM_PING", 0)
x11.XSelectInput(d, root, (1 << 19) | (1 << 20))  # SubstructureNotify|SubstructureRedirect
ev = ctypes.create_string_buffer(192); base = ctypes.addressof(ev)
def put(off, val, ctype):
    ctypes.memmove(base + off, ctypes.byref(ctype(val)), ctypes.sizeof(ctype))
put(0, 33, ctypes.c_int); put(8, 0, ctypes.c_ulong); put(16, 1, ctypes.c_int)
put(24, d, ctypes.c_void_p); put(32, win, ctypes.c_ulong)
put(40, wm_protocols, ctypes.c_ulong); put(48, 32, ctypes.c_int)
put(56, net_wm_ping, ctypes.c_long)  # data.l[0] = WM_PROTOCOLS 协议原子
put(64, token, ctypes.c_long)        # data.l[1] = 时间戳位（回执原样带回）
put(72, win, ctypes.c_long)          # data.l[2] = 被探测窗口
x11.XSendEvent(d, win, 0, 0, base); x11.XFlush(d)
r, _, _ = select.select([x11.XConnectionNumber(d)], [], [], 0.15)
if not r:
    sys.exit(1)                      # 超时无回执 = 主循环未处理事件
while x11.XPending(d) > 0:
    x11.XNextEvent(d, base)
    if ctypes.c_int.from_address(base).value != 33:
        continue
    mt = ctypes.c_ulong.from_address(base + 40).value
    l0 = ctypes.c_long.from_address(base + 56).value
    l1 = ctypes.c_long.from_address(base + 64).value
    l2 = ctypes.c_long.from_address(base + 72).value
    if mt == wm_protocols and l0 == net_wm_ping and (l1 == token or l2 == win):
        sys.exit(0)
sys.exit(1)
EOF
}
cleanup() {
  [ -n "$APP" ] && kill "$APP" 2>/dev/null
  [ -n "$GUI" ] && kill "$GUI" 2>/dev/null
  [ -f "$TMP/http8100.pid" ] && kill "$(cat "$TMP/http8100.pid")" 2>/dev/null
  [ -f "$TMP/http8101.pid" ] && kill "$(cat "$TMP/http8101.pid")" 2>/dev/null
  [ -n "${XVFB:-}" ] && kill "$XVFB" 2>/dev/null
  rm -rf "$TMP" "${TMPDIR:-/tmp}/w2a-deb-web2app-127.0.0.1-8100"
}
trap cleanup EXIT

echo "== Web2App Linux E2E (G-1) =="
[ -x "$CHILD" ] || { echo "child binary missing: $CHILD (先 cargo build -p child)"; exit 1; }
for c in Xvfb xdotool python3; do command -v "$c" >/dev/null || { echo "missing: $c"; exit 1; }; done

# ---- 夹具：页面（生成进临时目录）+ 尾部产物 ----
mkdir -p "$TMP/web8100" "$TMP/web8101"
# 模板布局与 T-1-2 探针同构：pad(0,0,1100x200，左侧 300px 是链接区)/
# 输入框(100,300,320x40)/跨源 iframe(100,450,400x250)。三个焦点点击点：
# 页面=700,100（惰性 pad）、可编辑框=260,320、iframe=300,550。
PAGE_TMPL='<!doctype html><title>@TITLE@</title><meta charset=utf-8>
<style>body{margin:0}a{display:block}#pad{position:absolute;left:0;top:0;width:1100px;height:200px}
#nav-away{position:absolute;left:0;top:0;width:300px;height:100px}
#nav-popup{position:absolute;left:0;top:100px;width:300px;height:100px}
#box{position:absolute;left:100px;top:300px;width:320px;height:40px}
#fr{position:absolute;left:100px;top:450px;width:400px;height:250px;border:1px solid #000}</style>
<div id=pad>
<a id=nav-away href="@OTHER@">go</a>
<a id=nav-popup href="#" onclick="openPopup();return false"></a>
</div>
<input id=box><iframe id=fr src="http://127.0.0.1:8101/frame.html"></iframe>
<script>
function openPopup(){
  // 实测（T-1-1 冒烟 + dbg2）：WebKitGTK 的空白窗请求须由用户手势触发，且
  // window.open("about:blank","_blank") 显式两参写法可用；打开后脚本同源导航。
  var w = window.open("about:blank", "_blank");
  if (w) setTimeout(function(){ w.location.href = "away.html"; }, 300);
}
var box=document.getElementById("box");
try {
  localStorage.setItem("w2a-probe", "isolation-marker");   // [2] 隔离观测写入点
  document.cookie = "w2a_cookie=marker; path=/";
} catch (e) {}
box.addEventListener("input",function(){document.title="TYPED:"+box.value});
addEventListener("keydown",function(e){
  if(!e.altKey&&e.target.tagName!=="INPUT")document.title="PAGE_KEY:"+e.key});
addEventListener("message",function(){document.title="MSG_FROM_IFRAME"});
</script>'
echo "${PAGE_TMPL/@TITLE@/HOME}" | sed "s|@OTHER@|away.html|" > "$TMP/web8100/home.html"
echo "${PAGE_TMPL/@TITLE@/AWAY}" | sed "s|@OTHER@|home.html|" > "$TMP/web8100/away.html"
cat > "$TMP/web8101/frame.html" <<'EOF'
<!doctype html><title>FRAME</title><meta charset=utf-8>
<body style="margin:0">iframe-doc
<script>addEventListener("keydown",function(){parent.postMessage("frame-key","*")})</script>
EOF
# 尾部产物（协议 v3：payload | MAGIC:16 | len:u64 LE | ver:u8=1 | crc32:u32 LE，
# CRC 覆盖 payload||len||ver——与 e2e.ps1::New-Product 同一语义，布局漂移点见头注）。
python3 - "$CHILD" "$TMP/child-app" <<'EOF'
import sys, struct, zlib
body = open(sys.argv[1], "rb").read()
payload = b'{"url":"http://127.0.0.1:8100/home.html","title":"HOME"}'
n = struct.pack("<Q", len(payload))
crc = zlib.crc32(payload + n + bytes([1])) & 0xFFFFFFFF
open(sys.argv[2], "wb").write(
    body + payload + b"__WEB2APP_TAIL__" + n + bytes([1]) + struct.pack("<I", crc))
EOF
chmod +x "$TMP/child-app"

# ---- 环境起停 ----
pkill -x Xvfb 2>/dev/null; pkill -x child-app 2>/dev/null
pkill -x WebKitWebProcess 2>/dev/null; pkill -x WebKitNetworkProcess 2>/dev/null
pkill -f "http.server 8100" 2>/dev/null; pkill -f "http.server 8101" 2>/dev/null
# 上次失败 run 可能残留的安装产物（[3] 失败路径不保证 WM_DELETE 到达）——
# 残留窗口会在下次 [3] 轮询里被误认，必须先清
pkill -f "usr/bin/web2app-" 2>/dev/null
sleep 0.5
Xvfb :99 -screen 0 1280x900x24 >/dev/null 2>&1 &
XVFB=$!
( cd "$TMP/web8100" && exec python3 -m http.server 8100 --bind 127.0.0.1 ) >/dev/null 2>&1 &
echo $! > "$TMP/http8100.pid"
( cd "$TMP/web8101" && exec python3 -m http.server 8101 --bind 127.0.0.1 ) >/dev/null 2>&1 &
echo $! > "$TMP/http8101.pid"
sleep 1.5

# ---- [1] 按键 × 焦点矩阵（T-1-2 / G-1 第一项）----
echo "[1] Alt+Home x focus matrix (page / cross-origin iframe / editable / popup)"
export DISPLAY=:99 GDK_BACKEND=x11
# 数据目录全部落进临时 XDG（隔离观测与零污染，见 [2]）
XDG_DATA_HOME="$TMP/xdg" "$TMP/child-app" > "$TMP/app.log" 2>&1 &
APP=$!
MAIN=""
for i in $(seq 60); do
  MAIN="$(xdotool search --name HOME 2>/dev/null | head -1)"
  [ -n "$MAIN" ] && break
  sleep 0.2
done
assert "product alive"        test -n "$APP" && kill -0 "$APP" 2>/dev/null
assert "main window titled HOME" test -n "$MAIN"
[ -z "$MAIN" ] && { echo "FATAL: no main window"; cat "$TMP/app.log"; exit 1; }
sleep 1.5                                # 窗口就绪缓冲（首击吞没实测，见 goto_away）
assert "navigate to AWAY"      goto_away "$MAIN"

# (a) 焦点=页面：点击惰性 pad → 顶层文档收键 → Alt+Home 回首页
focus_and_click "$MAIN" 700 100
xdotool key p
assert "page focus proof (PAGE_KEY)" wait_title "$MAIN" "PAGE_KEY:p"
xdotool key alt+Home
assert "page focus -> back HOME" wait_title "$MAIN" HOME

# (b) 焦点=可编辑框：点入输入框 → 打字证明焦点 → Alt+Home
goto_away "$MAIN" >/dev/null
focus_and_click "$MAIN" 260 320
xdotool type --delay 60 hi
assert "editor focus proof (TYPED)" wait_title "$MAIN" "TYPED:hi"
xdotool key alt+Home
assert "editor focus -> back HOME" wait_title "$MAIN" HOME

# (c) 焦点=跨源 iframe（页面层 keydown 收不到的边界，宿主层必须兜住）
goto_away "$MAIN" >/dev/null
focus_and_click "$MAIN" 300 550
xdotool key m
assert "iframe focus proof (MSG_FROM_IFRAME)" wait_title "$MAIN" "MSG_FROM_IFRAME"
xdotool key alt+Home
assert "iframe focus -> back HOME (host layer)" wait_title "$MAIN" HOME

# (d) 弹窗宿主层 Alt+Home（Linux 弹窗仅由空白窗 window.open() 触发——wry webkitgtk
#     后端不转发 size/position，见 platform/linux.rs::create_popup 注释）
focus_and_click "$MAIN" 150 150          # POPUP 触发链（空白窗 + 脚本导航到 away）
POPUP=""
for i in $(seq 60); do
  POPUP="$(xdotool search --name AWAY 2>/dev/null | grep -vx "$MAIN" | head -1)"
  [ -n "$POPUP" ] && break
  sleep 0.2
done
assert "popup window shows AWAY" test -n "$POPUP"
if [ -n "$POPUP" ]; then
  focus_and_click "$POPUP" 100 100
  xdotool key alt+Home
  assert "popup Alt+Home -> back HOME" wait_title "$POPUP" HOME
  assert "main unaffected by popup accel" wait_title "$MAIN" HOME
fi

# ---- 关闭确定性（真实关闭路径，非强杀）----
send_wm_delete "$MAIN"
CLOSED=1
for i in $(seq 40); do kill -0 "$APP" 2>/dev/null || { CLOSED=0; break; }; sleep 0.25; done
assert "product exits via WM_DELETE" test "$CLOSED" = 0
sleep 1.5
assert "zero WebKit residue" bash -c '! pgrep -x WebKitWebProcess >/dev/null && ! pgrep -x WebKitNetworkProcess >/dev/null'

# ---- [2] data_directory 隔离（T-1-3 / V-6，复用上方运行写入的数据）----
echo "[2] WebContext.data_directory isolation (V-6)"
# 实测布局（webkit2gtk-4.1 / 2.4x）：全部站点数据落
#   <data_dir>/<host>_<fingerprint>/{cookies, localstorage/, hsts-storage.sqlite, ...}
# data_directory 未生效时引擎会写 $XDG_DATA_HOME/webkitgtk/（默认路径）——
# 该目录不存在即证明未向默认位置泄漏。
APPDIR="$(find "$TMP/xdg/Web2App/apps" -maxdepth 1 -type d -name '127.0.0.1_8100_*' 2>/dev/null | head -1)"
assert "app data dir under Web2App/apps" test -n "$APPDIR"
if [ -n "$APPDIR" ]; then
  assert "localStorage file in app data dir" bash -c "find '$APPDIR' -iname '*localstorage*' -type f | grep -q ."
  assert "cookies file in app data dir"      test -f "$APPDIR/cookies"
fi
assert "no default webkitgtk dir (no leak)" test ! -e "$TMP/xdg/webkitgtk"

# ---- [3] .deb 打包 + 可装可运行（T-1-4 / G-1 第三、五项）----
echo "[3] .deb package (master --pack -> dpkg-deb -> install -> run -> remove)"
MASTER="${MASTER:-$ROOT/target/debug/web2app}"
PKG="127.0.0.1-8100"                               # package_name(url)：_ → -，无品牌前缀（platform/linux.rs）
DEB="$TMP/127.0.0.1-8100.deb"                      # product_filename(url) = 包名.deb（同名贯穿装/卸）
[ -x "$MASTER" ] || { echo "master binary missing: $MASTER (先 cargo build -p master)"; exit 1; }
assert "master --pack exit 0" bash -c "'$MASTER' --pack 'http://127.0.0.1:8100/home.html' --out '$DEB' 2>&1"
assert "deb file exists" test -s "$DEB"
# 结构校验：dpkg-deb -I（可读、控制字段）+ -c（payload 布局 §6.2）
assert "dpkg-deb -I control fields" bash -c "dpkg-deb -I '$DEB' | grep -E 'Package: $PKG|Version:|Architecture: amd64|Maintainer:|Depends: libwebkit2gtk-4.1-0'"
dpkg-deb -c "$DEB" > "$TMP/deblist.txt" 2>&1
assert "payload: /usr/bin ELF"      grep -q "./usr/bin/$PKG\$" "$TMP/deblist.txt"
assert "payload: .desktop"          grep -q "./usr/share/applications/$PKG.desktop" "$TMP/deblist.txt"
assert "payload: hicolor icon"      grep -q "./usr/share/icons/hicolor/256x256/apps/$PKG.png" "$TMP/deblist.txt"
assert "payload: doc/copyright"     grep -q "./usr/share/doc/$PKG/copyright" "$TMP/deblist.txt"
# 安装（root，§6.2 已接受的取舍）→ 运行安装产物（真实 /usr/bin ELF+tail）→ 卸载
sudo dpkg -r "$PKG" >/dev/null 2>&1 || true         # 幂等清旧
assert "dpkg -i install" bash -c "sudo dpkg -i '$DEB' >/dev/null"
assert "installed binary executable" test -x "/usr/bin/$PKG"
XDG_DATA_HOME="$TMP/xdg2" "/usr/bin/$PKG" > "$TMP/inst.log" 2>&1 &
INST=$!
INSTW=""
# 不排除 $MAIN：旧 app 退出已被断言，且 X 窗口 ID 会被 server 回收重用——
# 新客户端可能分到与死 MAIN 相同的 ID，排除它反而排除了新窗口（实测踩坑）。
# 此刻全屏只可能有安装产物这一个 HOME 窗口。
for i in $(seq 75); do
  INSTW="$(xdotool search --name HOME 2>/dev/null | head -1)"
  [ -n "$INSTW" ] && break
  sleep 0.2
done
if [ -z "$INSTW" ]; then
  echo "    (windows tree)" >&2; xwininfo -root -tree 2>&1 | grep -E '0x[0-9a-f]+ "' >&2 || true
  echo "    (inst.log)" >&2; cat "$TMP/inst.log" >&2 || true
  printf '    (http probe: %s)\n' "$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:8100/home.html || echo ERR)" >&2
fi
assert "installed app shows HOME (ELF+tail runs)" test -n "$INSTW"
[ -n "$INSTW" ] && send_wm_delete "$INSTW"
for i in $(seq 40); do kill -0 "$INST" 2>/dev/null || break; sleep 0.25; done
assert "installed app closes via WM_DELETE" bash -c "! kill -0 '$INST' 2>/dev/null"
assert "dpkg -r remove" bash -c "sudo dpkg -r '$PKG' >/dev/null"
assert "binary removed" test ! -e "/usr/bin/$PKG"

# ---- [4] GUI 表单打包链路（真实用户路径：点击 → 打包 → deb；含主循环响应性）----
# 门禁缺口复盘：此前只测了 --pack CLI，用户实际使用的 GUI 表单链路从未被测——
# 打包在事件循环内同步执行会阻塞 GDK 主循环（停止回应 _NET_WM_PING = 桌面会话
# 「未响应」，Zorin 实测被 GNOME 判死）。
#
# 判据（红对照标定：阻塞版 master 实测打包窗口内 ~150 连发 ping 全部无回应
# = 35s 全程死锁；线程化后每发 ping 均有回执）：
#   红线 = 打包窗口内最大连续未回应 < 10（≈2s 静默，远早于桌面会话 ~5s 判死）。
#   「至少一发有回应」不成立为判据——完成边界的回应会污染（实测阻塞版也判绿）。
# 窗口界定（零竞态）：staging 目录 <tmp>/w2a-deb-<pkg> 由 pack 第一步创建、
# dpkg-deb 完成后才删除；deb 渐进写入期间 staging 一直在 →
#   「staging 存在」= 打包确实进行中（采样窗口）；「staging 消失 && deb 非空」= 完成。
#   （Return 后 IPC 起跑之前的空闲回应不采样——首版血泪：阻塞版也借此判绿。）
echo "[4] GUI form pack (click -> pack -> deb; ping-alive during pack)"
# GUI 打包产物 = master 所在目录/product_filename(url)。门禁口径 = release master
#（与 CI 一致，见 ci.yml verify-linux 的 MASTER 覆盖；debug master 内嵌 debug child，
# xz 压缩 30s+，正好可作阻塞红对照的复现夹具）。
GUI_OUT="$(dirname "$MASTER")/127.0.0.1-8100.deb"
STAGING="${TMPDIR:-/tmp}/w2a-deb-127.0.0.1-8100"   # package_name(url)（w2a-deb- 为内部 staging 前缀）
rm -rf "$STAGING"; rm -f "$GUI_OUT"
XDG_DATA_HOME="$TMP/xdg3" "$MASTER" > "$TMP/gui.log" 2>&1 &
GUI=$!
GW=""
for i in $(seq 60); do
  GW="$(xdotool search --name '打包器' 2>/dev/null | head -1)"
  [ -n "$GW" ] && break
  sleep 0.2
done
assert "master GUI window up" test -n "$GW"
[ -z "$GW" ] && { echo "FATAL: no GUI window"; cat "$TMP/gui.log" >&2; exit 1; }
sleep 1.5
# (a) 探针自检：空闲必须回应——先证明探针自身有效，红绿才可信
IDLE=0; ping_once "$GW" && IDLE=1
assert "probe self-test (idle ping reply)" test "$IDLE" = 1
# (b) 键盘路径驱动表单（零几何依赖）：URL 输入框 autofocus，回车即提交（form_html
# 内置 Enter 处理）。不用点击定位——card 在窗口里垂直居中，坐标随内容漂移，
# 首版点击实测误开原生文件对话框（嵌套循环 + 永不打包）。
focus_and_click "$GW" 260 245          # 仅聚焦窗口（点 URL 输入区，命中 label 也等效聚焦）
xdotool type --delay 25 "http://127.0.0.1:8100/home.html"
xdotool key Return
# (c) 打包窗口内 ping 循环：只在 staging 存在（打包确实进行中）时采样
STREAK=0; MAXSTREAK=0; PINGS=0
SECONDS=0
while [ "$SECONDS" -lt 120 ]; do
  if [ -d "$STAGING" ]; then
    PINGS=$((PINGS + 1))
    if ping_once "$GW"; then
      STREAK=0
    else
      STREAK=$((STREAK + 1))
      [ "$STREAK" -gt "$MAXSTREAK" ] && MAXSTREAK=$STREAK
    fi
    [ -s "$GUI_OUT" ] && [ ! -d "$STAGING" ] && break
  else
    [ -s "$GUI_OUT" ] && [ ! -d "$STAGING" ] && break
    sleep 0.02
  fi
done
assert "UI responsive during pack (silent streak $MAXSTREAK/$PINGS)" test "$MAXSTREAK" -lt 10
assert "GUI pack produced deb" test -s "$GUI_OUT"
assert "master alive after pack" kill -0 "$GUI" 2>/dev/null
send_wm_delete "$GW"
for i in $(seq 40); do kill -0 "$GUI" 2>/dev/null || break; sleep 0.25; done
assert "master exits via WM_DELETE" bash -c "! kill -0 '$GUI' 2>/dev/null"
rm -rf "$STAGING"; rm -f "$GUI_OUT"

echo "== G-1 done: $FAIL failed =="
[ "$FAIL" = 0 ]
