# Android 套壳版 —— 实现契约

这是 `moeflow-desktop` 的 Android 对应版本。**本文是并行开发的唯一接口契约**，所有子任务都必须严格按此实现，不得自行改动类名、方法签名、资源 id 或字符串 key。

## 1. 目标与范围

把 MoeFlow 前端做成 Android 客户端。

**做法：纯套壳浏览器。** WebView 直接加载用户选定的站点 URL，不做本地环路服务器、不做反代、不做图片磁盘缓存、不做本地导出。

为什么这样可行（与桌面端的关键差异，不要试图"修正"这一点）：

| 桌面端的问题 | Android 套壳为什么没有这个问题 |
|---|---|
| 前端资产是根绝对路径（`/assets/...`），`file://` 下白屏 | 直接从站点 origin 加载，根绝对路径天然成立 |
| `BrowserRouter` 无 basename，需要 SPA fallback | 站点自己的 nginx 负责 SPA fallback |
| `token` cookie + `Authorization` 头需要同源 | 就在真实站点 origin 上，不需要任何改写 |
| `/moeflow-runtime-config.json` 需要客户端伪造 | 站点自己提供（或前端回退到 `/api/`），与本应用无关 |

上游 `index.html` 的 viewport 是 `width=device-width, initial-scale=1, minimum-scale=1, maximum-scale=1, user-scalable=no, shrink-to-fit=no, viewport-fit=cover` —— 即这份前端本来就按移动端设计。所以套壳是**架构上正确**的选择，不是妥协。

### 明确不做（非目标）

- 本地环路 HTTP 服务器 / 反向代理 / 媒体 URL 改写
- 图片磁盘缓存与 LRU 淘汰
- LabelPlus txt / 成品 zip 的本地导出（**需求已明确移除**）
- 代理设置（直连/系统/手动）—— 走系统网络配置
- 托盘、多窗口、窗口状态记忆
- 深色模式（与桌面端一致，上游不含该功能）
- iOS

### 与桌面端一致、必须保留的不变量

这些是从桌面端踩坑史里来的，**不要在 Android 版重新踩一遍**：

1. **落点不是 `/`**，而是 `/dashboard/projects`。`/` 在 `publicPaths` 里，未登录也会渲染站点主页（含公告横幅）；受保护路由才能让守卫自动重定向到 `/login` 或直接进工作台。常量 `MAIN_ENTRY_PATH = "/dashboard/projects"`。
2. **至少保留一个实例**：只剩一个时禁止删除，并明确报错，而不是留下一个连不上任何地方的窗口。
3. **实例选择页永远可达**：当前实例连不上时它照样能打开。
4. **站点地址要能被校验**：探测判据是 `GET <candidate>/ping` 返回字面量 `pong`（**不能看状态码**：`try_files $uri /index.html` 会让任何未知路径回 200）。详见 §4 的 `ServerProbe`。

   > 套壳版**没有**"API 地址"这个设置项，界面上也没有。桌面端有，是因为它的环路服务器要把 `/api/*` 反代到真正的后端；套壳版直接加载站点 origin，API 地址**不参与任何流量路由**，存下来只会让人以为需要配。探测只为回答一个问题：这个地址是不是一台可用的 MoeFlow 服务器。
5. **不要静默失败**：任何可能失败的路径（连不上、证书错、下载失败）都要让用户看到原因。
6. **预置只有一个实例：尨译 MoeTran**。一个默认就列着别人服务器的客户端只会让人连错地方。

## 2. 技术栈（已定，不要更换）

- Kotlin + **原生 WebView**（AndroidX），不使用 Tauri/Flutter/Capacitor
- 版本：Gradle **8.11.1**、AGP **8.7.3**、Kotlin **2.0.21**、compileSdk/targetSdk **35**、minSdk **26**、Java 17 兼容级别
- minSdk 26 的理由：自适应图标（adaptive icon）从 API 26 起才有，锁定 26 之后就不必再为 API<26 准备一套传统位图 mipmap；2026 年 Android 8.0 覆盖率已可忽略损失
- 依赖只允许这几个（**不要新增依赖**）：
  - `androidx.core:core-ktx:1.13.1`
  - `androidx.appcompat:appcompat:1.7.0`
  - `androidx.activity:activity-ktx:1.9.3`
  - `androidx.lifecycle:lifecycle-runtime-ktx:2.8.7`
  - `androidx.recyclerview:recyclerview:1.3.2`
  - `com.google.android.material:material:1.12.0`
  - `org.jetbrains.kotlinx:kotlinx-coroutines-android:1.8.1`
  - **不用 constraintlayout**：布局只用 `LinearLayout` / `FrameLayout` / `MaterialCardView` / `RecyclerView`
  - **不用 Gson/Moshi**：JSON 用 `org.json`（Android 内置）
  - **不用 androidx.webkit**
- 包名 / applicationId：`org.basmc.moeflow`，app 名称 `MoeFlow`，versionName `0.1.0`，versionCode `1`
- UI 文案：**简体中文**，全部走 `@string/`，默认（无语言限定符）就是中文。不提供 `values-en`
- 启用 `viewBinding = true`

## 3. 工程结构

```
android/
  settings.gradle.kts
  build.gradle.kts
  gradle.properties
  gradlew  gradlew.bat
  gradle/wrapper/gradle-wrapper.properties
  gradle/wrapper/gradle-wrapper.jar        （见 §9 关于此文件）
  .gitignore
  README.md                                （整合阶段由主流程写，子任务不要写）
  app/
    build.gradle.kts
    proguard-rules.pro
    src/main/AndroidManifest.xml
    src/main/res/
      values/{strings,colors,themes}.xml
      xml/network_security_config.xml
      drawable/{ic_add,ic_edit,ic_delete,ic_refresh,ic_open_in_new,ic_settings,
                ic_instance,ic_info,ic_arrow_back}.xml
      mipmap-anydpi/ic_launcher.xml
      mipmap-{mdpi,hdpi,xhdpi,xxhdpi,xxxhdpi}/ic_launcher_foreground.png
      layout/{activity_launcher,activity_settings,item_profile,dialog_profile_editor,activity_web}.xml
      menu/menu_web_fab.xml
    src/main/java/org/basmc/moeflow/
      MoeFlowApp.kt
      data/{Profile,ProfileStore,ServerPresets,ServerProbe,UrlTools}.kt
      ui/{LauncherActivity,SettingsActivity,ProfileListAdapter,ProfileEditorDialog}.kt
      web/{WebActivity,MoeFlowWebViewClient,MoeFlowWebChromeClient,DownloadSupport,BlobBridge}.kt
```

### 文件归属（避免并行写冲突，**严禁越界写别人的文件**）

| 子任务 | 独占文件 |
|---|---|
| A 骨架/构建/资源 | `android/*`（除 `README.md`）、`app/build.gradle.kts`、`app/proguard-rules.pro`、`AndroidManifest.xml`、`res/values/**`、`res/xml/**`、`res/drawable/**`、`res/mipmap-anydpi/**`（图标位图由主流程从上游 logo 生成，不归子任务） |
| B 数据层 | `java/org/basmc/moeflow/MoeFlowApp.kt`、`java/org/basmc/moeflow/data/**` |
| C WebView 宿主 | `java/org/basmc/moeflow/web/**`、`res/layout/activity_web.xml`、`res/menu/menu_web_fab.xml` |
| D 实例页/设置页 | `java/org/basmc/moeflow/ui/**`、`res/layout/{activity_launcher,activity_settings,item_profile,dialog_profile_editor}.xml` |

