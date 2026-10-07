# MoeFlow Android 套壳客户端

把 MoeFlow 站点做成 Android 应用：**WebView 直接加载用户选定的站点 URL**。

这是 [`moeflow-desktop`](../README.md) 的移动端对应版本，但**不是**桌面版的移植 —— 它刻意少了一大半东西，理由见下。

## 为什么"直接加载站点"在移动端成立，在桌面端却不成立

桌面端之所以要内置一个 axum 环路服务器（`http://127.0.0.1:<port>/`），是因为它加载的是**打包进安装包的前端产物**：根绝对路径的资产、`BrowserRouter` 的 SPA fallback、`token` cookie 的同源要求，都只有在"像 nginx 一样服务"时才满足。

Android 版加载的是**真实站点的真实 origin**，上面每一条都不再是问题：

| 桌面端的麻烦 | Android 版为什么没有 |
|---|---|
| 前端资产是根绝对路径（`/assets/...`），`file://` 下白屏 | 从站点 origin 加载，路径天然成立 |
| `BrowserRouter` 无 basename，需要 SPA fallback | 站点自己的 nginx 负责 |
| `token` cookie + `Authorization` 头需要同源 | 就在真实 origin 上，不需要任何改写 |
| `/moeflow-runtime-config.json` 需要客户端伪造 | 站点自己提供（或前端回退到 `/api/`） |

而且上游前端本来就是按移动端写的 —— `index.html` 的 viewport 是
`width=device-width, initial-scale=1, minimum-scale=1, maximum-scale=1, user-scalable=no, shrink-to-fit=no, viewport-fit=cover`。
所以这是**架构上正确**的选择，不是省事。

代价是少了一组桌面端专有的能力，而这些在"套壳浏览器"的定位下本来就不该有：

| 能力 | 桌面端 | Android 版 |
|---|---|---|
| 本地反向代理 | 有（环路服务器） | 无（直连站点） |
| 图片磁盘缓存 / LRU | 有 | 无（交给 WebView 的 HTTP 缓存） |
| 本地导出（LabelPlus txt / 成品 zip） | 有 | **无 —— 用服务器导出**，成品走系统下载 |
| 代理设置（直连/系统/手动） | 有 | 无（走系统网络配置） |
| 多实例 + 设置 | 有 | 有（native 实现，预置表与桌面端共用一份定义） |
| 托盘 / 多窗口 | 有 | 无（移动端没有这些语义） |

**唯一的功能性缺口是本地导出**，这是需求明确砍掉的：导出改走服务器队列，成品 zip 由
`DownloadListener` 交给系统 `DownloadManager`（带 Cookie 与 Referer）。

## 目录

```
android/
  settings.gradle.kts / build.gradle.kts / gradle.properties
  gradle/wrapper/            Gradle 8.11.1 wrapper（jar 已随仓库提供）
  app/
    build.gradle.kts
    src/main/AndroidManifest.xml
    src/main/res/            布局 / 文案 / 主题 / 功能图标 / 启动图标（位图）
    src/main/java/org/basmc/moeflow/
      MoeFlowApp.kt          Application：启动时播种实例
      data/                  实例档案、预设表、URL 工具、连接探测
      ui/                    实例选择页、设置页、列表与编辑弹窗
      web/                   WebView 宿主、导航策略、下载、文件选择、blob 桥
```

完整的接口契约（类签名、资源 id、字符串键、功能图标 pathData、每条行为规则）在
[`../docs/android-port-contract.md`](../docs/android-port-contract.md)。**改这个工程之前先读它**，
里面的规则基本都对应一次真实故障。

## 构建

### 前置

| 需要 | 说明 |
|---|---|
| JDK 17+ | 本仓库在 JDK 21 上配置（`compileOptions` / `jvmTarget` 均为 17） |
| Android SDK | 需 `platforms;android-35`、`build-tools;35.0.0` |
| Android Studio | 非必需，但用它可以省掉手写 `local.properties` 的步骤 |

```bash
cd android
echo "sdk.dir=/path/to/Android/Sdk" > local.properties     # 或用 ANDROID_HOME
./gradlew :app:assembleDebug                               # Windows 上用 gradlew.bat
./gradlew :app:lintDebug
```

产物：`app/build/outputs/apk/debug/app-debug.apk`（约 5.8 MB）

安装：

```bash
adb install -r app/build/outputs/apk/debug/app-debug.apk
```

Release 版：

```bash
./gradlew :app:assembleRelease
```

