# 本地 Codex 翻译：本机验收记录（2026-11-10）

环境：Windows 10/11 x64，Rust 1.99.1（MSVC，rustup minimal），MSVC 14.44 + Windows SDK 10.0.26100，
Node 24.22.1，WebView2 155–157。分支 `codex-local`（基于 `1673718`）。

## 已通过

| 项 | 命令 / 方式 | 结果 |
| --- | --- | --- |
| Rust 编译 | `cargo check --all-targets` | 通过，2m14s（含 500+ 依赖首次编译） |
| Rust 单元测试 | `cargo test --lib` | **113 passed / 0 failed / 1 ignored**，17s |
| Codex 端到端（真 CLI） | `--ignored codex::tests::runs_codex_and_returns_schema_shaped_json` | 通过，57s，返回 schema 形状 JSON |
| 并发保护 | `codex::tests::rejects_a_second_concurrent_run` | 通过，第二次调用立刻返回「已有任务在跑」 |
| 前端构建 | `scripts/build-frontend.sh`（含 overlay 拷入 + `vite build`） | 通过，4424 modules，15–20s |
| 产物包含特性 | 在运行中的应用里取 `/assets/index-*.js` | 命中「本地 Codex 翻译」 |
| 类型检查 | `tsc --strict`（overlay 4 个文件） | 0 error |
| IPC 权限编译 | 读 `target/debug/build/…/out/acl-manifests.json` | `allow-codex-run → codex_run`、`allow-codex-probe → codex_probe` 均在 `default` 权限集内 |
| 应用启动（debug / release） | 直接运行 exe | 窗口渲染实例选择页；外壳端口与三个档位端口均监听；`/moeflow-runtime-config.json` 返回 `{"baseURL":"/api/"}` |
| 打包 | `npm run build` | NSIS 安装包 6.80 MiB，5m31s |

## 未验证（需要登录态）

1. 图片翻译页上的实际交互：按钮出现 → 预览 → 开始 → 写入 → 刷新。
2. 写入是否落在正确的 source / target 上（代码路径已用假 fetch 做过 12 项断言，但未打真实后端）。
3. 语言自动解析：依赖 `GET /v1/files/{id}` 返回 `project_id`，当前后端 `File.to_api()` 不返回该字段，
   因此实际会退化为「手填」。补一行 `"project_id": str(self.project.id),` 即可自动。
4. 大图（> 2000 px 长边）缩放后坐标是否仍对齐画布。

## 复现命令

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cd moeflow-desktop
bash scripts/fetch-frontend.sh && bash scripts/build-frontend.sh
cargo test --manifest-path src-tauri/Cargo.toml --lib      # 见下方注意事项
npm run build                                              # 产出 NSIS 安装包
```

### 注意：本机 `scripts/run-rust-tests.mjs` 的 sidecar 方案不生效

测试 harness 需要 Common Controls v6 清单，脚本的做法是写 `<exe>.manifest` 旁挂文件。本机实测
仍以 `0xc0000139 STATUS_ENTRYPOINT_NOT_FOUND` 退出；把清单**嵌入**才对：

```bash
MT="C:/Program Files (x86)/Windows Kits/10/bin/10.0.22621.0/x64/mt.exe"
"$MT" -manifest /tmp/m.manifest "-outputresource:moeflow_desktop_lib-<hash>.exe;#1"
```

原因应是带内嵌清单的 exe 会忽略旁挂清单，而 tauri-build 给测试 harness 也加了资源。是否要把脚本改成
`mt.exe` 嵌入（或给 harness 加 `-C link-arg=/MANIFEST:EMBED`），待定。

## 手工验收清单（登录后）

1. 打开 `/image-translator/<fileID>-<targetID>`，dock 出现「本地 Codex 翻译」，未登录/非该页时是灰的。
2. 点「预览」：应显示 `label-only` 或 `translate-only`、尺寸、待写条数；不产生任何请求写入。
3. 点「开始」：进度 `写入 n/N`，结束后页面自动刷新；核对标号位置与译文。
4. 无标号的页面：确认新建标号的 rank 从 1 递增、框贴合文字。
5. 破坏性用例：开始后立刻切换文件，应提示「页面已切换，停止后续写入」。
6. 失败恢复：中途失败会显示已写入条数；**不要**直接重跑 `label-only`（会重复建标号）。

## 追加：`program not found` 的根因与修复（0.1.2）

现象：0.1.1 安装版启动后 dock 显示「Codex 不可用：program not found」。

根因：Codex 桌面版不把自己的 CLI 放进 PATH（本机实际在
`C:\Users\Administrator\Codex\_internal\app\resources\codex.exe`），而 GUI 进程也拿不到用户在终端里
改过的 PATH，所以「直接 spawn `codex`」必然失败——0.1.1 只试了 `CODEX_BIN` 和 PATH。

修复：四级解析（设置里保存的路径 → `CODEX_BIN` → 常见安装位置 → 裸 `codex`），设置页新增
「本地 Codex」区块（路径输入框 + 一键锁定 + 保存并检测），`codex_status` 返回配置值/实际解析/版本/候选。
本机实测 `status("")` → `C:\Users\Administrator\Codex/_internal/app/resources/codex.exe`，
版本 `codex-cli 0.163.0-alpha.11.1`（`codex::tests::detects_and_probes_the_local_install`）。
dock 现在在窗口重新获得焦点时重试探测，改完设置无需重启。

## 真机联调：双前端探测（2026-11-11，测试机已更新后端）

| 实例 | `GET /v1/site/flavor` | 客户端实际服务的 JS | 判定 |
| --- | --- | --- | --- |
| caifan → `https://moeflow.basmc.org/api` | `200 {"flavor":"irohamod","name":"BASMC-Moeflow","version":"iroha16-fix11"}` | `index-CMwqSJTT.js`，sha256 `289d45a5…` | **irohamod 包**（与 `frontend-irohamod/build` 逐字节一致） |
| 尨译 → `https://api.moetran.com` | `404` | `index-Btls65hu.js`，sha256 `99e77639…` | **原版包**（与 `frontend/build` 逐字节一致） |

两个实例同时运行在 14653 / 47100 端口，同一进程内各自服务不同前端，说明选择是按实例而不是全局的。
点开 caifan 后登录页正常渲染（irohamod 站点品牌与布局），dock 三按钮在位，非图片页时「本地 Codex 翻译」按预期置灰。

未覆盖：登录后的图片翻译页交互（需要站点账号），以及 irohamod 后端在真实数据下的 `/v1/site/flavor` 失败路径。