**A 负责所有 drawable**（含 C/D 用到的图标），B/C/D 只按 §7 的名字引用，不要自己创建 drawable。

## 4. 数据层契约（B 实现，C/D 只调用）

### `data/Profile.kt`

```kotlin
package org.basmc.moeflow.data

data class Profile(
    val id: String,
    val name: String,
    /** 站点地址，形如 https://moetran.com，结尾不带 / */
    val siteUrl: String,
    /** 允许无效 TLS 证书（内网自签名站点） */
    val allowInvalidCerts: Boolean = false,
) {
    /** WebView 的落点，见 §1 不变量 1 */
    val entryUrl: String get() = siteUrl + "/dashboard/projects"
    val host: String? get() = UrlTools.hostOf(siteUrl)
}
```

**只有一个地址概念：`siteUrl`。** 早期版本还带 `apiBase` / `presetName`，已连同 UI 一起去掉 ——
套壳版没有反代，API 地址没有去处（详见 §1 不变量 4）。

### `data/UrlTools.kt`

```kotlin
package org.basmc.moeflow.data

object UrlTools {
    /** 补全 scheme（缺省 https://）、去掉结尾 / 与多余空白；失败返回 null。 */
    fun normalizeSiteUrl(input: String): String?

    /** 取 host（小写），失败返回 null。 */
    fun hostOf(url: String): String?

    /**
     * 从 content-disposition 或 URL 末段猜一个安全的文件名；都失败时返回 fallback。
     * 必须剥离路径分隔符与控制字符，并限制长度 <= 120。
     */
    fun guessFileName(url: String, contentDisposition: String?, fallback: String): String
}
```

`normalizeSiteUrl` 规则：
- `trim()`；空 → null
- 无 `://` 前缀 → 前面补 `https://`
- 只接受 `http` / `https` scheme，其它 → null
- 解析出的 host 为空 → null
- 去掉 path/query/fragment，只保留 `scheme://host[:port]`，并去掉结尾 `/`
- 例：`" moetran.com/ "` → `https://moetran.com`；`"http://172.29.133.24"` → `http://172.29.133.24`

### `data/ServerPresets.kt`

与 `src-tauri/presets/servers.json` **逐字对应**（注释里注明来源文件）：

```kotlin
package org.basmc.moeflow.data

data class ServerPreset(val host: String, val name: String, val apiBase: String)
// 注意：apiBase 不再是设置项，但内部还有两处要用 —— ServerProbe 命中预设时直接判定可用，
// 以及 MoeFlowWebViewClient 判断跨 host 导航要不要留在应用内（尨译的 API 在另一个源）

object ServerPresets {
    val ALL: List<ServerPreset> = listOf(
        ServerPreset("moeflow.basmc.org", "彩翻（basmc）", "https://moeflow.basmc.org/api"),
        ServerPreset("moetran.com", "尨译 MoeTran", "https://api.moetran.com"),
        ServerPreset("demo.moeflow.org", "MoeFlow 演示站", "https://demo.moeflow.org/api"),
    )

    /** 按 host 精确匹配（忽略大小写），命中即无需网络探测。 */
    fun match(host: String?): ServerPreset?

    /** 首次启动时预置的唯一实例：尨译 MoeTran。 */
    fun seedProfile(): Profile
}
```

`seedProfile()` 返回 `Profile(id = 固定常量 "preset-moetran", name = "尨译 MoeTran", siteUrl = "https://moetran.com")`。

> 这张表**不出现在界面上**：预置实例只有尨译一条，其余由用户自行添加。早期版本在编辑弹窗里放过
> 一排"预设服务器" Chip，已去掉 —— 一个套壳客户端不需要一张别人的服务器清单摆在编辑框上面。

### `data/ServerProbe.kt`

纯逻辑 + `HttpURLConnection`，全部在 `Dispatchers.IO` 上跑，**不引入新依赖**。

```kotlin
package org.basmc.moeflow.data

sealed interface ProbeOutcome {
    /** 站点可达，且 /ping 返回了字面量 pong。不带任何地址。 */
    object Ok : ProbeOutcome
    data class Failed(val reason: String) : ProbeOutcome
}

object ServerProbe {
    /** 按候选顺序探测。允许 5s 连接 / 5s 读取超时。 */
    suspend fun probe(siteUrl: String): ProbeOutcome
}
```

规则（**逐条照做，这是桌面端踩过的坑**）：

1. `normalizeSiteUrl` 失败 → `Failed`，`lastError = "站点地址无法解析"`。
2. host 命中预设 → 直接 `Ok`，**不发任何网络请求**（已知部署形态，省一次往返也少一个误报面）。
3. 否则按候选顺序探测：`<site>/api`、`<site>`、`https://api.<host>`。
4. 判据：`GET <candidate>/ping`，响应体 `trim()` 后**等于 `pong`** 才算通过。**不得**用 HTTP 状态码判断（SPA fallback 会把任何未知路径回成 200 + HTML）。
5. 全部失败 → `Failed(最后一次的失败原因)`（如 `HTTP 404` / `该地址不是 MoeFlow 服务器` / `连接超时` / `证书错误`）。
6. 候选去重；`ok` 即返回第一个通过的。
7. 任何异常（`IOException`、`SSLException` 等）都不能抛出到调用方，转成 `Failed`。

> 推导出的候选基址（`<site>/api`、`https://api.<host>` 等）**只活在函数内部**，既不保存也不出现在界面上。探测失败**不阻断保存**（见 §5 弹窗行为），只是给出提示 —— 服务器临时离线、内网机器不在同一网段，都是"此刻探不到但地址没错"的正常情况。

### `data/ProfileStore.kt`

```kotlin
package org.basmc.moeflow.data

/** 实例档案的持久化。用 SharedPreferences + org.json，键名见下。 */
class ProfileStore(context: Context) {

    fun profiles(): List<Profile>
    fun activeId(): String?
    fun activeProfile(): Profile?
    fun skipLauncher(): Boolean

    /** 不存在则追加，存在则按 id 覆盖。返回写入后的完整列表。 */
    fun upsert(profile: Profile): List<Profile>

    /**
     * 删除实例。**只剩一个时拒绝**（抛/返回失败），返回是否成功。
     * 删掉的是当前活动的实例时，activeId 回退到列表第一个。
     */
    fun delete(id: String): Boolean

    /** 设置当前实例；id 不存在则不改变并返回 false。 */
    fun setActive(id: String): Boolean

    fun setSkipLauncher(value: Boolean)

    /** 首次启动播种：列表为空时写入 ServerPresets.seedProfile() 并设为活动。返回是否播种过。 */
    fun seedIfEmpty(): Boolean
}
```

实现要求：
- SharedPreferences 名 `moeflow`；键：`profiles`（JSON 数组字符串）、`active_id`、`skip_launcher`
- 每个 profile 序列化为 JSON 对象，键名 **snake_case**：`id` / `name` / `site_url` / `allow_invalid_certs`。（早期版本写过 `api_base` / `preset_name`，读取时忽略即可，**旧数据不需要迁移** —— 下次编辑保存时自然消失。）
- 解析失败（脏数据）不能崩：跳过坏条目；整份 JSON 解析失败则返回空列表（由 `seedIfEmpty` 兜底）
- 线程安全：方法可被任意线程调用，内部用 `@Synchronized` 或锁
- 提供单例入口供 Activity 使用：`companion object { fun get(context: Context): ProfileStore }`，内部缓存实例

