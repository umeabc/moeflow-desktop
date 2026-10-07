package org.basmc.moeflow.data

/**
 * MoeFlow 服务器实例。
 *
 * 只有一个地址概念：**[siteUrl]**。WebView 直接加载它，页面、资产、XHR、图片全都在这个
 * origin 上发生 —— 这也正是这个客户端不需要任何反向代理的原因。
 *
 * **为什么这里没有 API 地址字段**：桌面端有一个，因为它的环路服务器要把 `/api/` 下的请求反代到
 * 真正的后端，那是"加载打包产物"方案的必然产物。套壳版没有反代，API 地址不参与任何流量路由，
 * 存下来就是一个永远用不到、还让人以为需要配的东西 —— 所以这个版本把它连同它的 UI 一起去掉了。
 *
 * （写这段注释时注意：Kotlin 的块注释可以嵌套，所以在注释里写出一个斜杠加星号的路径通配写法，
 * 会让整个文件变成"注释未闭合"。要表达"这个前缀下的所有路径"，写成 `/api/` 加中文说明即可。）
 *
 * 「测试连接」还在，但它现在的语义只是"这个地址是不是一台可用的 MoeFlow 服务器"，不产出任何
 * 需要保存的地址（见 [ServerProbe]）。
 *
 * @param id 唯一标识符，UUID 或预设固定 ID
 * @param name 用户可见的实例名称
 * @param siteUrl 站点地址，形如 `https://moetran.com`，结尾不带 `/`
 * @param allowInvalidCerts 允许无效 TLS 证书（内网自签名站点）
 */
data class Profile(
    val id: String,
    val name: String,
    val siteUrl: String,
    val allowInvalidCerts: Boolean = false,
) {
    /**
     * WebView 的落点。
     *
     * 为什么不是 `/`：`/` 在 publicPaths 里，未登录也会渲染站点主页（含公告横幅）。
     * 受保护路由才能让守卫自动重定向到 `/login` 或直接进工作台。
     */
    val entryUrl: String
        get() = "$siteUrl/dashboard/projects"

    /**
     * 站点的 host（小写），用于判断跨域跳转与预设匹配。
     */
    val host: String?
        get() = UrlTools.hostOf(siteUrl)
}
