package org.basmc.moeflow.data

import android.net.Uri
import java.util.Locale

/**
 * URL 处理工具。
 */
object UrlTools {
    /**
     * 补全 scheme（缺省 https://）、去掉结尾 / 与多余空白；失败返回 null。
     *
     * 规则：
     * - trim()；空 → null
     * - 无 `://` 前缀 → 前面补 `https://`
     * - 只接受 `http` / `https` scheme，其它 → null
     * - 解析出的 host 为空 → null
     * - 去掉 path/query/fragment，只保留 `scheme://host[:port]`，并去掉结尾 `/`
     *
     * 例：
     * - `" moetran.com/ "` → `https://moetran.com`
     * - `"http://172.29.133.24"` → `http://172.29.133.24`
     * - `"ftp://example.com"` → null
     */
    fun normalizeSiteUrl(input: String): String? {
        val trimmed = input.trim()
        if (trimmed.isEmpty()) return null

        val withScheme = if ("://" !in trimmed) {
            "https://$trimmed"
        } else {
            trimmed
        }

        val uri = try {
            Uri.parse(withScheme)
        } catch (e: Exception) {
            return null
        }

        val scheme = uri.scheme?.lowercase(Locale.ROOT)
        if (scheme != "http" && scheme != "https") return null

        val host = uri.host
        if (host.isNullOrEmpty()) return null

        val port = uri.port
        val portPart = if (port != -1) ":$port" else ""

        return "$scheme://$host$portPart"
    }

    /**
     * 取 host（小写），失败返回 null。
     */
    fun hostOf(url: String): String? {
        val uri = try {
            Uri.parse(url)
        } catch (e: Exception) {
            return null
        }
        return uri.host?.lowercase(Locale.ROOT)
    }

    /**
     * 从 content-disposition 或 URL 末段猜一个安全的文件名；都失败时返回 fallback。
     *
     * 必须剥离路径分隔符与控制字符，并限制长度 <= 120。
     *
     * 为什么需要清洗：防止路径穿越攻击（`../../etc/passwd`）以及文件系统不支持的字符。
     */
    fun guessFileName(url: String, contentDisposition: String?, fallback: String): String {
        var name: String? = null

        // 优先从 Content-Disposition 提取
        if (!contentDisposition.isNullOrBlank()) {
            val filenamePattern = """filename\s*=\s*"?([^";\r\n]+)"?""".toRegex(RegexOption.IGNORE_CASE)
            val match = filenamePattern.find(contentDisposition)
            if (match != null) {
                name = match.groupValues[1].trim()
            }
        }

        // 回退到 URL 末段
        if (name.isNullOrBlank()) {
            try {
                val uri = Uri.parse(url)
                name = uri.lastPathSegment
            } catch (e: Exception) {
                // ignore
            }
        }

        // 都失败时用 fallback
        if (name.isNullOrBlank()) {
            name = fallback
        }

        // 清洗：去掉路径分隔符、控制字符
        name = name
            .replace(Regex("[/\\\\]"), "_")
            .replace(Regex("[\\x00-\\x1F\\x7F]"), "")
            .trim()

        // 限制长度
        if (name.length > 120) {
            val ext = name.substringAfterLast('.', "")
            val base = name.substringBeforeLast('.', name)
            name = if (ext.isNotEmpty() && ext.length < 10) {
                base.take(120 - ext.length - 1) + "." + ext
            } else {
                name.take(120)
            }
        }

        return name.ifBlank { fallback }
    }
}