产物是 `app/build/outputs/apk/release/app-release.apk`。

**它必须被签名，否则装不上。** 未签名的 APK，Android 的安装器一律拒绝，报的是
「安装包无效 / 解析失败」—— 看起来像包坏了，其实只是没有签名。凭据放在
`android/keystore.properties`（已 gitignore），模板见 `android/keystore.properties.example`：

```properties
storeFile=moeflow-release.jks
storePassword=...
keyAlias=moeflow
keyPassword=...
```

`storeFile` 相对 `android/` 解析。生成一把新密钥：

```bash
keytool -genkeypair -v -keystore moeflow-release.jks -alias moeflow \
  -keyalg RSA -keysize 2048 -validity 10000 \
  -storepass <密码> -keypass <密码> -dname "CN=..., OU=..., O=..., C=CN"
```

⚠️ **这把密钥决定应用的「升级身份」。** 包名 + 签名一起唯一标识一个应用：换了密钥再签名，
系统会拒绝覆盖安装，用户必须先卸载 —— 而卸载会清掉 WebView 里的登录态。所以 jks 和
properties 都要备份好，不要丢、不要随手重生成。

凭据缺失时构建会**退回 debug 签名**（并在构建日志里打出警告）：包能装，但不适合分发，
而且将来换成正式密钥签名后同样覆盖安装不上。留这条退路，是为了不让 `assembleRelease`
默默产出一个「装不上」的包。

验签：

```bash
$ANDROID_HOME/build-tools/35.0.0/apksigner verify --verbose app/build/outputs/apk/release/app-release.apk
```

`local.properties` 已加进 `.gitignore`，不要提交。

### 需要的 SDK 组件

`platforms;android-35`、`build-tools;35.0.0`（或让 AGP 自己选，它会按需下载它默认的 build-tools）。
`platform-tools` 只在装到设备时需要。

> **注意**：新版 `cmdline-tools` 里的 `sdkmanager.bat` 已经转发给新的 `android` CLI，而那个 CLI 在
> 某些环境下会因为 `.sdk/lock` 拿不到写权限直接崩（`AccessDeniedException` at `ProcessFileLock.withLock`）。
> 遇到这种情况，直接从 `https://dl.google.com/android/repository/platform-35_r02.zip` 下下来解压到
> `$ANDROID_HOME/platforms/` 即可 —— 压缩包的顶层目录就是 `android-35`，解压完就是装好的状态，
> 用 `source.properties` 里的 `AndroidVersion.ApiLevel=35` 可以核对。

### 关于 `gradle/wrapper/gradle-wrapper.jar`

它已随仓库提供（从官方 Gradle 8.11.1 发行包的 `gradle-wrapper-main-8.11.1.jar` 内嵌资源中提取，
33 个条目、依赖自检零缺口）。如果它丢失或损坏，用任意 Gradle 重建：

```bash
gradle wrapper --gradle-version 8.11.1
```

或者直接用 Android Studio 打开 `android/` 目录，它会提示修复。

> 顺带一条经验：`raw.githubusercontent.com` 上那份 `gradle-wrapper.jar` 在部分网络环境下会被重置连接，
> 走发行包内嵌资源比走 raw 链接可靠。另外 `lib/gradle-wrapper-shared-*.jar` 是**未打平**的版本，
> 把两个 jar 合起来当 wrapper jar 用会缺 `org/gradle/cli/*`、`org/gradle/internal/file/*` 这些被 shade
> 进去的类，不能这么凑。

### 网络受限时怎么让 `./gradlew` 跑起来

`distributionUrl` 指向 `services.gradle.org`，而它会 307 到 GitHub Releases —— 国内网络下这个下载
很容易卡死（症状是 `Downloading https://services.gradle.org/distributions/gradle-8.11.1-bin.zip failed:
timeout (10000ms)`）。两条办法，任选：

**其一：换镜像。** 把 `gradle/wrapper/gradle-wrapper.properties` 里的 `distributionUrl` 换成

```
https://mirrors.cloud.tencent.com/gradle/gradle-8.11.1-bin.zip
```

**其二：手动预置分发包**（不改配置，适合只想临时救急）。wrapper 的本地缓存布局是固定的，
目录名是 `distributionUrl` 的 md5 转 36 进制：

```
~/.gradle/wrapper/dists/gradle-8.11.1-bin/bpt9gzteqjrbo1mjrsomdt32c/
    gradle-8.11.1-bin.zip          ← 从镜像下回来的分发包
    gradle-8.11.1-bin.zip.ok       ← 空标记文件，wrapper 见到它就跳过下载
    gradle-8.11.1/                 ← 把上面的 zip 解开得到的目录
```

