package org.basmc.moeflow.web

import android.content.ActivityNotFoundException
import android.content.Intent
import android.graphics.Bitmap
import android.net.Uri
import android.net.http.SslError
import android.webkit.SslErrorHandler
import android.webkit.WebResourceError
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebView
import android.webkit.WebViewClient
import android.widget.Toast
import org.basmc.moeflow.R
import org.basmc.moeflow.data.Profile
import org.basmc.moeflow.data.ServerPresets
import org.basmc.moeflow.data.UrlTools
import java.util.Locale

/**
 * MoeFlow WebViewClient。
 *
 * 实现契约 §6：
 * - shouldOverrideUrlLoading：同 host 交给 WebView、跨 host 交给浏览器、其它 scheme 交给系统、blob/data 交给 BlobBridge
 * - onReceivedError：主框架才处理，按错误码归类
 * - onReceivedHttpError：排除 401/403
 * - onReceivedSslError：allowInvalidCerts 为 true 时 proceed 并提示
 */
class MoeFlowWebViewClient(
    private val activity: WebActivity,
    profile: Profile,
    private val onPageStarted: (String) -> Unit,
    private val onPageFinished: (String) -> Unit,
    private val onError: (ErrorKind, String?, Int) -> Unit,
    private val onSslToastNeeded: () -> Unit,
) : WebViewClient() {

    enum class ErrorKind { OFFLINE, HTTP, SSL, GENERIC }

    /**
     * 当前实例。
     *
     * 必须是可变的：WebActivity 切换实例时只做 `onNewIntent`，不会重建这个 client。
     * 如果把它固定成构造参数，切换实例后跨 host 判定与证书开关都还会拿旧实例的配置。
     */
    @Volatile
    private var profile: Profile = profile

    fun updateProfile(profile: Profile) {
        this.profile = profile
    }

    /**
     * 视为「应用内」的 host：站点自身，以及它已知的 API 主机。
     *
     * 为什么要把 API 主机也算进来：尨译的 API 在另一个源（api.moetran.com）。如果这里不认它，
     * 任何指向 API 主机的顶层导航 —— 典型的就是服务器导出完成后的成品 zip 链接 ——
     * 都会被丢给系统浏览器，用户在应用里就永远等不到那个下载。
     *
     * API 主机是从预设表推出来的，而不是存在实例里：套壳版没有"API 地址"这个设置项
     * （它不参与流量路由），但"已知部署形态"这张表内部还在用。
     *
     * 判定本身是「导航目标」级别的事，不涉及子资源：图片、XHR 都不会走到这里。
     */
    private fun isInternalHost(host: String?): Boolean {
        val target = host?.lowercase(Locale.ROOT) ?: return false
        val siteHost = profile.host?.lowercase(Locale.ROOT) ?: return false
        if (target == siteHost) return true

        // UrlTools.hostOf 已经返回小写
        val presetApiHost = ServerPresets.match(siteHost)
            ?.let { UrlTools.hostOf(it.apiBase) }
            ?: return false
        return target == presetApiHost
    }

    override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest): Boolean {
        val url = request.url.toString()
        val scheme = request.url.scheme?.lowercase(Locale.ROOT)

        // 1. http/https：应用内的 host 交给 WebView，其余交给系统浏览器
        if (scheme == "http" || scheme == "https") {
            if (isInternalHost(UrlTools.hostOf(url))) {
                // 同 host，WebView 自己导航
                return false
            } else {
                // 跨 host，交给系统浏览器
                try {
                    val intent = Intent(Intent.ACTION_VIEW, Uri.parse(url))
                    activity.startActivity(intent)
                } catch (e: ActivityNotFoundException) {
                    Toast.makeText(activity, R.string.external_open_failed, Toast.LENGTH_SHORT).show()
                }
                return true
            }
        }

        // 2. blob / data：交给 BlobBridge
        if (scheme == "blob" || scheme == "data") {
            activity.getBlobBridge().requestBlob(view, url, "download")
            return true
        }

        // 3. 其它 scheme：交给系统
        try {
            val action = if (scheme == "tel") Intent.ACTION_DIAL else Intent.ACTION_VIEW
            val intent = Intent(action, Uri.parse(url))
            activity.startActivity(intent)
        } catch (e: ActivityNotFoundException) {
            Toast.makeText(activity, R.string.external_open_failed, Toast.LENGTH_SHORT).show()
        }
        return true
    }

    override fun onPageStarted(view: WebView, url: String, favicon: Bitmap?) {
        super.onPageStarted(view, url, favicon)
        onPageStarted(url)
    }

    override fun onPageFinished(view: WebView, url: String) {
        super.onPageFinished(view, url)
        onPageFinished(url)
    }

    override fun onReceivedError(
        view: WebView,
        request: WebResourceRequest,
        error: WebResourceError
    ) {
        super.onReceivedError(view, request, error)

        // 只处理主框架
        if (!request.isForMainFrame) return

        val kind = when (error.errorCode) {
            ERROR_HOST_LOOKUP, ERROR_CONNECT, ERROR_TIMEOUT -> ErrorKind.OFFLINE
            else -> ErrorKind.GENERIC
        }

        onError(kind, error.description?.toString(), 0)
    }

    override fun onReceivedHttpError(
        view: WebView,
        request: WebResourceRequest,
        errorResponse: WebResourceResponse
    ) {
        super.onReceivedHttpError(view, request, errorResponse)

        // 只处理主框架
        if (!request.isForMainFrame) return

        val statusCode = errorResponse.statusCode

        // 排除 401/403：那是前端自己的鉴权流程，不能弹错误页把登录界面盖掉
        if (statusCode >= 400 && statusCode != 401 && statusCode != 403) {
            onError(ErrorKind.HTTP, null, statusCode)
        }
    }

    @android.annotation.SuppressLint("WebViewClientOnReceivedSslError")
    override fun onReceivedSslError(view: WebView, handler: SslErrorHandler, error: SslError) {
        if (profile.allowInvalidCerts) {
            // 用户在实例设置里主动开的开关，仅用于内网自签名站点
            handler.proceed()
            onSslToastNeeded()
        } else {
            handler.cancel()
            onError(ErrorKind.SSL, error.toString(), 0)
        }
    }
}
