# 补丁

## `0001-desktop-entry.patch`（前端）

**改动上游源码的全部两处**，各自只有一行：

**1. `src/index.tsx` — 挂载桌面叠加层**

```ts
import './desktop/mount';
```

`mount.tsx` 会往 `document.body` 挂一个独立容器，渲染右下角的浮动入口（「实例」+「本地导出」）。它**不依赖上游任何组件结构**——上游改版不会让它失效，它自己出问题也不会影响应用本身（挂载失败时前端照常工作）。在普通浏览器里打开时，检测不到 `window.__TAURI__` 就直接不渲染，等于不存在。

锚点是 `const debugLogger = createDebugLogger('app');`。

**2. `src/components/project/OutputList.tsx` — PS 脚本外链改走本地**

```diff
- href: 'https://files.kozzzx.com/labelplus/LabelPlus_PS-Script_latest.zip',
+ href: '/__ps-script',
```

改由桌面端下载一次并缓存到本地（详见 README「外部依赖本地化」）。在浏览器里这条链接会 404，所以本仓库的产物**只用于桌面客户端**。

overlay 的其余文件（`src/desktop/*`）是**新增文件**，由 `scripts/build-frontend.sh` 直接拷贝，不走 patch。

## `apply-backend-show-blank.py`（后端，可选但强烈建议）

让原文列表接口能返回**空白原文**。

不打的后果：本地导出的 LabelPlus txt 会缺少空白原文对应的行，**导致后续标号编号整体错位**，而且不会报错。客户端会用 `rank` 序列是否连续来检测这种情况并明确告警，但要彻底消除差异需要这个补丁。

```bash
python3 apply-backend-show-blank.py /path/to/moeflow-backend
```

脚本是幂等的，对上游 `backend-v1` 和 iroha 定制分支都适用（两处代码相同）。

### 它改了什么

1. `app/validators/source.py` — `SourceSearchSchema` 增加 `show_blank = fields.Bool(missing=False)`
2. `app/apis/source.py` — `FileSourceListAPI.get` 把 `show_blank` 透传给 `File.to_translator`

### 为什么必须显式传

`File.to_translator` 的默认值是 `show_blank=False`（`app/models/file.py`），会执行 `sources.filter(blank=False)`；而服务器自己的导出走的是 `File.to_labelplus` → `self.sources()`，**没有这个过滤**。两边因此不一致。

空白原文并不罕见：从 LabelPlus txt 导入时，**没有译文的标号就会产生一条空白原文**（`app/models/file.py` 里 `create_source` 的 `blank=True` 分支）。

顺带一提，`DefaultSchema` 设了 `unknown = "EXCLUDE"`，所以未打补丁的服务器收到 `show_blank=true` 只会**静默忽略**，不会报错——客户端的请求参数是向前兼容的。