三步就位之后 `./gradlew` 会直接用这份，不再联网。注意 `.ok` 标记**必须**在，否则 wrapper 会重新下载。

`networkTimeout` 已经在本仓库里从模板默认的 10 秒放宽到 2 分钟 —— 130 MB 的分发包用 10 秒超时本来就不合理。

### 应用图标

启动图标用的是上游 MoeFlow 的 logo（`frontend/public/static/logo512.png`），与桌面端 `tauri icon`
同一份来源。但它**不能**直接拿来当自适应图标的前景层：

- 源 logo 是**不透明的白底方图** —— PNG 里连 alpha 通道都没有；
- 而自适应图标保证不被裁的只有画布中央 **66dp 圆**（多数启动器的遮罩约 72dp）。

直接套用，角色的下半身会被遮罩切掉。所以 `res/mipmap-<density>/ic_launcher_foreground.png`
是按下面三步生成的（画布固定 108dp）：

1. **去白底**：`alpha = 1 - min(r,g,b)/255`，再反求原始墨色 `fg = (c - 255*(1-a)) / a`。
   这份线稿去底后墨色恒为纯黑、深浅全在 alpha 上 —— 正好也是 `monochrome` 层要的形态。
2. **缩进安全区**：把图案包围盒内接进 **72dp 圆**，居中。三档留白（圆内接 66dp / 圆内接 72dp /
   方内接 66dp）出图对比后选的这一档：66 明显偏小，方内接在圆形遮罩下会碰到边界。
3. 导出五档：mdpi 108px / hdpi 162 / xhdpi 216 / xxhdpi 324 / xxxhdpi 432。

背景层是纯白（`@color/ic_launcher_background`），与 logo 自身的白底一致。

三种常见遮罩（圆形 / 圆角方 / 小圆角方）下的实际效果见
[`../docs/android-icon-preview.png`](../docs/android-icon-preview.png)。

> 仓库里 `src-tauri/icons/android/` 那份是 Tauri 生成的，它的 `ic_launcher_foreground.png`
> 是**整张不透明**的直角方图（实测不透明包围盒 = 整张画布 100%），不能直接拿来当自适应前景。
> 不要因为"仓库里已经有了"就省这一道。

## 它是怎么跑的

### 启动与返回栈

```
[实例选择页]  --点某一行（CLEAR_TOP|SINGLE_TOP）-->  [实例选择页, Web]
[Web]         --右下角「实例」按钮（REORDER_TO_FRONT）-->  [Web, 实例选择页]
[Web, 实例选择页] --选另一个实例（CLEAR_TOP 弹掉实例页）-->  [实例选择页, Web]
```

- 入口是**实例选择页**，不是直接进某个站点 —— 与桌面端一致，「直接落进某个服务器」是多实例客户端不该有的意外
- `本地导出` 那个浮动入口没有对应物（功能已砍），取而代之的是右下角一个半透明的**「实例」按钮**：
  单击回实例选择页，长按弹菜单（刷新 / 切换实例 / 在浏览器中打开 / 关于）
- 返回键先走 WebView 历史，退回不动时结束 Web 回到实例页
- 「启动时直接进入上次使用的实例」开关默认**关**

### 落点为什么是 `/dashboard/projects`

与桌面端同一条理由：前端把 `/` 放在 `publicPaths` 里，未登录也会渲染站点主页（含服务端配置的公告横幅）。
选受保护路由，守卫（`!token && !publicPaths.includes(path)`）会自己处理两种情况：未登录跳 `/login`，
已登录直接进工作台。过期令牌也会自愈 —— 首次 401 清 token，守卫随即重定向。

定义在 `Profile.entryUrl`，不要改成 `/`。

### 没有「API 地址」这个设置项

界面上**只有名称和站点地址两项**。这是刻意的：

桌面端有一个"API 地址"，因为它的环路服务器要把 `/api/` 下的请求反代到真正的后端 —— 那是
"加载打包产物"方案的必然产物。套壳版没有反代，WebView 直接在站点 origin 上发请求，
**API 地址不参与任何流量路由**，存下来就是一个永远用不到、还让人以为需要配的死字段。
所以这个版本把它连同界面一起去掉了（`Profile` 里只有 `id` / `name` / `siteUrl` / `allowInvalidCerts`）。

