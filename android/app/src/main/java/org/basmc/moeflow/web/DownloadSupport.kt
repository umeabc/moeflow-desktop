package org.basmc.moeflow.web

import android.app.Activity
import android.app.DownloadManager
import android.content.Context
import android.net.Uri
import android.os.Build
import android.os.Environment
import android.webkit.CookieManager
import android.webkit.WebView
import android.widget.Toast
import org.basmc.moeflow.R
import org.basmc.moeflow.data.Profile
import org.basmc.moeflow.data.UrlTools

/**
 * http/https 下载。
 *
 * 这条路径不是可选项：**服务器导出出来的成品 zip 就走它**。
 */
object DownloadSupport {

    /**
     * 处理一次下载。返回 true 表示已接手。
     *
     * 非 http/https 的 URL 直接放回 false —— blob/data 由 [BlobBridge] 负责。
     */
    fun handle(
        activity: Activity,
        webView: WebView,
        url: String,
        userAgent: String?,
        contentDisposition: String?,
        mimeType: String?,
        contentLength: Long,
        profile: Profile,
    ): Boolean {
        if (!url.startsWith("http://") && !url.startsWith("https://")) {
            return false
        }

        val fileName = UrlTools.guessFileName(url, contentDisposition, "download")

        try {
            val request = DownloadManager.Request(Uri.parse(url)).apply {
                // Cookie 与 Referer 都必须自己带上：DownloadManager 是一个独立进程，
                // 它既没有 WebView 的 cookie jar，也不会替我们补 Referer。
                // 尨译的 OSS 桶（c01.m-t.pics）设了防盗链且**拒绝空 Referer**，漏掉就是整站图片/资源 403。
                CookieManager.getInstance().getCookie(url)
                    ?.takeIf { it.isNotBlank() }
                    ?.let { addRequestHeader("Cookie", it) }
                addRequestHeader("Referer", profile.siteUrl + "/")

                // 自定义 User-Agent 只能走 header 表：`Request.setUserAgent()` 是 **API 36** 才加的方法，
                // 本工程 compileSdk 是 35，直接调它会 "Unresolved reference"。
                //
                // 这里不会和服务端收到的那条重复：API < 36 上 DownloadManager 自己**不补** User-Agent
                // （请求记录里那个字段只有 setUserAgent 被调用时才写入），所以 header 里这一条就是唯一一条。
                addRequestHeader("User-Agent", userAgent ?: webView.settings.userAgentString)

                setNotificationVisibility(DownloadManager.Request.VISIBILITY_VISIBLE_NOTIFY_COMPLETED)
                setTitle(fileName)

                if (mimeType != null) {
                    setMimeType(mimeType)
                }

                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                    // 分区存储下写公共「下载」目录不需要权限
                    setDestinationInExternalPublicDir(Environment.DIRECTORY_DOWNLOADS, fileName)
                } else {
                    // API < 29 写公共目录要 WRITE_EXTERNAL_STORAGE 运行时授权。
                    // 为一条下载路径引一套权限流程不划算，退到应用专属目录：
                    // 文件仍然下得下来（通知点开就能用），只是不出现在系统「下载」里。
                    setDestinationInExternalFilesDir(
                        activity.applicationContext,
                        Environment.DIRECTORY_DOWNLOADS,
                        fileName,
                    )
                }
            }

            val manager = activity.getSystemService(Context.DOWNLOAD_SERVICE) as DownloadManager
            manager.enqueue(request)

            Toast.makeText(
                activity,
                activity.getString(R.string.download_started, fileName),
                Toast.LENGTH_SHORT,
            ).show()
            return true
        } catch (e: Exception) {
            // 每条失败路径都要有可见输出。静默失败在这个应用里等于"点了没反应"，
            // 而这里的失败原因（非法 URL、存储不可用、下载服务不可用）用户自己能处理。
            Toast.makeText(
                activity,
                activity.getString(R.string.download_failed, e.message ?: e.javaClass.simpleName),
                Toast.LENGTH_LONG,
            ).show()
            return false
        }
    }
}
