# 本地 Codex 翻译

桌面 dock 中的“本地 Codex 翻译”用于 `/image-translator/<fileID>-<targetID>`。
不修改上游前端；普通浏览器中不显示入口，也不调用 Codex。

没有标签时使用 `label-only`：识别原文和像素框，不翻译。有标签时使用
`translate-only`：排除 `kind === 'author'`，按原 rank 翻译，图片仅作上下文。
先读取文件详情；有 `project_id` 时获取该项目的 targets，严格匹配 `id === targetID`，
依次使用 `language.en_name`、`i18n_name`、`code` 中首个非空值，显示“语言：<name>（自动）”。
匹配失败或语言为空时阻止执行，不猜测语言。仅当旧服务器缺少 `project_id` 时显示“语言：手填”，
输入初始为空，按 `targetID` 在 localStorage 记住并恢复上次输入；未填写时不能预览或开始。
交给 Codex 的是语言名称，写入仍使用当前 `target_id: targetID`。

图片由 webview 的 `createImageBitmap` 和 canvas 转为 PNG；最长边超过 2000 px
时等比缩小，预览显示交给 Codex 的尺寸。框坐标均针对该尺寸，不需要缩放回原图。
识别模式归一化会裁剪框、保证宽高至少 1 px、丢弃空项，并修复无效或重复 rank。
`text` 为空时使用 `textLines.join('')`，保留有文本行的项目；`comment` 取译文（此模式为空）。
翻译模式仅接受已有标签的整数 rank，每个 rank 最多保留一次；保留稀疏的原 rank，
避免把译文重新编号到另一个标签。翻译行只有 `{rank,translated}`，不构造像素框；
预览另存原 rank 到已有 source ID 的映射，写入时使用该映射。

## HTTP 契约

请求经过同源 axum `/api/*` 代理，携带 cookie `token` 对应的
`Authorization: Bearer <token>`。失败显示 HTTP 状态及服务器响应正文。

- `GET /api/v1/files/{fileID}` 返回 `{url,name,project_id?,...}`。
  `url` 已由 Rust 重写成同源 `/__media/<base64 origin>/...`，直接获取该图片。
- `GET /api/v1/files/{fileID}/sources?target_id=<targetID>&paging=false`
  返回数组 `[{id,x,y,rank,content,kind,...}]`，x/y 是归一化中心坐标。
- 有 `project_id` 时，`GET /api/v1/projects/{project_id}/targets?limit=100000`
  返回目标数组（也兼容 `{list:[...]}`），用于解析当前目标语言。
- 仅 `label-only`：`POST /api/v1/files/{fileID}/sources`，JSON 为
  `{x:(left+width/2)/W,y:(top+height/2)/H,content,rank}`，x/y 裁剪至 0..1。
  只创建标签，不写译文。依 rank 顺序逐条等待写入完成。
- 仅 `translate-only`：`POST /api/v1/sources/{label.id}/translations`，JSON 为
  `{content:translated,target_id:targetID}`。

翻译模式将译文附加到已有标签，不创建任何 source；预览显示新建 0 个标签及待写入译文数。
识别模式显示待新建标签数及 0 条译文。首次写入前及每条写入前均比较当前页面与预览捕获的
fileID/targetID，切页即停止。这里没有删除或自动回滚。

## Codex CLI 契约

可执行文件按四级顺序解析：**「设置 → 本地 Codex」里保存的路径** → 环境变量 `CODEX_BIN`
→ 常见安装位置（`%USERPROFILE%\Codex\_internalppesources\codex.exe`、
`%LOCALAPPDATA%\Codex\_internalppesources\codex.exe`、`%LOCALAPPDATA%\Programs\Codex\codex.exe`、
`%ProgramFiles%\Codex\codex.exe`，以及 PATH 里的 `codex.exe`）→ 裸 `codex`。