同理，编辑弹窗里原来有一排"预设服务器" Chip，也去掉了 —— 一个套壳客户端不需要一张别人的服务器
清单摆在编辑框上面。预置实例仍然只有尨译一条。

> `ServerPresets` 这张表**内部还在用**（它不再出现在界面上）：`ServerProbe` 对已知 host 直接判定
> 可用、省一次网络探测；`MoeFlowWebViewClient` 用它推出 API 主机，好让指向 `api.moetran.com`
> 的顶层导航留在应用内（服务器导出的成品 zip 链接可能指过去，丢给系统浏览器用户就等不到下载了）。

### 「测试连接」在测什么

它只回答一个问题：**这个地址是不是一台可用的 MoeFlow 服务器。** 不产出任何要保存的地址。

**判据是 `GET <candidate>/ping` 返回字面量 `pong`，不是状态码。**

所有部署形态的 nginx 都写着 `try_files $uri /index.html`，任何未知路径都会回 200 + SPA 外壳
（实测 `moetran.com/storage/xxx.png` 返回 `200 text/html`）。用状态码判断会把 SPA fallback 当成
"服务器可用"。候选顺序 `<site>/api` → `<site>` → `https://api.<host>`，命中预设表则**不发任何网络请求**。
推导出来的候选只活在函数内部，既不保存也不出现在界面上。

探测失败**不阻断保存** —— 服务器临时离线、内网机器不在同一网段，都是"此刻探不到但地址没错"的情况。

### 必须做对的几件事

这几条每一条都对应一个会让应用"看起来能用其实不能用"的故障：

| 事项 | 做错会怎样 |
|---|---|
| 下载请求带 `Cookie` **和** `Referer` | 尨译的 OSS 桶（`c01.m-t.pics`）有防盗链且**拒绝空 Referer**，漏掉就是整站资源 403。`DownloadManager` 是独立进程，两样都得自己带 |
| `onPause` 里 `CookieManager.flush()` | 进程被回收时会话 cookie 直接丢，用户下次打开发现被登出 |
| 文件选择同时接住单选（`data.data`）与多选（`data.clipData`） | 前端有作品文件上传（`FileList.tsx`）。漏掉多选那条路，选多张图就是"选完没反应" |
| `onReceivedHttpError` 排除 401/403 | 那是前端自己的鉴权流程，弹错误页会把登录界面盖掉 |
| 实例切换只更新 client/bridge 持有的 profile，不重建 | 重建意味着重复 `addJavascriptInterface`，同名接口会被 WebView 忽略并打警告 |
| `blob` 下载的 host 护栏读 `WebActivity.currentUrl` | 那是 WebView 线程读不到 UI 线程状态的问题：用 `runOnUiThread` 去"取"值永远是 null，护栏就成了"永远拒绝" |
| 根布局 `fitsSystemWindows="true"` | targetSdk 35 在 Android 15 上强制边到边，不加就是内容被状态栏盖住 |
| `buildFeatures { buildConfig = true }` | AGP 8.x 起 `buildConfig` 默认关闭，而代码用 `BuildConfig.VERSION_NAME`，不开就是 `Unresolved reference` |

### 为什么不做实例间的登录隔离

Cookie 天然按 host 隔离，而套壳版每个实例是**各自的真实域名**，所以多实例共用一个 cookie jar
不会串号。（桌面端需要按端口隔离，恰恰是因为它把所有实例都反代到了同一个 `127.0.0.1`。）

**不要**"修"成切换实例时清 cookie —— 那会把用户直接登出。唯一会共享登录态的情况是
两个实例用同一个域名，这是浏览器语义，不是 bug。

## 已知限制

1. **摄像头 / 麦克风未启用**：`onPermissionRequest` 一律拒绝并提示
2. **blob / data 下载超过 8 MB 会失败**：这条路径要把文件当字符串跨 JNI 传一次，超大字符串会 OOM
3. **Android 9 及以下下载落在应用专属目录**：`MediaStore.Downloads` 是 API 29 才有的类；低版本写公共目录又要 `WRITE_EXTERNAL_STORAGE` 运行时授权，为一条兜底路径不划算
4. **明文 HTTP 全量放行**（`cleartextTrafficPermitted="true"`）：内网部署形态确实有 `http://172.29.133.24`
5. **不含本地导出 / 图片磁盘缓存 / 代理设置**：相对桌面端的功能缺口，需求已确认
6. **未签名 APK 需开发者模式侧载**
7. **实例地址只保留 `scheme://host[:port]`**：部署在子路径下的站点（`https://host/moeflow`）不支持，与桌面端相同假设
8. **targetSdk 是 35 而不是 36**：本工程按 targetSdk 35 定义行为（边到边、分区存储等）。升到 36 需要同时抬 `compileSdk` 与 AGP 版本，会引入一轮新的行为变更，没有明确需求就不动

