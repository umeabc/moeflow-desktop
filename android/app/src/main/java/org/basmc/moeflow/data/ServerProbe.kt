package org.basmc.moeflow.data

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.IOException
import java.net.HttpURLConnection
import java.net.SocketTimeoutException
import java.net.URL
import java.net.UnknownHostException
import javax.net.ssl.SSLException

/**
 * 站点探测结果。
 *
 * 注意这里**不带任何地址**：套壳版不需要知道 API 地址。这个探测只回答一个问题 ——
 * "用户填的这个站点地址，是不是一台真的能用的 MoeFlow 服务器"。
 */
sealed interface ProbeOutcome {
    /** 站点可达，且 `/ping` 返回了字面量 `pong`。 */
    object Ok : ProbeOutcome

    /**
     * 探测失败。
     *
     * @param reason 给用户看的一句话原因（如 `连接超时`）
     */
    data class Failed(val reason: String) : ProbeOutcome
}

/**
 * 站点探测。
 *
 * **为什么需要它**：套壳版唯一要用户填的东西就是站点地址，而填错的表现在界面上是白屏或落到
 * 别的站点上，用户看不出原因。这个探测给一个明确的"是不是 MoeFlow 服务器"的答复。
 *
 * **为什么判据必须是 `/ping` 返回字面量 `pong`**：所有部署形态的 nginx 都写着
 * `try_files $uri /index.html`，任何未知路径都会回 200 + SPA 外壳。实测
 * `moetran.com/storage/xxx.png` 返回 `200 text/html`。用状态码判断会把 SPA fallback
 * 误认为"服务器可用"，之后所有调用都失败。
 *
 * 探测过程仍然要推导候选基址（`<site>/api` → `<site>` → `https://api.<host>`），但推导出来的
 * 东西只活在函数内部，既不保存也不出现在界面上。
 */
object ServerProbe {
    private const val CONNECT_TIMEOUT_MS = 5000
    private const val READ_TIMEOUT_MS = 5000

    /**
     * 按候选顺序探测。允许 5s 连接 / 5s 读取超时。
     *
     * 所有网络异常都转成 [ProbeOutcome.Failed]，**不会**抛给调用方。
     */
    suspend fun probe(siteUrl: String): ProbeOutcome = withContext(Dispatchers.IO) {
        val normalized = UrlTools.normalizeSiteUrl(siteUrl)
            ?: return@withContext ProbeOutcome.Failed("站点地址无法解析")

        val host = UrlTools.hostOf(normalized)

        // 命中预设就不发网络请求：这些 host 的部署形态是已知的，
        // 省一次往返，也少一个"服务器暂时没响应"的误报面。
        if (ServerPresets.match(host) != null) {
            return@withContext ProbeOutcome.Ok
        }

        var lastReason = "未知错误"
        for (candidate in buildCandidates(normalized, host)) {
            when (val result = probePing(candidate)) {
                is PingResult.Success -> return@withContext ProbeOutcome.Ok
                is PingResult.Failure -> lastReason = result.reason
            }
        }

        ProbeOutcome.Failed(lastReason)
    }

    /**
     * 构造探测候选列表，并去重。
     *
     * 顺序：`<site>/api`、`<site>`、`https://api.<host>`
     *
     * 为什么这个顺序：
     * - `<site>/api` 是 nginx 部署的标准配置（`proxy_pass` 的尾斜杠把 `/api` 剥掉）
     * - `<site>` 可能是根路径直接挂 API（少见但存在）
     * - `https://api.<host>` 是尨译的部署形态（API 在子域名上）
     */
    private fun buildCandidates(normalized: String, host: String?): List<String> {
        val candidates = mutableListOf<String>()
        candidates.add("$normalized/api")
        candidates.add(normalized)

        // 子域名形态只对 https 有意义，且 host 本身不以 api. 开头
        if (host != null && normalized.startsWith("https://") && !host.startsWith("api.")) {
            candidates.add("https://api.$host")
        }

        return candidates.distinct()
    }

    /**
     * 探测单个候选：`GET <candidate>/ping`，响应体 `trim()` 后必须等于 `pong`。
     *
     * **不能**用 HTTP 状态码判断，理由见类注释。
     */
    private fun probePing(candidate: String): PingResult {
        val url = "${candidate.trimEnd('/')}/ping"

        return try {
            val connection = URL(url).openConnection() as HttpURLConnection
            connection.requestMethod = "GET"
            connection.connectTimeout = CONNECT_TIMEOUT_MS
            connection.readTimeout = READ_TIMEOUT_MS
            connection.instanceFollowRedirects = true
            // 证书错误会如实抛出 SSLException，这里不要忽略证书
            // （忽略证书是 WebView 层 allowInvalidCerts 的事，不该由探测替用户决定）

            try {
                connection.connect()
                val code = connection.responseCode

                if (code !in 200..299) {
                    return PingResult.Failure("HTTP $code")
                }

                val trimmed = connection.inputStream.bufferedReader().use { it.readText() }.trim()

                if (trimmed == "pong") {
                    PingResult.Success
                } else {
                    PingResult.Failure("该地址不是 MoeFlow 服务器（/ping 没有返回 pong）")
                }
            } finally {
                connection.disconnect()
            }
        } catch (e: SSLException) {
            PingResult.Failure("证书错误：${e.message ?: "SSL 异常"}")
        } catch (e: UnknownHostException) {
            PingResult.Failure("域名无法解析：${e.message ?: "未知主机"}")
        } catch (e: SocketTimeoutException) {
            PingResult.Failure("连接超时")
        } catch (e: IOException) {
            PingResult.Failure("连接失败：${e.message ?: "IO 异常"}")
        } catch (e: Exception) {
            PingResult.Failure("探测失败：${e.message ?: e.javaClass.simpleName}")
        }
    }

    private sealed interface PingResult {
        object Success : PingResult
        data class Failure(val reason: String) : PingResult
    }
}