这条顺序不是可选的：Codex 桌面版不会把 CLI 放进 PATH，而 GUI 进程也拿不到用户在终端里改的 PATH，
所以「直接 spawn codex」在「只装了 Codex 应用」的机器上必然失败（`program not found`）。
设置页的**一键锁定**会把上述位置逐个试 `--version`，记住第一个能应答的路径；
`codex_status` 返回配置值、实际解析结果、版本与候选清单，供设置页展示。
`codex.cmd` / `.ps1` 这类 npm 包装脚本会被明确拒绝——运行它们需要经过 shell，
而提示词是用户内容，不能交给 cmd.exe。直接启动可执行文件，不经过 shell，Windows 设置 `CREATE_NO_WINDOW`。
使用前仍需完成 Codex 登录。

```text
codex --version
codex exec --skip-git-repo-check --ephemeral -s read-only -C <tmpdir> --output-schema <schema.json> [-m <model>] [-i <png>]... -o <last-message.json> -
```

`codex_probe` 执行第一条（10 秒超时），失败时禁用按钮并在提示中显示原因。
`codex_run({req:{prompt,schema,images,model,timeout_ms}})` 执行第二条；
images 是无 data URL 前缀的 PNG base64。prompt 写入 stdin；每张图通过 `-i` 传入。
本 UI 每次传一张图，不覆盖 CLI 默认模型。返回 `{json,ms}`。

每次运行创建进程 ID 加计数器命名的临时目录，保存 schema、图片和输出，成功和失败
都清理。非零退出返回 stderr 最后 4000 字节；成功优先读 `-o` 文件，缺失或空白则
回退 stdout。默认及最大超时均为 180 秒，超时杀死并等待子进程退出。
全应用同时只运行一个识别任务，第二个请求立即返回“已有任务在跑”。

识别输出为 `{imageW,imageH,texts:[{rank,left,top,width,height,textLines,text,translated,comment}]}`，
其中 translated 为空。翻译输出为 `{texts:[{rank,translated}]}`。
JS 完成归一化，信任实际 canvas 尺寸而非模型返回的尺寸。

## 试用

1. 在有构建环境的机器构建桌面客户端，确保真实 Codex 可执行文件可用、已登录且模型支持图片。
2. 登录服务器，打开图片翻译页。非图片页的按钮提示“请先打开一个图片翻译页”。
3. 点击“本地 Codex 翻译”，确认翻译语言。先点“预览”，检查尺寸、模式、数量与 JSON。
   预览会调用 Codex 并使用临时文件/媒体缓存，但不写服务器标签或译文。
4. 点“开始”写入当前预览；无预览则先识别。写入成功后刷新页面。“取消”仅关闭空闲对话框；
   运行中禁用关闭和再次执行。运行期间留在当前页面。
5. 用无标签页检查识别，用已有标签页检查翻译。检查作者标签排除、超大图缩放、非法 JSON、
   Codex 非零退出/超时、缺少 CLI，以及服务器在第 N 条写入时返回错误的表现。

## 中途失败恢复

先刷新页面，逐项核对并清点实际落地的标签和译文，与错误中的已确认数量比较。
请求中断时最后一项可能已经保存但未收到确认，因此不能只依赖计数判断是否需要重试。
不要盲目重跑 `label-only`，它会重复创建标签；核对后先人工处理已落地项目和缺失部分，
再决定后续操作。翻译模式也应先检查已有译文，避免重复提交。

## 已知限制

- 单图通常约 35–50 秒，并消耗所配置 Codex 模型的额度/费用。“本地”指调用本机 CLI，
  不代表离线推理；图片和文字会交给 CLI 配置的模型服务。没有可用 Codex 时没有视觉识别回退。
- 图片很大、文字密集或缩小后字太小时可能漏识别；模型输出需要人工检查。
- 写入不是事务。中途失败会保留部分标签/译文，错误显示分别确认成功的数量，阻止本次页面
  直接重试。网络断开时最后一条可能已被服务器保存但未收到确认，请刷新核对。
- 预览之后其他用户仍可能修改页面。本实现不锁定服务器数据，也不自动解决并发编辑。
- 路由支持标准 UUID 对 UUID，或两段不含连字符的 ID；其他 ID 编码需要调整解析规则。
- 当前工作环境没有 Rust 工具链、bun 或 node_modules，仅完成静态检查，没有编译或运行验证。
  优先核对 Tauri IPC 注册、Windows 原生 Codex 启动、API 响应形状、语言解析及已有 source 的译文写入。