### `MoeFlowApp.kt`

```kotlin
package org.basmc.moeflow

class MoeFlowApp : Application() {
    override fun onCreate() {
        super.onCreate()
        // 只做一件事：播种。不要在这里做任何网络请求或 WebView 初始化。
        ProfileStore.get(this).seedIfEmpty()
    }
}
```

## 5. UI 契约（D 实现）

### 启动流程与返回栈（必须按此实现）

- `LauncherActivity` 是 MAIN/LAUNCHER 入口，`launchMode="singleTop"`
- `WebActivity` 是 `standard`/`singleTop`，从实例页启动时用 **`FLAG_ACTIVITY_CLEAR_TOP or FLAG_ACTIVITY_SINGLE_TOP`** + `EXTRA_PROFILE_ID`
- FAB → 实例页用 **`FLAG_ACTIVITY_REORDER_TO_FRONT`**
- 期望的栈行为：`[实例页, Web]` ←（FAB）→ `[Web, 实例页]` ←（选实例，CLEAR_TOP）→ `[实例页, Web]`
- `skipLauncher`（默认 **关**）打开且存在活动实例时：实例页 `onCreate` 里直接带着活动实例启动 `WebActivity` 并 `finish()` 自己
- `onNewIntent` 收到不同 profile id 时切换实例（见 §6）

### `LauncherActivity`

布局 `activity_launcher.xml`。自顶向下（`LinearLayout` 垂直，根节点 `android:fitsSystemWindows="true"`）：

1. `MaterialToolbar` id `launcherToolbar`，标题 `@string/launcher_title`，右侧菜单项 `action_settings`（图标 `@drawable/ic_settings`，`app:showAsAction="always"`）
2. `TextView` id `launcherSubtitle`，`@string/launcher_subtitle`，次要色，16dp 水平内边距
3. `RecyclerView` id `profileList`，`layout_weight=1`
4. `LinearLayout` id `emptyView`（`visibility="gone"`），内含 `TextView` id `emptyText` = `@string/empty_instances`
5. `SwitchMaterial` id `skipLauncherSwitch`，文案 `@string/skip_launcher`
6. `MaterialButton` id `addProfileButton`，文案 `@string/action_add_instance`，左侧图标 `@drawable/ic_add`，`style="?attr/materialButtonOutlinedStyle"`
7. `TextView` id `versionText`，文案运行时填 `@string/about_version`（`BuildConfig.VERSION_NAME`）

行为：
- 点 `item_profile` 的行（**不含**行内的编辑/删除图标）→ `setActive(id)` + 启动 `WebActivity`
- 行内 `editButton` → 打开编辑弹窗（`ProfileEditorDialog`）
- 行内 `deleteButton` → 见「删除」
- `addProfileButton` → 新增弹窗
- 菜单 `action_settings` → 启动 `SettingsActivity`
- `skipLauncherSwitch` 的初值取 `skipLauncher()`；用户拨动即写入
- 列表为空时显示 `emptyView`（正常不会发生，因为播种）
- 活动实例的行显示 `activeBadge`（`@string/badge_active`）
- **删除**：`profileList.size <= 1` 时 → `Toast(@string/last_instance_cannot_be_deleted)`，不做任何删除。否则弹 `MaterialAlertDialogBuilder`（标题 `@string/delete_confirm_title`，正文 `@string/delete_confirm_message` 带实例名），确认后 `ProfileStore.delete(id)`，刷新列表并 `Toast(@string/deleted)`
- 行点击（连接）后若 `WebActivity` 启动失败（实例被删）→ `Toast(@string/instance_missing)`

### `SettingsActivity`

`parentActivityName=".ui.LauncherActivity"`，Toolbar 带返回箭头（`setNavigationOnClickListener { finish() }`）。
布局 `activity_settings.xml`：`MaterialToolbar` id `settingsToolbar`（标题 `@string/settings_title`，`app:navigationIcon="@drawable/ic_arrow_back"`——**注意：`ic_arrow_back` 由 A 提供**，见 §7）→ `RecyclerView` id `settingsProfileList` → `addProfileButton` → `skipLauncherSwitch` → `TextView` id `aboutText`（多行，`@string/about_shell_note` + `\n\n` + `@string/about_privacy`）。

行为与 `LauncherActivity` 共享列表渲染（用同一个 `ProfileListAdapter`，构造参数控制是否显示 `connectButton`；设置页里点行 = 编辑，实例页里点行 = 连接）。

### `ProfileListAdapter`

```kotlin
package org.basmc.moeflow.ui

class ProfileListAdapter(
    private val onRowClick: (Profile) -> Unit,
    private val onEdit: (Profile) -> Unit,
    private val onDelete: (Profile) -> Unit,
) : RecyclerView.Adapter<ProfileListAdapter.Holder>()

fun submit(profiles: List<Profile>, activeId: String?)
```

`item_profile.xml` 为 `MaterialCardView`（`app:cardCornerRadius="12dp"`、`app:cardElevation="0dp"`、`android:clickable="true"`、`android:focusable="true"`），ids：`profileName`、`profileSiteUrl`、`activeBadge`、`editButton`、`deleteButton`。
- 行上只显示**名称与站点地址**。不要在这里显示任何"API 地址"之类的东西：套壳版没有这个概念。
- `activeBadge`：`TextView`，`@string/badge_active`，圆角背景用 `?attr/colorPrimaryContainer`（**不要**新建 drawable，用 `MaterialCardView` 包一层或直接设 `background` 为 `?attr/colorPrimaryContainer` + `paddingHorizontal`）
- `editButton`/`deleteButton` 是 `ImageButton`，`background="?attr/selectableItemBackgroundBorderless"`，`contentDescription` 用 `@string/action_edit` / `@string/action_delete`，`tint` 用 `?attr/colorOnSurfaceVariant`

### `ProfileEditorDialog`

```kotlin
package org.basmc.moeflow.ui

object ProfileEditorDialog {
    /** existing == null 为新增。探测成功/失败结果直接回填到栏位并显示提示，不阻断保存。 */
    fun show(context: Context, existing: Profile?, onSaved: (Profile) -> Unit)
}
```

`dialog_profile_editor.xml`（根 `ScrollView` > `LinearLayout` 垂直，水平内边距 24dp）ids：**只有名称、站点地址、测试连接、允许无效证书四项**。
不要加"预设服务器"选择，也不要加 API 地址栏（理由见 §1 不变量 4）。
- `editorName`（`TextInputLayout` > `TextInputEditText`，hint `@string/field_name`，单行）
- `editorSite`（`TextInputLayout` > `TextInputEditText`，hint `@string/field_site_url`，`inputType="textUri"`，单行）
- `probeButton`（`MaterialButton`，`@string/probe_button`，`style="?attr/materialButtonOutlinedStyle"`）
- `probeResult`（`TextView`，默认 `gone`）
- `editorAllowInvalidCerts`（`SwitchMaterial`，`@string/field_allow_invalid_certs`）
- `allowInvalidCertsHint`（`TextView`，`@string/allow_invalid_certs_hint`，小字次要色）

