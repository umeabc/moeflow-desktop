package org.basmc.moeflow.data

import java.util.Locale

/**
 * 服务器预设配置。
 *
 * 与 `src-tauri/presets/servers.json` 逐字对应。
 */
data class ServerPreset(
    val host: String,
    val name: String,
    /**
     * 该站点的 API 基址。
     *
     * 套壳版**不再把它当作设置项**（界面上已经没有了），但内部还有两处要用：
     * 1. [ServerProbe] 命中预设时直接判定"可用"，省一次网络探测
     * 2. [org.basmc.moeflow.web.MoeFlowWebViewClient] 判断跨 host 导航要不要留在应用内 ——
     *    尨译的 API 在另一个源（`api.moetran.com`），指过去的顶层导航（典型是服务器导出的
     *    成品 zip 链接）如果被丢给系统浏览器，用户在应用里就永远等不到那个下载
     */
    val apiBase: String,
)

/**
 * 预设服务器列表。
 *
 * 这些是已知的公开 MoeFlow 实例。它**不出现在界面上**（预置实例只有尨译一条，其余由用户自行
 * 添加），只是一张"我们知道的部署形态"对照表。
 */
object ServerPresets {
    /**
     * 所有预设实例。
     *
     * 来源：`src-tauri/presets/servers.json`
     */
    val ALL: List<ServerPreset> = listOf(
        ServerPreset("moeflow.basmc.org", "彩翻（basmc）", "https://moeflow.basmc.org/api"),
        ServerPreset("moetran.com", "尨译 MoeTran", "https://api.moetran.com"),
        ServerPreset("demo.moeflow.org", "MoeFlow 演示站", "https://demo.moeflow.org/api"),
    )

    /**
     * 按 host 精确匹配（忽略大小写），命中即无需网络探测。
     *
     * 为什么按 host 匹配：用户可能填 `moetran.com` 或 `https://moetran.com/some/path`，
     * 提取 host 后才能判断是否命中预设。
     */
    fun match(host: String?): ServerPreset? {
        if (host == null) return null
        val normalized = host.lowercase(Locale.ROOT)
        return ALL.find { it.host.lowercase(Locale.ROOT) == normalized }
    }

    /**
     * 首次启动时预置的唯一实例：尨译 MoeTran。
     *
     * 为什么只预置一个：一个默认就列着别人服务器的客户端只会让人连错地方。
     * 其他实例需要用户主动添加。
     */
    fun seedProfile(): Profile {
        return Profile(
            id = "preset-moetran",
            name = "尨译 MoeTran",
            siteUrl = "https://moetran.com",
            allowInvalidCerts = false,
        )
    }
}
