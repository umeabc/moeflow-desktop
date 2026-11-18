# MoeFlow 桌面客户端

把 [moeflow-com/moeflow](https://github.com/moeflow-com/moeflow) 的前端打包成原生桌面应用（Tauri v2 + 系统 WebView）：内置本地反向代理、图片磁盘缓存，以及**在本地完成**的 LabelPlus txt / 成品 zip 导出。

同一份安装包内置两套前端——上游版与 iroha 定制分支（irohamod）——按实例自动切换，所以既能连上游站，也能连 iroha 定制站。支持 Windows / macOS / Linux，Windows 是主要验证目标。

> Android 版在 [`android/`](android/README.md)，是另一套实现（Kotlin + 原生 WebView），没有本地反代与本地导出。

## 使用说明

### 第一次启动

启动后先是**实例选择页**：预置实例列表 + 「添加实例」（填站点地址，客户端自己探测出真实 API 地址——判据是后端 `/ping` 返回 `pong`，不是 HTTP 状态码）。勾选「启动时直接进入上次使用的实例」可跳过这一步。登录在站点自己的登录页完成，客户端不保存密码，只沿用登录 cookie。

### 主界面

左边缘的竖排导航栏是客户端功能，不属于网站：

| 入口 | 作用 |
| --- | --- |
| 实例 | 切换、添加服务器实例 |
| 导出 | 在本地生成 LabelPlus txt 与成品 zip |
| 翻译 | 用本机智能体给当前图片页做标号 / 翻译 |
| 设置 | 服务器、代理、缓存、本地后端配置 |

### 本地翻译

「翻译」只在图片翻译页可用，按页面状态分两条路径：

- **已有标号** → 用「翻译后端」里选的那个（codex / dsh / Antigravity ACP）逐条翻译，写回已有标号；
- **没有标号** → 只能由 codex 看图建标号并翻译（dsh 没有视觉）。

作者标号始终不参与。点「开始」前建议先「预览」，它只出规范化后的 JSON，不写库。

三个后端都要先在设置里「一键锁定」找到可执行文件：codex 桌面版不会把 CLI 放进 PATH，一键锁定会扫常见安装位置；dsh 走 npm 包里的 `bin.js`；Antigravity 走 `agy acp` 的 ACP 协议，图片只对被实测过的模型家族放行（gemini-4 / gemini-3.8 / orchestrator / claude-5.5），其余模型自动降级为纯文本翻译。

### 本地导出

「导出」在客户端生成成品 zip 与 LabelPlus txt，不占服务器导出队列；已缓存的图片零网络开销。

### 托盘与关闭

- 关闭主窗口 = 隐藏到托盘，不退出进程。
- 关闭仪表台（实例选择 / 设置）只隐藏它自己，**不会**把已经关掉的实例窗口再弹出来。
- 托盘菜单可以重新打开主窗口或仪表台。

### 常见问题

| 现象 | 处理 |
| --- | --- |
| 拖拽图片到页面上没反应 | 0.1.9 起已修复（Tauri 会替换 WebView2 的拖放处理器），升级即可 |
| 「Codex 不可用：program not found」 | 设置 → 本地 Codex → 一键锁定 |
| 「agy 检测超时」 | 设置里的路径优先于自动探测；握手分 `initialize` / `session/new` 两步、各 30 秒，超时信息里带 stderr 尾部 |
| 某个站点的图片出不来 | 用 `MOEFLOW_MEDIA_DEBUG=1` 启动看被改写的存储 URL，细节见 [details.md](details.md) |

## 构建

### 两套前端

安装包里有两份前端产物，构建时都要生成：

```sh
bash scripts/fetch-frontend.sh          # 上游前端，固定 commit
bash scripts/build-frontend.sh          # 默认两套都建
# 或者只建一套：bash scripts/build-frontend.sh --variant irohamod
```

`scripts/fetch-frontend-irohamod.sh` 默认从 `umeabc/moeflow-irohamod` 稀疏克隆固定提交（当前 fix12）；本地开发可以用 `IROHAMOD_FRONTEND=/path/to/checkout` 直接指向自己的工作副本。CI 走的是前者。

### 打包目标按平台拆开

`src-tauri/tauri.conf.json` 只保留全平台共用的部分；**打包目标写在平台专属配置里**——Tauri v2 会按当前平台把它合并进来（对象逐键合并，数组整体替换）：

| 文件 | `bundle.targets` | 备注 |
|---|---|---|
| `tauri.windows.conf.json` | `nsis` | `webviewInstallMode: embedBootstrapper` 也在这里 |
| `tauri.macos.conf.json` | `app`、`dmg` | `minimumSystemVersion: 10.15` |
| `tauri.linux.conf.json` | `deb`、`rpm`、`appimage` | |

基础配置写 `"all"` 而不是某个平台的目标名：万一平台文件缺失，该平台会退回构建**自己的**默认目标，而不是拿另一个平台的目标去报「不支持」。

### 各平台前置

| 平台 | 需要 |
|---|---|
| Windows | Node 18+、Rust stable（MSVC 工具链）、VS Build Tools 的 C++ 工作负载、WebView2 运行时（Win11 自带） |
| macOS | Node 18+、Rust stable、Xcode（或 `xcode-select --install`）。**只能从 macOS 本机构建** |
| Linux | Node 18+、Rust stable、`libwebkit2gtk-4.1-dev`、`libayatana-appindicator3-dev`、`librsvg2-dev`、`libxdo-dev`、`patchelf`；构建 rpm 还需 `rpm` |

```bash
bash scripts/fetch-frontend.sh    # 拉取并锁定上游前端（含基线 git 仓库）—— 平台无关
npm install
npm run build:frontend            # npm ci → build:locale → vite build，并校验产物
npm run icons                     # 从 logo512.png 生成 .ico / .icns / png 全套（图标已提交，通常不必跑）
npm run test:rust                 # 纯逻辑单元测试（浮点格式化、LabelPlus、排序、缓存…）
npm run build                     # tauri build → 当前平台的安装包
```

产出一律落在 `src-tauri/target/release/bundle/<目标>/`：

| 平台 | 产物 |
|---|---|
| Windows | `nsis/MoeFlow_<版本>_x64-setup.exe`（约 8.5 MB） |
| macOS | `dmg/MoeFlow_<版本>_<arch>.dmg`，以及 `macos/MoeFlow.app` |
| Linux | `deb/*.deb`、`rpm/*.rpm`、`appimage/*.AppImage` |

前端构建与图标生成都是纯 Node，平台无关；`fetch-frontend.sh` / `build-frontend.sh` 是 bash，Windows 上走 Git Bash。

只想要其中一个目标时用 `--bundles`，比改配置干净：

```bash
npx tauri build --bundles appimage                 # 只要 AppImage
npx tauri build --target universal-apple-darwin    # Intel + Apple Silicon 通用二进制
```

### macOS

- **本机只能从 macOS 构建**（需要 Xcode）。CI 里的 `macos-latest` 就是为了绕开这一点——见 `.github/workflows/build.yml`。
- 分发需要**代码签名 + 公证**，否则 Gatekeeper 会直接拦下。CI 默认只出**未签名**包，够本机/内测验证；要正式分发，把 Apple 证书与 `notarytool` 凭据放进仓库 Secrets（`APPLE_CERTIFICATE`、`APPLE_CERTIFICATE_PASSWORD`、`APPLE_SIGNING_IDENTITY`、`APPLE_ID`、`APPLE_PASSWORD`、`APPLE_TEAM_ID`），再接到 workflow 的 `env` 上。
- 未签名的 `.app` 本机自用可以右键「打开」绕过一次 Gatekeeper。
- 通用二进制要两个 Rust target 都在：`rustup target add aarch64-apple-darwin x86_64-apple-darwin`。

### Linux

- 运行依赖 `libwebkit2gtk-4.1`（WebView）。**AppImage 不打包它**——目标机器仍需装有这个库。这是 Tauri 在 Linux 上的固有约束，不是本项目能配置掉的。
- `.deb` 的依赖由 Tauri 在构建时探测生成，一般不必手写。
- **托盘需要 AppIndicator**：`libayatana-appindicator3` 外加一个真正实现了 StatusNotifierItem 的桌面环境。默认 GNOME 两者都不提供，此时托盘创建会失败——应用**跳过托盘继续启动**（`main.rs` 里 `build_tray` 的失败现在只记日志，不再用 `?` 上抛）。此处曾经是 `build_tray(&handle)?`，症状是「装完一打开就闪退，连窗口都没有」。

### Windows 专有事项

MSVC 环境：若 `link.exe` 找不到库，用 `scripts/msvc-env.sh` 设置 `LIB`/`INCLUDE`（本机把工具链装在 `C:\BuildTools`，非默认路径，`vcvars64.bat` 也慢）。

### WebView2 引导程序下载失败

`webviewInstallMode` 是 `embedBootstrapper`：**构建时**要把微软的 1.8 MB 引导程序下载回来嵌进安装包（`fail` 会看到 `Downloading https://go.microsoft.com/fwlink/p/?LinkId=2124703` 然后连接被重置）。本机走 `127.0.0.1:1445` 代理时这个地址必挂，但这台机器**直连**是通的，所以把域名排除出代理即可：

```bash
NO_PROXY="go.microsoft.com,microsoft.com" npm run build
```

> 换成 `downloadBootstrapper` 可以绕开构建期下载，但那等于把「装不装得上」押在**目标机器**装的时候能连上微软——干净环境验收恰恰不该有这个前提。嵌进去之后，只有目标机器真的缺 WebView2 运行时（Win11 一律自带）才会去联网。

### 安装包内容与「干净环境」验收

NSIS 包解开后是这样（`7z l` 或 `7z x` 可直接看）：

```
moeflow-desktop.exe            7.5 MB
web\...                       13 MB   上游前端产物（bundle.resources）
$TEMP\MicrosoftEdgeWebview2Setup.exe  1.8 MB  WebView2 引导程序
```

`web\` 是随包资源，运行时由 `web_root()` 通过 `app.path().resource_dir()` 定位；**找不到时才会退回到编译期写死的仓库路径**——那个路径在别人机器上不存在，所以这个回退只在开发机上有效。装完后要验的就是它：把仓库里的 `frontend/build` 临时改名，再从别处跑安装出来的 exe，若仍能打开登录页，说明资源定位是对的。

## 更多

架构（本地反代、端口分配、外壳窗口、启动流程）、本地导出与空白原文的内部实现、IPC/窗口约定、缓存策略、测试环境的坑与已知限制，都在 [details.md](details.md)。