行为：
- 标题：`@string/editor_title_add` / `@string/editor_title_edit`
- 按钮：`@string/action_cancel` / `@string/action_save`
- 点 `probeButton` → 按钮禁用 + 文案换成 `@string/probe_running`，`lifecycleScope.launch` 调 `ServerProbe.probe(...)`（用 `withContext(Dispatchers.IO)`），回来后：`ProbeOutcome.Ok` → `probeResult` 显示 `@string/probe_ok`（绿色）；`is Failed` → 显示 `@string/probe_failed`（橙色，带原因）。**没有"回填地址"这一步** —— 探测不产出需要保存的东西
- 保存校验：名称空 → `editorName.error = @string/name_required`；站点地址空 → `editorSite.error = @string/site_url_required`；`normalizeSiteUrl` 失败 → `editorSite.error = @string/site_url_invalid`。任一不过就不保存、不关弹窗
- 保存：`ProfileStore.upsert(...)`，构造 `Profile(id = existing?.id ?: UUID.randomUUID().toString(), name, siteUrl = normalized, allowInvalidCerts = ...)`。**探测失败不拦截保存**（服务器临时离线、内网不同网段都是正常情况），弹窗里已经提示过了
- 保存成功后 `onSaved(profile)` 并 dismiss
- 弹窗内的探测必须 `lifecycleScope` 绑定宿主 Activity 的生命周期，避免泄漏

> 为什么要探测按钮：站点地址填错时，套壳版会白屏或落到别处，用户看不出原因。`/ping` 返回 `pong` 是唯一可靠判据（见桌面端 README 同样结论）。

## 6. WebView 宿主契约（C 实现）

### `WebActivity`

**intent 契约（C 与 D 都必须遵守）**：

```kotlin
companion object {
    const val EXTRA_PROFILE_ID = "org.basmc.moeflow.extra.PROFILE_ID"
    fun intent(context: Context, profileId: String): Intent
}
```

布局 `activity_web.xml`（根 `FrameLayout`，`android:fitsSystemWindows="true"`，`android:background="@color/color_surface"`）：

- `WebView` id `webView`，match_parent
- `LinearProgressIndicator` id `webProgress`，高 3dp，`layout_gravity="top"`，`app:trackThickness="3dp"`
- `LinearLayout` id `errorView`（`visibility="gone"`，`layout_gravity="center"`，垂直居中），内含 `TextView` id `errorText` + `MaterialButton` id `retryButton`（`@string/action_retry`）
- `com.google.android.material.floatingactionbutton.FloatingActionButton` id `instanceButton`，`layout_gravity="bottom|end"`，`layout_margin="16dp"`，`app:fabSize="mini"`，`android:src="@drawable/ic_instance"`，`android:contentDescription="@string/action_switch_instance"`，`android:alpha="0.72"`

行为：

1. `onCreate`：取 `profileId`，从 store 读 `Profile`；取不到 → `Toast(@string/instance_missing)` + `finish()`，**不要留下空白页**
2. 加载 `profile.entryUrl`
3. `instanceButton` 单击 → 用 `FLAG_ACTIVITY_REORDER_TO_FRONT` 启动 `LauncherActivity`；**长按** → 弹 `PopupMenu`（`menu_web_fab.xml`：`action_refresh` / `action_switch_instance` / `action_open_in_browser` / `action_about`）：
   - `action_refresh` → `webView.reload()`
   - `action_switch_instance` → 同单击
   - `action_open_in_browser` → 用 `ACTION_VIEW` 打开**当前 URL**，失败 `Toast(@string/external_open_failed)`
   - `action_about` → `MaterialAlertDialogBuilder`：标题 `@string/about_title`，正文 `@string/about_version` + `\n\n` + `@string/about_shell_note` + `\n\n` + `@string/about_export_hint`
4. **返回键**：用 `OnBackPressedCallback`（在 `AndroidManifest` 里开 `android:enableOnBackInvokedCallback="true"`，走 `onBackPressedDispatcher`）。`webView.canGoBack()` → `goBack()`；否则 `finish()`（自然回到实例页）
5. `onNewIntent`：新 id 与当前不同 → 重新读 profile、`webView.loadUrl(entryUrl)`；相同则忽略
6. `onPause` → `CookieManager.getInstance().flush()`；**必须**做，否则 Android 会在进程被回收时丢掉会话 cookie
7. `onDestroy` → 把 WebView 从父容器摘下来、`loadUrl("about:blank")`、`destroy()`（防泄漏）
8. 键盘：`AndroidManifest` 的 `windowSoftInputMode="adjustResize"`
9. 配置变更：`configChanges="orientation|screenSize|screenLayout|smallestScreenSize|keyboardHidden|uiMode"`，避免旋转重建；同时在 `onSaveInstanceState` 里存 `webView.saveState(outState)` 作为兜底

### WebView 设置（`WebActivity` 内一次性配好）

```kotlin
settings.javaScriptEnabled = true
settings.domStorageEnabled = true          // 前端会用 localStorage
// 不要设 databaseEnabled —— 它对应早已废弃的 WebSQL，AGP 会报弃用警告，现代 WebView 里也没有效果
settings.loadsImagesAutomatically = true
settings.setSupportMultipleWindows(false)
settings.javaScriptCanOpenWindowsAutomatically = false
settings.mediaPlaybackRequiresUserGesture = true
settings.mixedContentMode = WebSettings.MIXED_CONTENT_COMPATIBILITY_MODE
settings.userAgentString = settings.userAgentString + " MoeFlowAndroid/${BuildConfig.VERSION_NAME}"
settings.setSupportZoom(false)               // 页面自己声明 user-scalable=no
settings.builtInZoomControls = false
settings.displayZoomControls = false
settings.domStorageEnabled = true
```

Cookie：
```kotlin
CookieManager.getInstance().apply {
    setAcceptCookie(true)
    setAcceptThirdPartyCookies(webView, true)
}
```
> 为什么不做实例间的 cookie 隔离：Cookie 天然按 host 隔离，而套壳版每个实例是**各自的真实域名**，所以多实例共用一个 cookie jar 也不会串号。（桌面端需要按端口隔离，恰恰是因为它把所有实例都反代到了同一个 `127.0.0.1`。）这一点在代码注释里写清楚，避免后来者"修"成清 cookie —— 那会把用户直接登出。

**不要**做边到边（edge-to-edge）特殊处理：根布局用 `android:fitsSystemWindows="true"`，Android 15 强制边到边时它会照常吃掉系统栏内边距（targetSdk 35 下必须依赖这条，否则内容会被状态栏盖住）。

### `MoeFlowWebViewClient`

```kotlin
package org.basmc.moeflow.web

class MoeFlowWebViewClient(
    private val activity: WebActivity,
    private val profile: Profile,
    private val onPageStarted: (String) -> Unit,
    private val onPageFinished: (String) -> Unit,
    private val onError: (ErrorKind, String?, Int) -> Unit,
) : WebViewClient() {

    enum class ErrorKind { OFFLINE, HTTP, SSL, GENERIC }
}
```

`shouldOverrideUrlLoading` 规则（顺序执行，返回 `true` 表示我们接手）：
1. `url` 是 `http`/`https`：
   - host 属于**应用内 host**（忽略大小写）：站点自身，或它的**预设 API 主机**（尨译的 API 在 `api.moetran.com`）→ 返回 `false`，WebView 自己导航
     > 为什么要认 API 主机：服务器导出完成后的成品 zip 链接可能指过去，丢给系统浏览器的话用户在应用里就永远等不到那个下载。API 主机是**从预设表推出来的**，不是存在实例上 —— 套壳版没有"API 地址"这个设置项。
   - 其它 host → 用 `ACTION_VIEW` 交给系统浏览器，返回 `true`；`ActivityNotFoundException` → `Toast(@string/external_open_failed)`，返回 `true`