## 验证状态

### 已经跑通的

| 项 | 结果 |
|---|---|
| `./gradlew :app:assembleDebug` | **成功**，产出 `app-debug.apk`（约 5.81 MB） |
| `./gradlew :app:assembleRelease`（含 R8/minify） | **成功**，产出 `app-release.apk`（约 2.16 MB，**已签名**） |
| `./gradlew :app:lintDebug` | **0 error / 10 warning** |
| APK 清单核对（`aapt2 dump badging`） | 包名 `org.basmc.moeflow`、versionName 0.1.0、minSdk 26、targetSdk 35、权限只有 INTERNET + ACCESS_NETWORK_STATE、入口 `org.basmc.moeflow.ui.LauncherActivity`、自适应图标就位（`res/mipmap-anydpi-v21/ic_launcher.xml` + 五档前景位图） |
| `apksigner verify --print-certs` | release `Verifies`（v2 scheme），签名证书 SHA-256 与 `moeflow-release.jks` 一致；debug 同样 `Verifies` |
| 签名退路（移走 `keystore.properties` 后配置） | 按预期打出警告并退回 debug 签名，构建不失败 |

剩下 10 条 lint 警告都是**刻意保留**的取舍，不是待办：明文放行（内网部署形态确实用 http）、
targetSdk 未取最新、`allowBackup=false` 已覆盖 `dataExtractionRules`、小列表用 `notifyDataSetChanged`、
以及三处根布局 background 的 overdraw（那是有意画的底色）。

编译过程中真正修掉的代码问题只有一个，但很典型：
`DownloadManager.Request.setUserAgent()` 是 **API 36** 才加的方法，`compileSdk = 35` 下直接调它是
`Unresolved reference`。现在改成从 header 表加 `User-Agent`（API < 36 上 DownloadManager 自己不会补
这个头，所以不会重复）。

**还有一个是装到真机上才暴露的**：release 包一直没配签名，AGP 产出的是 `app-release-unsigned.apk`，
而 Android 一律拒绝安装未签名的包 —— 报「安装包无效 / 解析失败」，现象很容易被当成包损坏或设备不兼容。
对照很明确：**debug 包能装、release 包装不上，那就不是兼容性问题，是签名问题**。
现在 release 走 `moeflow-release.jks` 签名，`apksigner verify` 通过。

> 另一条环境经验：改**资源目录结构**（例如把 `mipmap-anydpi-v26/` 挪成 `mipmap-anydpi/`）之后，
> AGP 的增量资源合并会把旧目录留在 `packaged_res` 里，随后报 `resource mipmap/ic_launcher not found`。
> 这时 `clean` 一次再构建即可 —— 不是配置写错了。

> 还有一条**语言层面的**坑，代价很大所以特别记一下：**Kotlin 的块注释是可以嵌套的**。
> 在 KDoc 里写一个"斜杠 + 星号"开头的路径通配写法（比如描述某个前缀下的所有请求），那个斜杠星号
> 会被当成嵌套注释的开始，于是文档注释真正的收尾符只关掉了内层 —— 整个文件报 `Unclosed comment`，
> 然后**引出上百条级联错误**（本次实测 136 条，全在其它文件里）。看到那种"一大片 Unresolved reference"
> 时，先去看**第一条**错误和 `Unclosed comment` 之类的词法错误，不要从表象开始查。

### 还需要在真机/模拟器上验的

编译通过只说明它**能构建**，不代表这些行为是对的。按优先级：

1. 首启动 → 实例选择页出现，预置「尨译 MoeTran」一条
2. 点它 → 落到 `/dashboard/projects`，未登录自动跳 `/login`，验证码图片能显示
3. 登录 → 杀进程 → 重开，确认仍是登录态（验 `CookieManager.flush()`）
4. 上传一张作品文件（验文件选择器的**单选与多选**两条路）
5. 走一次服务器导出，确认成品 zip 落到系统「下载」（验 Cookie + **Referer** 是否带对）
6. 点一个站外链接，确认交给系统浏览器；点一个 blob 下载，确认走的是应用内保存
7. 切换到另一个实例再切回来，确认没被登出

第 5、6 两条对应本工程最容易"看着能用其实不能用"的两处，值得优先手动过一遍。
