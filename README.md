# MoeFlow 桌面客户端（Windows）

把 [moeflow-com/moeflow](https://github.com/moeflow-com/moeflow) 的 `frontend-v1` 前端打包成原生 Windows 应用：Tauri v2 + 系统 WebView2，内置本地反向代理、图片磁盘缓存，以及**在客户端本地完成**的 LabelPlus txt / 成品 zip 导出。

## 它是怎么工作的

前端产物挂在 `http://127.0.0.1:<端口>/` 上，由应用内置的一个 axum 服务器提供。这一步是整个方案的支点——因为浏览器里能跑的路径，在这里原样能跑：

| 前端的行为 | 为什么必须同源 |
|---|---|
| 资产路径是根绝对的（`/assets/...`、`/static/...`） | `file://` 下必然白屏 |
| `BrowserRouter` 无 `basename`，且有整页跳转的 `<a href="/dashboard/...">` | 需要 SPA fallback（未知路径回 `index.html`） |
| `/moeflow-runtime-config.json` 是根绝对路径 | 用它把 `baseURL` 指到我们的代理 |
| `token` cookie + `Authorization` 头 | 同源才不需要改鉴权 |

服务器端点：

```
GET  /moeflow-runtime-config.json   →  {"baseURL": "/api/"}
ANY  /api/*          →  流式反代到当前档位的 API（转发 Bearer、上传/下载不落内存）
GET  /__media/*      →  取图，走 LRU 磁盘缓存
GET  /__download/*   →  原生「另存为」对话框 + 流式写盘
GET  /*              →  前端静态资源 + SPA fallback
```

另有一个**独立的外壳服务器**（自己的端口，随进程生命周期运行），只提供实例选择页与设置页：

```
GET  /launcher.html  →  实例选择页
GET  /settings.html  →  设置页
GET  /               →  同 launcher.html
其它                 →  404（刻意不做 SPA fallback）
```

### 为什么外壳页不走 Tauri 的 `tauri://` 资产协议

一开始这两个页面是用 `WebviewUrl::App("launcher.html")` 加载的，结果是**一片白的窗口**，日志里什么都没有。

原因：`tauri://` 协议按一张**编译期烤进二进制**的资产表解析路径，而 cargo 在只有 `frontendDist`（`ui/`）里的文件变化时**不会重跑 `generate_context!`**。于是页面改动不会被重新嵌入，资产表一旦对不上就返回 `AssetNotFound`——表现就是一个纯白窗口，既不报错也不提示。这类失败在开发机上时隐时现，非常难查。

现在这两个页面由我们自己的 axum 服务器提供，HTML 用 `include_str!` 嵌入（普通的编译期依赖，改文件必然触发重建），并且跟主窗口走**同一条已经验证过的服务路径**。

> 附带好处：dev 与打包后的行为完全一致——不再有「dev 模式下 `cfg(dev)` 常开」这类只在某一种构建里出现的行为差异。

设置页与实例选择页**各自独立于档位服务器**：当前实例连不上时它们照样能打开。

**每个服务器档位绑定各自的固定端口。** `token` cookie 按 origin 隔离，端口不同 → 切换服务器时登录态天然隔离，不会串号（端口变了也会导致登录态丢失，所以端口是持久化的）。

## 启动流程

首次启动**不会**直接进入某个实例，而是打开实例选择页（`src-tauri/ui/launcher.html`）：

- 列出所有实例及其 API 地址，点一下即连接（预置只有尨译 MoeTran 一个）
- 「添加实例」可填站点地址，**测试连接**会按候选顺序探测并回填真正可用的 API 地址
- 勾选「启动时直接进入上次使用的实例」可跳过（默认不勾——直接落进某个服务器正是多实例客户端不该有的意外）
- 只剩一个实例时不能删除（会明确报错，而不是留下一个连不上任何地方的窗口）

### ⚠️ API 地址必须探测出来，不能拿站点地址顶上

`api_base` 有两种形态：nginx 部署在 `<站点>/api`（`proxy_pass` 的尾斜杠把 `/api` 剥掉），而尨译是**另一个源**且没有 `/api` 段。**人填的是站点地址，不是 API 地址**，所以猜不得。

早期两个编辑页都在 API 栏留空时退回站点地址——存进去的地址什么都答不上来，只会被 SPA fallback 接住。症状极容易被误判成服务端问题：

| 调用 | 结果 |
|---|---|
| `GET <站点>/v1/...` | 200 + SPA 外壳 → 前端解析 JSON 失败 |
| `POST <站点>/v1/captchas` | nginx 直接 **405** → **登录验证码永远不显示** |
| `GET <站点>/ping` | 200 + HTML（不是 `pong`） |

现在三道防线：两个编辑页保存前都会先探测（`resolveApiBase`），**并且 `upsert_profile` 命令自己也会解析**——`api_base` 为空、或等于站点地址时，它按候选顺序探测一遍，探不到就拒绝保存。不变量落在唯一写配置的地方，新增调用方不会再把这条路走回去。

判据是 `api_base_needs_resolution`：**和站点地址不同的** API 地址原样接受（有人确实知道探测探不出的地址，也不该因为服务器暂时不在线就存不进去）。

主窗口此时是隐藏的，选定实例后才显示；托盘菜单和主窗口右下角的「实例」按钮都能随时回到这个页面。

### 落点为什么不是首页

选定实例后打开的是受保护的 `/dashboard/projects`，**不是** `/`。

前端把 `/` 放在 `publicPaths` 里（`App.tsx`），未登录时它照样渲染站点主页——包括服务端配置的公告横幅。客户端直接进首页就会看到「域名调整通告」而不是登录框。

选受保护的路由则两种情形都对：

| 状态 | 结果 |
|---|---|
| 未登录 | `App.tsx` 的守卫（`!token && !publicPaths.includes(path)`）自动重定向到 `/login` |
| 已登录 | 直接进工作台，不用先划过一次登录表单 |

过期的令牌也会自愈：首次 401 会清掉 token，守卫随即重定向到登录页。

**每个实例使用独立的本地端口**，这不是随手为之：会话令牌存在按 origin 隔离的 cookie 里，端口不同 → 切换实例时登录态天然隔离，绝不会把 A 站的登录态带给 B 站。

## 支持的服务器

**预置只有一个：尨译 MoeTran**（`https://moetran.com` + `https://api.moetran.com`）。其余实例通过「添加实例」自行添加——一个默认就列着别人服务器的客户端，只会让人连错地方。

档位里的 `api_base` 是**完整前缀**，`/api/<rest>` 直接拼在它后面。这样四种部署形态都能覆盖：

| 形态 | 站点地址 | API 地址 | 说明 |
|---|---|---|---|
| 前后端同源、nginx 带 `/api` 段 | `https://moeflow.basmc.org` | `.../api` | `proxy_pass http://backend:5000/` 的尾斜杠**剥掉** `/api` |
| 同上 | `https://demo.moeflow.org` | `.../api` | 同上 |
| **前后端不同源、API 在根路径** | `https://moetran.com` | `https://api.moetran.com` | 没有 `/api` 段 |
| 内网机器 | `http://172.29.133.24` | `.../api` | 同 nginx 形态 |

> **探测不能看状态码。** `location / { try_files $uri /index.html; }` 会把任何未知路径都回成 SPA 壳 + 200，所以 `/v1/anything` 看起来「通」其实是假的。可靠判据是后端公开的 `/ping` 返回字面量 `pong`。「测试连接」按钮走的就是这个。

## 构建

前置：Node 18+、Rust stable（MSVC 工具链）、VS Build Tools 的 C++ 工作负载、WebView2 运行时（Win11 自带）。

```bash
bash scripts/fetch-frontend.sh    # 拉取并锁定上游前端（含基线 git 仓库）
npm install
npm run build:frontend            # npm ci → build:locale → vite build，并校验产物
npm run icons                     # 从 logo512.png 生成 .ico / png 全套
npm run test:rust                 # 纯逻辑单元测试（浮点格式化、LabelPlus、排序、缓存…）
npm run build                     # tauri build → NSIS 安装包
```

产物：`src-tauri/target/release/bundle/nsis/MoeFlow_0.1.0_x64-setup.exe`

MSVC 环境：若 `link.exe` 找不到库，用 `scripts/msvc-env.sh` 设置 `LIB`/`INCLUDE`（本机把工具链装在 `C:\BuildTools`，非默认路径，`vcvars64.bat` 也慢）。

### 调试

单独跑 `cargo build` 出来的二进制时，Tauri 认为自己在 dev 模式（`cfg(dev)` 是 `!custom-protocol`，而 `custom-protocol` 只有 `tauri build` 才会打开）。想看前端控制台：

```bash
cd src-tauri && MOEFLOW_DEVTOOLS=1 ./target/debug/moeflow-desktop.exe
```

窗口一片白、日志又没线索时，用 `scripts/capture-window.ps1 main|launcher|settings <输出路径>` 抓单个窗口（走 `PrintWindow`，不依赖窗口在前台——`SetForegroundWindow` 在非前台进程上会静默失败，否则你会拍到别的窗口）。要一次看全部顶层窗口用 `scripts/capture-all-windows.ps1`：只看「主窗口」会漏掉真正出问题的那个窗口，Windows 的 `MainWindowHandle` 在多窗口进程里挑哪个并没有保证。

`scripts/window-probe.ps1` 是同一套枚举的**只读**版本（不调用 `ShowWindow`），用来判断「窗口到底关掉了没有」——截图脚本会主动把窗口显示出来，会掩盖掉要观测的状态。

`scripts/click-launcher-instance.ps1 <x 比例> <y 比例>` / `scripts/click-at.ps1 -X <屏幕 x> -Y <屏幕 y>` 用真实鼠标事件驱动界面（WebView2 会忽略合成 `WM_LBUTTON` 消息），用于端到端验证「选实例 → 主窗口加载」这条路径。窗口会滚动或改尺寸时，从截图坐标换算绝对屏幕坐标比按比例点击可靠。

`scripts/resize-window.ps1 -Handle <hwnd> -Width <w> -Height <h>` 把窗口调到表单完整可见——比滚轮滚动可靠（滚轮要求指针在窗口上且窗口是活动窗口，`SetWindowPos` 没这些前提）。

> **键盘输入这条路走不通。** `SendKeys` 只投递给**前台窗口**，而我们的进程抢不到前台（`SetForegroundWindow` 对非前台进程静默失败，即便 `AttachThreadInput` + 最小化/还原也时灵时不灵）。需要往界面里输入文字的验证，别指望这条路——要么改成不依赖输入的场景，要么在测试里直接验证底层逻辑。

排查「某个站点的图片出不来」时开 `MOEFLOW_MEDIA_DEBUG=1`：它会把 API 响应里被改写的每个存储 URL、以及每个被拒绝/失败的候选 origin 打到 stderr。这类问题的答案永远是**API 回来的是什么形式的 URL**，靠猜会绕远路。

## 本地导出

「成品 zip」和「LabelPlus txt」在客户端生成，不占用服务器 worker，已缓存过的图片零网络开销。

输出严格复刻服务器 `app/tasks/output_project.py` 的结构：

```
images/<文件名>
translations.txt
project.json
errors.txt        （仅有图片取不到时）
```

`translations.txt` 的格式细节（全部逐字对照后端源码移植）：

- 头部 `1,0` / `-` / `框内` / `框外` / `-` / 注释行 —— 这三个词条走**后端 i18n**，客户端内置 zh 与 en 两套取值
- 路径行 `>>>>>>>>[目录/文件名]<<<<<<<<`，目录来自 `ancestors` 逐级拼接
- 标号行 16 个 `-`，`[序号]` 按 `rank` 升序从 **1** 开始、**每张图重新计数**
- 译文行：优先 `proofread_content`，否则 `content`；空译文**仍写一行空行**
- 换行统一为 CRLF，且是**朴素替换** `\n → \r\n`（内容里原有的 CRLF 会变成 CRCRLF，与服务器一致）
- 译文选取复刻 `default_translations_order = ["-selected","-proofread_content","-edit_time"]`
- 文件顺序复刻 `["dir_sort_name","type","sort_name"]`，其中 `sort_name` 把文件名里的数字段左补零到 6 位（所以 `2.jpg` 排在 `10.jpg` 前面）

坐标用 **Python 兼容的浮点格式化**（`py_float_str`）。Rust 的 `{}` 与 Python 的 `repr()` 都是最短往返表示，但**记法不同**：Python 在十进制指数 ≤ −4 或 > 16 时切到科学计数法，Rust 从不。不处理的话 `1e-05` 会写成 `0.00001`，导出就不再字节一致。

### ⚠️ 空白原文：必须知道的坑

服务器导出遍历**全部**原文，但唯一的原文列表接口默认过滤掉空白原文（`show_blank=False`）。而**从 LabelPlus txt 导入时，没有译文的标号会产生空白原文**——这在图片上很常见。

直接用这个接口生成 txt，这些原文会缺失，**导致后续标号编号整体错位，且不报错**。

客户端的两道防线：
1. 请求带 `show_blank=true`（`DefaultSchema` 设了 `unknown="EXCLUDE"`，对未打补丁的服务器是无害的）
2. 用 `rank` 序列是否连续来**检测**过滤行为（`Source.to_api()` 带 `rank`；rank 是稠密分配的，出现空洞就说明有原文被吞了）→ 命中就明确告警并可选择中止

要彻底消除差异，给后端打 2 行补丁：

```bash
python3 patches/apply-backend-show-blank.py /path/to/moeflow-backend
```

上游 `backend-v1` 与 iroha 定制分支都适用（两者该处代码相同）。

### 本地导出与服务器导出的差异

- 本地导出**不会**在服务器登记 `Output` 记录，因此 `project.has_output` 不会翻转，作品进度与「有成品」闸门不推进。需要推进进度时仍要走一次服务器导出。
- 同名图片（不同目录下重名）的处理与服务器一致：后者覆盖前者，zip 里只留一个条目。

## 外部依赖本地化

前端右下角的「LabelPlus PS 脚本」原本直接指向第三方主机（`files.kozzzx.com`）。现在改走应用内的 `/__ps-script`：

- 首次点击时下载并写入本地缓存，之后不再联网
- 仍然通过原生「另存为」对话框保存

**脚本本身不随安装包分发。** 它是 GPLv2，上游前端也只是提供外链而非打包——按需下载到用户自己的缓存里和浏览器原来的行为一致，随包分发则是另一回事。

## 目录

```
frontend/          上游前端（锁定 commit 的 vendor，自带 git 仓库供打补丁）
overlay/           叠加在上游之上的桌面集成（仅新增文件）
patches/           给前端的补丁 + 给后端的 show_blank 修复
scripts/           vendor / 构建 / MSVC 环境 / 窗口截图诊断
src-tauri/
  src/
    pyfloat.rs     Python 兼容的浮点 repr
    labelplus.rs   LabelPlus txt 生成
    exporter.rs    本地导出编排（含排序与译文选取）
    media.rs       LRU 磁盘缓存
    rewrite.rs     API 响应里的存储 URL 改写到本地路由
    server.rs      环路服务器：静态 + 反代 + 媒体 + 下载 + PS 脚本
    shell.rs       外壳页服务器：实例选择页 + 设置页
    profiles.rs    实例档位与连接探测
    download.rs    原生另存为
    commands.rs    IPC 接口
    lib.rs         服务器生命周期 / 窗口 / 入口页
    main.rs        窗口 / 托盘 / 单实例
  permissions/
    moeflow/       IPC 命令的 ACL 权限定义（每个命令一条）
  capabilities/
    default.json   哪些窗口 / 哪些来源可以使用这些命令
  ui/
    launcher.html  实例选择入口页（启动时显示）
    settings.html  设置窗口（实例管理 + 缓存管理）
```

### 加一条 IPC 命令要动三个地方

Tauri v2 的 ACL 是**默认拒绝**的，漏掉哪一步都只会得到「点下去没反应」：

1. `commands.rs` 里写命令，并在 `main.rs` 的 `generate_handler!` 里注册
2. `permissions/moeflow/default.toml` 里加一条 `[[permission]]`，把命令名写进 `commands.allow`
3. 该文件末尾的 `[default]` 权限集里把这个标识符加进去（`capabilities/default.json` 引用的就是它）

两个容易踩的点：

- 权限集必须写成 `[default]` 表，**不能**写成 `[[permission]] identifier = "default"`——后者生成的是一个名叫 default 的普通权限，`default_permission` 解析成 `null`，于是什么都不放行，且构建照样成功。
- 权限标识符只能用小写字母、连字符和**一个**冒号前缀，所以命令名里的下划线要换成连字符（`boot_payload` → `allow-boot-payload`），且不能直接在 capability 里写裸命令名。

主窗口与两个外壳窗口都是从 `http://127.0.0.1:<端口>` 加载的——对 Tauri 来说属于**远程来源**，capability 里的 `remote.urls` 少了它们，`window.__TAURI__` 确实存在，但每次调用都会被静默拒绝。

### 不要在窗口事件回调里同步调用窗口方法

`on_window_event` 的闭包跑在**事件循环线程**上，而 `is_visible()` / `show()` 这些方法会把调用派发回事件循环并**等它答复**。在回调里同步调用它们 = 自己等自己。

它只在另一个窗口正好忙的时候才卡住（webview 正在加载、导航中），所以表现是**间歇性的**，很容易被当成偶发问题放过去。要动别的窗口就交给 `async_runtime::spawn`，别在回调里直接做。


## 缓存

默认上限 2 GB，按 LRU 淘汰，可在设置里调整或清空。缓存目录在应用缓存目录下的 `media/`，键是上游 URL 的 SHA-256。

URL 改写是**尽力而为**的：只改写已知媒体域名下、且出现在媒体字段（`cover_url` / `url` / `avatar` / …）里的绝对 URL。没命中的 URL 保持原样、照常直连加载——漏改写只损失缓存，不影响正确性。

### ⚠️ 代理必须替浏览器带上 `Referer`

`c01.m-t.pics`（尨译的阿里云 OSS 桶）设了 **Referer 防盗链，且拒绝空 Referer**。浏览器加载图片时天然带 Referer，所以站点上一切正常；我们的代理不带，于是**该站所有图片都 403**：

```
<Code>AccessDenied</Code><Message>You are denied by bucket referer policy.</Message>
```

代理就是替浏览器发请求的，所以取图和下载都要带上 `Referer: <站点 origin>/`（见 `Profile::media_referer`）。只带 `api_base` 的 origin 不够——那可能是另一个主机。

> 这个故障还有个**会骗人的变体**：一旦某张图带着正确 Referer 成功过一次，OSS 边缘就可能把它缓存住，之后不带 Referer 也返回 200。于是现象看起来是「有的能显示有的不能」「刚才还好好的」，很容易被误判成并发或缓存问题。判断方法是换一个**从没成功过**的路径再测。

### ⚠️ 状态码不是「取到了图」的证明

各站的 nginx 都写着 `try_files $uri /index.html`，所以**任何不存在的路径都会回 200 + SPA 外壳**（`moetran.com/storage/xxx.png` 实测 `200 text/html`）。

取图的重试循环如果只看 `status().is_success()`，就会把这段 HTML 当成图片收下、缓存起来，还按 URL 后缀报成 `image/png` —— 一张**永久损坏**的图，并且**挡住后面本来正确的候选 origin**（轮不到它重试）。所以候选响应要验证 Content-Type（`looks_like_an_image`）：`text/html` 一律拒绝，其余放过（对象存储常对图片回 `application/octet-stream`，白名单会误杀）。

漏掉这条的症状就是「图片全部裂开，但缓存目录里躺着一堆 HTML 文件」。

### ⚠️ 下载暂存文件必须每个请求一份

一个工作台页面会**同时**发起一屏的取图请求。如果这些请求共用同一个暂存文件，后果不是「慢一点」而是**静默的数据损坏**：

- 两个下载器各自的文件游标都从 0 开始，字节互相覆盖 → 最终文件是两张图的**拼接物**
- 缓存把它当成合法内容存下来，之后**永远**返回这张错误的图
- 抢输的那个 `rename` 在 Windows 上会因为另一个句柄还开着而失败 → 请求直接 500

实测（修复前，14 个并发请求）：**2 个成功、12 个 500**，缓存里留下一个「JPEG 头 + PNG 尾」的缝合文件。修复后：14 个全 200，14 份缓存全部完整。

所以暂存路径用的是**进程内自增序号**（`.incoming-<pid>-<seq>`），不是时间戳——秒级时间戳在一屏请求里必然撞车。

索引文件带 `CACHE_VERSION`：一旦有版本会写出**键正确但内容错误**的条目，就把整个缓存丢弃重来。空缓存会在下次取图时自愈，而被污染的条目会一直错下去。

## 已知限制

- 邀请链接由前端用 `window.location` 拼出，在客户端里会得到 `127.0.0.1:端口/...`。复制给他人无效，需要在浏览器里从站点复制。
- 上游 `BatchTranslateModal` 直接向存储域名取图；跨域被拦时可通过媒体代理绕行（见 `rewrite.rs`）。
- 上游与 iroha 定制分支**不是**同一份前端。本客户端打包的是上游版本，因此不含邀请码注册、站点管理页、公告/日志、深色模式等定制功能。