2. 其它 scheme（`mailto:` / `tel:` / `intent:` / `market:` …）→ `ACTION_VIEW`（`tel:` 用 `ACTION_DIAL`），返回 `true`
3. `blob:` / `data:` → 交给 `BlobBridge`（见下），返回 `true`

`onReceivedError`：主框架（`request.isForMainFrame`）才处理，按 `WebResourceError.errorCode` 归类（`ERROR_HOST_LOOKUP` / `ERROR_CONNECT` / `ERROR_TIMEOUT` → `OFFLINE`；其余 `GENERIC`），回调 `onError`。
`onReceivedHttpError`：仅主框架且 `statusCode >= 400` 时回调 `HTTP`（排除 401/403 —— 那是前端自己的鉴权流程，不能弹错误页把登录界面盖掉）。
`onReceivedSslError`：`profile.allowInvalidCerts` 为 true → `handler.proceed()`，并在 `Toast` 提示一次（`@string/allow_invalid_certs_toast`，每次 Activity 生命周期只提示一次）；否则 `handler.cancel()` 并回调 `SSL`。

> `handler.proceed()` 是 `@SuppressLint("WebViewClientOnReceivedSslError")` 的显式例外，注释里说明：这是用户在实例设置里主动开的开关，仅用于内网自签名站点。

### `MoeFlowWebChromeClient`

```kotlin
class MoeFlowWebChromeClient(
    private val activity: WebActivity,
    private val onProgress: (Int) -> Unit,
) : WebChromeClient()
```

- `onProgressChanged` → 回调进度（`< 100` 显示进度条，`100` 隐藏）
- `onShowFileChooser` → 用 `activity.registerForActivityResult` 拿到的 `ActivityResultLauncher` 启动 `Intent(Intent.ACTION_GET_CONTENT)`，`type` 取 `fileChooserParams.acceptTypes.firstOrNull()` 经 `normalizeMimeType()` 处理（**只能有一个 MIME，多值要合并成 `*/*`**；空/异常值一律 `*/*`），`putExtra(Intent.EXTRA_ALLOW_MULTIPLE, fileChooserParams.mode == MODE_OPEN_MULTIPLE)`。**必须同时接住单选与多选两种返回**：`result.data?.data`（Uri）与 `result.data?.clipData`（多选）都要处理，组装成 `Uri[]` 交给 `fileChooserParams.parseResult(resultCode, result.data)`。`fileChooserParams` 也要能接受 `createIntent()` 的结果（行为更完整），但**不要**用 `fileChooserParams.createIntent()` 之外的隐式 intent，保持 `EXTRA_ALLOW_MULTIPLE` 可控
- `onPermissionRequest` → `request.deny()` + `Toast(@string/permission_denied)`。在注释里写明这是当前版本的已知限制（摄像头/麦克风未启用）
- `onCreateWindow` → 返回 `false`（不允许弹新窗口）

> 文件选择是**必须做对**的部分：前端有作品文件上传（`FileList.tsx`）。这里回归了就没人能上传稿子。

### `DownloadSupport`

`setDownloadListener` 的实现，处理 `http`/`https` 下载（**服务器导出的成品 zip 就走这条路**，所以这不是可选项）：

```kotlin
object DownloadSupport {
    /** DownloadListener 的实体。返回 true 表示已接手。 */
    fun handle(
        activity: Activity,
        webView: WebView,
        url: String,
        userAgent: String?,
        contentDisposition: String?,
        mimeType: String?,
        contentLength: Long,
    ): Boolean
}
```

- 用 `DownloadManager.Request(Uri.parse(url))`
- **必须**带上 cookie 与 Referer，否则服务端会拒绝：`addRequestHeader("Cookie", CookieManager.getInstance().getCookie(url) ?: "")`、`addRequestHeader("Referer", profile.siteUrl + "/")`、`setUserAgent(userAgent ?: webView.settings.userAgentString)`
- `setNotificationVisibility(VISIBLE_NOTIFY_COMPLETED)`
- 目标目录：API 29+ 用 `setDestinationInExternalPublicDir(DIRECTORY_DOWNLOADS, ...)`（分区存储下免权限）；**API 26-28 用 `setDestinationInExternalFilesDir(...)`** —— 写公共目录在低版本要 `WRITE_EXTERNAL_STORAGE` 运行时授权，为一条下载路径引一套权限流程不划算。两种都落在文件名上，行为差异写进 README 的已知限制
- 失败（`SecurityException` / `IllegalArgumentException` / 无法解析 URL）→ `Toast(@string/download_failed)`
- 成功入队 → `Toast(@string.download_started)`

> Referer 不是可有可无：尨译的 OSS 桶（`c01.m-t.pics`）有防盗链且拒绝空 Referer。桌面端的反代必须手动补 Referer，浏览器/DownloadManager 同样要补。

### `BlobBridge`

`DownloadListener` 拿到的 `blob:` / `data:` URL 无法交给 `DownloadManager`。做法：在页面上下文里把内容读成 base64，经 JS 接口回传落盘。

```kotlin
class BlobBridge(private val activity: WebActivity, private val profile: Profile) {
    /** JS 侧调用入口，名字固定为 "MoeFlowBridge"，方法固定为 saveBlob(name, mime, base64)。 */
    fun installOn(webView: WebView)

    /** 页面里触发一次 blob 下载。 */
    fun requestBlob(webView: WebView, blobUrl: String, suggestedName: String)
}
```

- `addJavascriptInterface(this, "MoeFlowBridge")`，方法标 `@JavascriptInterface`
- `saveBlob` 里**先校验**：当前页面 host 必须等于 `profile.host`，否则直接丢弃并打日志
  - **护栏必须读 `WebActivity.currentUrl`**（由 UI 线程维护的 `@Volatile` 字段），**不要**在 JS 线程里用 `runOnUiThread` 去取 `WebView.getUrl()`：那是异步投递，"取"回来的永远是 null，护栏会退化成"永远拒绝"，blob 下载被静默丢弃
  - `addJavascriptInterface` 无法知道调用方 frame 的 origin，这只是近似护栏（挡不住同页 XSS），注释写明
- 落盘：API 29+ 用 `MediaStore.Downloads`（`EXTERNAL_CONTENT_URI` + `IS_PENDING` 两段式写入）；**API 26-28 必须走应用专属目录**（`getExternalFilesDir(DIRECTORY_DOWNLOADS)`），因为 `MediaStore.Downloads` 这个类本身是 API 29 才加的，低版本引用它会 `NoSuchFieldError`。文件名一律经 `UrlTools.guessFileName` 清洗
- `requestBlob` 用 `evaluateJavascript` 跑一段固定脚本：`fetch(blobUrl).then(r=>r.blob()).then(b=>{ const fr=new FileReader(); ... })`，用 `FileReader.readAsDataURL` 得到 base64 后调用 `window.MoeFlowBridge.saveBlob(...)`。脚本必须**自包含、无模板注入**（blob URL 用 `JSONObject.quote()` 转义后拼进去）
- 体积护栏：base64 超过 **8 MB** 时放弃并 `Toast(@string/download_too_large)`（跨 JNI 传超大字符串会 OOM）
- `data:` URL 同样走这条路：直接从 `evaluateJavascript` 里 `fetch()` 它

### 错误页（`errorView`）

`onError` 时：隐藏 WebView？**不要**。做法是把 `errorView` 显示在 WebView 之上（`bringToFront()`），文案按 `ErrorKind`：

- `OFFLINE` → `@string/error_offline`（参数 = `profile.siteUrl`）
- `HTTP` → `@string/error_http`（参数 = 状态码）
- `SSL` → `@string/error_ssl`
- `GENERIC` → `@string/error_generic`（参数 = 描述）

`retryButton` → 隐藏 `errorView` + `webView.reload()`。

**每条失败路径都必须有可见输出**（桌面端的教训：静默失败等于"点了没反应"，而这里大部分远程调用都可能失败）。

## 7. 资源契约（A 实现）

### `res/values/colors.xml`

```xml
<color name="color_primary">#4A6CF7</color>
<color name="color_on_primary">#FFFFFF</color>
<color name="color_primary_container">#E4E9FE</color>
<color name="color_on_primary_container">#1B2A6B</color>
<color name="color_surface">#FFFFFF</color>
<color name="color_background">#F6F7FB</color>
<color name="color_on_surface">#1B1D23</color>
<color name="color_on_surface_variant">#5A6072</color>
<color name="color_outline">#DCE0EA</color>
<color name="color_error">#C0392B</color>
<color name="ic_launcher_background">#FFFFFF</color>   <!-- 图标背景层：与 logo 自身的白底一致 -->
```

### `res/values/themes.xml`（+ `res/values-v27/themes.xml`）

```xml
<!-- values/themes.xml：各 API 级别共用 -->
<style name="Theme.MoeFlow.Base" parent="Theme.Material3.Light.NoActionBar">
    <item name="colorPrimary">@color/color_primary</item>
    <item name="colorOnPrimary">@color/color_on_primary</item>
    <item name="colorPrimaryContainer">@color/color_primary_container</item>
    <item name="colorOnPrimaryContainer">@color/color_on_primary_container</item>
    <item name="android:colorBackground">@color/color_background</item>
    <item name="colorSurface">@color/color_surface</item>
    <item name="colorOnSurface">@color/color_on_surface</item>
    <item name="colorOnSurfaceVariant">@color/color_on_surface_variant</item>
    <item name="colorOutline">@color/color_outline</item>
    <item name="colorError">@color/color_error</item>
    <item name="android:windowBackground">@color/color_surface</item>
    <item name="android:statusBarColor">@color/color_surface</item>
    <item name="android:windowLightStatusBar">true</item>
    <!-- 故意不设 navigationBarColor，理由见下 -->
</style>

<style name="Theme.MoeFlow" parent="Theme.MoeFlow.Base" />
```

```xml
<!-- values-v27/themes.xml：API 27+ 才有的浅色导航栏 -->
<style name="Theme.MoeFlow" parent="Theme.MoeFlow.Base">
    <item name="android:navigationBarColor">@color/color_surface</item>
    <item name="android:windowLightNavigationBar">true</item>
</style>
```

**为什么要拆成 Base + v27 两份**：`android:windowLightNavigationBar` 是 API 27 才加的属性，而 minSdk 是 26。写在 `values/` 里 lint 直接报 `NewApi`（低版本上这类属性 id 可能撞上厂商私有属性）。而带限定符的资源是**整体替换**而非逐项合并，所以公共部分必须先抽成 `Base`，否则 v27 那份要把所有条目抄一遍。

**API 26 上故意不设 `navigationBarColor`**：那一档不存在"浅色导航栏图标"这个能力，把栏刷成浅色又没法把图标变深，就是白底白图标、三个按钮看不见。留系统默认（深色栏 + 浅色图标）最稳。

固定浅色：上游前端不含深色模式，`DayNight` 会让我们自己的 chrome 与页面不同步。状态栏用浅色 + 深色图标，理由：targetSdk 35 在 Android 15 上会强制边到边并忽略 `android:statusBarColor`，此时状态栏图标颜色只由 `windowLightStatusBar` 决定，而它背后是浅色窗口背景 —— 写 `false` 会得到"白底白图标"。**各版本表现一致**优先于"看起来更花"。

### `res/xml/network_security_config.xml`

```xml
<network-security-config>
    <base-config cleartextTrafficPermitted="true">
        <trust-anchors><certificates src="system" /></trust-anchors>
    </base-config>
</network-security-config>
```

需要明文（`http://`）的原因：内网部署形态确实是 `http://172.29.133.24`（见桌面端 README 的四种部署形态）。在文件里写明这一取舍。

### drawable（全部为 `24dp` 矢量，`android:viewportWidth/Height="24"`，`android:fillColor="#FF000000"`，在调用处用 `app:tint` / `android:tint` 着色）

| 文件名 | 用途 | pathData |
|---|---|---|
| `ic_add` | 添加实例 | `M19,13h-6v6h-2v-6H5v-2h6V5h2v6h6v2z` |
| `ic_edit` | 编辑 | `M3,17.25V21h3.75L17.81,9.94l-3.75,-3.75L3,17.25zM20.71,7.04c0.39,-0.39 0.39,-1.02 0,-1.41l-2.34,-2.34c-0.39,-0.39 -1.02,-0.39 -1.41,0l-1.83,1.83 3.75,3.75 1.83,-1.83z` |
| `ic_delete` | 删除 | `M6,19c0,1.1 0.9,2 2,2h8c1.1,0 2,-0.9 2,-2V7H6v12zM19,4h-3.5l-1,-1h-5l-1,1H5v2h14V4z` |
| `ic_refresh` | 刷新 | `M17.65,6.35C16.2,4.9 14.21,4 12,4c-4.42,0 -7.99,3.58 -8,8s3.58,8 8,8c3.73,0 6.84,-2.55 7.73,-6h-2.08c-0.82,2.33 -3.04,4 -5.65,4 -3.31,0 -6,-2.69 -6,-6s2.69,-6 6,-6c1.66,0 3.14,0.69 4.22,1.78L13,11h7V4l-2.35,2.35z` |
| `ic_open_in_new` | 在浏览器打开 | `M19,19H5V5h7V3H5c-1.11,0 -2,0.9 -2,2v14c0,1.1 0.89,2 2,2h14c1.1,0 2,-0.9 2,-2v-7h-2v7zM14,3v2h3.59l-9.83,9.83 1.41,1.41L19,6.41V10h2V3h-7z` |
| `ic_settings` | 设置 | `M19.14,12.94c0.04,-0.3 0.06,-0.61 0.06,-0.94c0,-0.32 -0.02,-0.64 -0.07,-0.94l2.03,-1.58c0.18,-0.14 0.23,-0.41 0.12,-0.61l-1.92,-3.32c-0.12,-0.22 -0.37,-0.29 -0.59,-0.22l-2.39,0.96c-0.5,-0.38 -1.03,-0.7 -1.62,-0.94L14.4,2.81c-0.04,-0.24 -0.24,-0.41 -0.48,-0.41h-3.84c-0.24,0 -0.43,0.17 -0.47,0.41L9.25,5.35C8.66,5.59 8.12,5.92 7.63,6.29L5.24,5.33c-0.22,-0.08 -0.47,0 -0.59,0.22L2.74,8.87C2.62,9.08 2.66,9.34 2.86,9.48l2.03,1.58C4.84,11.36 4.8,11.69 4.8,12s0.02,0.64 0.07,0.94l-2.03,1.58c-0.18,0.14 -0.23,0.41 -0.12,0.61l1.92,3.32c0.12,0.22 0.37,0.29 0.59,0.22l2.39,-0.96c0.5,0.38 1.03,0.7 1.62,0.94l0.36,2.54c0.05,0.24 0.24,0.41 0.48,0.41h3.84c0.24,0 0.44,-0.17 0.47,-0.41l0.36,-2.54c0.59,-0.24 1.13,-0.56 1.62,-0.94l2.39,0.96c0.22,0.08 0.47,0 0.59,-0.22l1.92,-3.32c0.12,-0.22 0.07,-0.47 -0.12,-0.61L19.14,12.94zM12,15.6c-1.98,0 -3.6,-1.62 -3.6,-3.6s1.62,-3.6 3.6,-3.6s3.6,1.62 3.6,3.6S13.98,15.6 12,15.6z` |
| `ic_instance` | 实例（FAB） | `M20,13H4c-0.55,0 -1,0.45 -1,1v6c0,0.55 0.45,1 1,1h16c0.55,0 1,-0.45 1,-1v-6c0,-0.55 -0.45,-1 -1,-1zM7,19c-1.1,0 -2,-0.9 -2,-2s0.9,-2 2,-2 2,0.9 2,2 -0.9,2 -2,2zM20,3H4c-0.55,0 -1,0.45 -1,1v6c0,0.55 0.45,1 1,1h16c0.55,0 1,-0.45 1,-1V4c0,-0.55 -0.45,-1 -1,-1zM7,9c-1.1,0 -2,-0.9 -2,-2s0.9,-2 2,-2 2,0.9 2,2 -0.9,2 -2,2z` |
| `ic_info` | 关于 | `M11,7h2v2h-2zM11,11h2v6h-2zM12,2C6.48,2 2,6.48 2,12s4.48,10 10,10 10,-4.48 10,-10S17.52,2 12,2zM12,20c-4.41,0 -8,-3.59 -8,-8s3.59,-8 8,-8 8,3.59 8,8 -3.59,8 -8,8z` |
| `ic_arrow_back` | 返回 | `M20,11H7.83l5.59,-5.59L12,4l-8,8 8,8 1.41,-1.41L7.83,13H20v-2z` |

### 启动图标（前景位图，不是矢量）

前景层是**从上游 logo 生成**的位图，不放在 `drawable/`，而是每个密度一份
`mipmap-<density>/ic_launcher_foreground.png`（画布 108dp：mdpi 108px / hdpi 162 / xhdpi 216 / xxhdpi 324 / xxxhdpi 432）。

来源：`frontend/public/static/logo512.png` —— 与桌面端 `tauri icon` 用的是同一份。

**不能直接把 logo 当自适应图标的前景层。** 它是一张**不透明的白底方图**（源 PNG 根本没有 alpha 通道），
而自适应图标保证不被裁的只有画布中央 **66dp 圆**（多数启动器的遮罩约 72dp）。直接套用，角色的下半身
会被遮罩切掉。所以生成时做两件事：

1. **去白底**（标准 un-matte）：`alpha = 1 - min(r,g,b)/255`，再按 alpha 反求原始墨色
   `fg = (c - 255*(1-a)) / a`。这份线稿去底后墨色恒为 `(0,0,0)`，alpha 承载深浅 —— 正好也是
   `monochrome` 层需要的形态（系统只取 alpha 再按壁纸染色）。
2. **缩到安全区内**：把图案包围盒内接进 **72dp 圆**，居中放在 108dp 画布上。
   实测三档留白的观感对比后选定的这一档：圆内接 66dp 偏小，方内接 66dp 在圆形遮罩下会碰到边界。

对照：仓库里 `src-tauri/icons/android/` 那份是 Tauri 生成的，它的 `ic_launcher_foreground.png`
是**整张不透明**的直角方图（我们量过：不透明包围盒 = 整张画布 100%），不适合直接拿来用。

### `mipmap-anydpi/ic_launcher.xml`

```xml
<adaptive-icon>
    <background android:drawable="@color/ic_launcher_background" />
    <foreground android:drawable="@mipmap/ic_launcher_foreground" />
    <monochrome android:drawable="@mipmap/ic_launcher_foreground" />
</adaptive-icon>
```

目录**不带 `-v26` 限定符**：minSdk 已经是 26，再加是多余的（lint 的 `ObsoleteSdkInt` 会报）。
`monochrome` 层是给 Android 13+ 的「主题图标」用的（系统只取 alpha 通道再按壁纸染色），
直接复用前景位图 —— 它本身就是"黑色墨 + alpha 深浅"，正合要求；不加这一层 lint 会报
`MonochromeLauncherIcon`。

manifest 只引用 `@mipmap/ic_launcher`，**不写 `roundIcon`**：既然已经提供了自适应图标，
`roundIcon` 再指一张传统位图反而会让"请求圆形图标"的启动器拿到一张非自适应的图。


### `res/values/strings.xml`（**键名唯一，值照抄**）

```
app_name                         MoeFlow
launcher_title                   MoeFlow
launcher_subtitle                选择要连接的实例
action_settings                  设置
action_add_instance              添加实例
action_edit                      编辑
action_delete                    删除
action_refresh                   刷新
action_switch_instance           切换实例
action_open_in_browser           在浏览器中打开
action_about                     关于
action_retry                     重试
action_cancel                    取消
action_save                      保存
content_desc_back                返回
badge_active                     当前使用
skip_launcher                    启动时直接进入上次使用的实例
empty_instances                  还没有实例，点「添加实例」添加一个站点
last_instance_cannot_be_deleted  至少保留一个实例，否则应用将无处可连
delete_confirm_title             删除实例
delete_confirm_message           确定要删除「%1$s」吗？该实例的登录状态也会一并清除。
deleted                          已删除「%1$s」
settings_title                   设置
editor_title_add                 添加实例
editor_title_edit                编辑实例
field_name                       名称
field_site_url                   站点地址
field_allow_invalid_certs        允许无效证书
allow_invalid_certs_hint         仅在内网自签名证书时开启；开启会降低连接安全性
probe_button                     测试连接
probe_running                    正在探测…
probe_ok                         站点连接正常
probe_failed                     无法连接到该站点：%1$s
name_required                    请填写名称
site_url_required                请填写站点地址
site_url_invalid                 站点地址无法解析：%1$s
error_offline                    无法连接到「%1$s」，请检查网络或站点地址。
error_http                       服务器返回 HTTP %1$d
error_ssl                        证书校验失败。若这是内网自签名站点，可在实例设置里开启「允许无效证书」。
error_generic                    加载失败：%1$s
download_started                 已开始下载「%1$s」
download_failed                  下载失败：%1$s
download_too_large               文件过大，无法在应用内保存，请改用「在浏览器中打开」
external_open_failed             没有可以打开该链接的应用
instance_missing                 实例不存在，已返回实例选择
permission_denied                该网页请求了摄像头或麦克风权限，当前版本未启用
allow_invalid_certs_toast        正在忽略证书校验（已在实例设置中开启）
about_title                      关于
about_version                    版本 %1$s
about_shell_note                 本版本是 Android 套壳客户端：直接加载站点页面，不含桌面端的本地反向代理、图片磁盘缓存与本地导出。
about_export_hint                服务器导出的成品会通过系统下载保存到「下载」目录（Android 9 及以下落在应用专属目录）。
about_privacy                    应用不收集任何数据；登录凭据保存在系统 WebView 的 Cookie 中，卸载应用即清除。
```

带 `%1$s` 的条目必须写 `<string name="x">…%1$s…</string>`（不要加 `formatted="false"`）。

### `AndroidManifest.xml` 要点

- `<uses-permission android:name="android.permission.INTERNET" />`
- `<uses-permission android:name="android.permission.ACCESS_NETWORK_STATE" />`
- `<application>`：`android:name=".MoeFlowApp"`、`android:allowBackup="false"`（**不要**允许备份：WebView 的 cookie 会被一起带走，里面有会话令牌）、`android:icon="@mipmap/ic_launcher"`、`android:label="@string/app_name"`、`android:networkSecurityConfig="@xml/network_security_config"`、`android:supportsRtl="true"`、`android:theme="@style/Theme.MoeFlow"`、`android:usesCleartextTraffic="true"`、`android:enableOnBackInvokedCallback="true"`
- `LauncherActivity`：`exported="true"`、`launchMode="singleTop"`、`windowSoftInputMode="adjustResize"`、MAIN/LAUNCHER intent-filter
- `SettingsActivity`：`exported="false"`、`parentActivityName=".ui.LauncherActivity"`
- `WebActivity`：`exported="false"`、`launchMode="singleTop"`、`windowSoftInputMode="adjustResize"`、`configChanges="orientation|screenSize|screenLayout|smallestScreenSize|keyboardHidden|uiMode"`
- 三个 activity 都给 `android:configChanges`（同上）

## 8. 构建配置要点（A 实现）

- `settings.gradle.kts`：`pluginManagement` + `dependencyResolutionManagement`（`repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)`，仓库 `google()`、`mavenCentral()`）；`rootProject.name = "moeflow-android"`；`include(":app")`
- 根 `build.gradle.kts`：只用 `plugins { id(...) apply false }` 声明 AGP/Kotlin 版本（用 `alias` 或显式版本号均可，不要引入 version catalog 文件）
- `gradle.properties`：`org.gradle.jvmargs=-Xmx2048m -Dfile.encoding=UTF-8`、`android.useAndroidX=true`、`android.nonTransitiveRClass=true`、`kotlin.code.style=official`
- `app/build.gradle.kts`：`namespace = "org.basmc.moeflow"`、`applicationId = "org.basmc.moeflow"`、`versionCode = 1`、`versionName = "0.1.0"`、`buildFeatures { viewBinding = true; buildConfig = true }`、`compileOptions` 与 `kotlinOptions.jvmTarget` 均为 Java 17、`buildTypes { release { isMinifyEnabled = true; proguardFiles(...) } }`
  - **`buildConfig = true` 不能省**：AGP 8.x 起该特性默认关闭，而代码要用 `BuildConfig.VERSION_NAME` 拼版本号与 User-Agent，不开就是 `Unresolved reference: BuildConfig`
  - 不要用 `java.toolchain`：构建机上不一定装着对应 JDK，会让构建在 toolchain 探测阶段失败
- `proguard-rules.pro`：至少保留 `@JavascriptInterface` 注解的方法（`-keepclassmembers class * { @android.webkit.JavascriptInterface <methods>; }`），否则 release 版会静默删掉 `BlobBridge.saveBlob`——这是 release-only 的坑，注释写清楚
- **必须配 release 签名**（`signingConfigs` + `buildTypes.release.signingConfig`），否则 AGP 产出的是
  `app-release-unsigned.apk`，而 **Android 一律拒绝安装未签名的包**（报「安装包无效 / 解析失败」，
  现象很像包损坏，其实只是没签名）。
  - 凭据从 `android/keystore.properties` 读（**gitignore**，模板是 `keystore.properties.example`）；`*.jks` 同样 gitignore
  - 凭据缺失时**退回 debug 签名**并在构建日志里警告 —— 保证 `assembleRelease` 永远出一个可安装的包，
    而不是默默产出装不上的东西。但要在 README 里写明：debug 签名不适合分发，且将来换正式密钥后覆盖安装不上
  - 密钥是**升级身份**（包名 + 签名），换密钥 = 用户必须先卸载 = 登录态丢失。这条必须写进 README
- `gradle/wrapper/gradle-wrapper.properties`：`distributionUrl=https\://services.gradle.org/distributions/gradle-8.11.1-bin.zip`
- `.gitignore`：`.gradle/`、`build/`、`local.properties`、`*.iml`、`.idea/`、`captures/`、`.cxx/`、`.externalNativeBuild/`

## 9. 验证方式与结果

### 当时的约束
写这版代码的机器上**没有 Android SDK / Gradle / adb**（只有 JDK 21），所以第一轮交付时只做了静态复核。
随后 SDK 与 Gradle 装好，**已经补跑过完整构建**，结果见下。

静态复核（在没有 SDK 时唯一可行的办法）值得保留成习惯：用脚本从 `res/**` 提取全部定义
（`<string name>`、`@+id`、drawable/menu/layout 文件名、`<color name>`、`<style name>`），
再与 Kotlin/XML 里的 `R.string.x` / `@string/x` / `binding.x` 引用做差集。
**`binding.<name>` 要按「Binding 类 → 布局文件」的映射分别核对**，否则会把 `binding.root`
（ViewBinding 自带属性，不是布局 id）误报成缺失。

### 已跑通的（本机实测）

| 命令 | 结果 |
|---|---|
| `./gradlew :app:assembleDebug` | 成功，`app-debug.apk` 约 5.8 MB |
| `./gradlew :app:assembleRelease` | 成功（R8 + `lintVitalRelease`），`app-release-unsigned.apk` 约 2.14 MB |
| `./gradlew :app:lintDebug` | 0 error / 10 warning（剩余均为刻意取舍） |
| `aapt2 dump badging`（debug 与 release） | 包名 `org.basmc.moeflow`、versionName 0.1.0、minSdk 26、targetSdk 35、权限仅 INTERNET + ACCESS_NETWORK_STATE、入口 `org.basmc.moeflow.ui.LauncherActivity` |
| `mapping.txt` 核对 | `BlobBridge.saveBlob` 在 release 里**保留原名**（类被混淆成 `A1.c`，方法名未变）。这条必须查：它被 R8 改名的话，JS 侧 `window.MoeFlowBridge.saveBlob(...)` 就是静默失效，而且只有 release 版才会出问题 |

**未做**：真机/模拟器验证。`adb devices` 没有连接设备，所以运行时行为（会话保持、上传、下载、切实例）
仍待真机确认 —— 清单见 `android/README.md` 的「验证状态」。

### 工具链的两个坑（都已写进 README）

- `DownloadManager.Request.setUserAgent()` 是 **API 36** 才加的方法，`compileSdk = 35` 下直接调是
  `Unresolved reference`。改用 header 表加 `User-Agent`（API < 36 上 DownloadManager 不会自己补这个头）。
- 改**资源目录结构**后（如 `mipmap-anydpi-v26/` → `mipmap-anydpi/`），AGP 增量资源合并会把旧目录留在
  `packaged_res` 里，随后报 `resource mipmap/ic_launcher not found`。`clean` 一次即可，不是配置错误。

## 10. 已知限制（要写进 README，不要装作没有）

1. 摄像头/麦克风未启用（`onPermissionRequest` 一律拒绝）
2. blob/data 下载超过 8 MB 会失败
3. 同一域名的两个实例共用登录态（Cookie 按 host 隔离，这是浏览器语义，不是 bug）
4. 明文 HTTP 全量放行（`cleartextTrafficPermitted="true"`），为兼容内网部署形态
5. 不含本地导出、图片缓存、代理设置（相对桌面端的功能缺口，需求已确认）
6. 未签名 APK 需开发者模式侧载
