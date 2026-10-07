package org.basmc.moeflow.web

import android.content.ContentValues
import android.os.Build
import android.os.Environment
import android.provider.MediaStore
import android.util.Base64
import android.util.Log
import android.webkit.JavascriptInterface
import android.webkit.WebView
import android.widget.Toast
import androidx.annotation.RequiresApi
import org.json.JSONObject
import org.basmc.moeflow.R
import org.basmc.moeflow.data.Profile
import org.basmc.moeflow.data.UrlTools
import java.io.File
import java.util.Locale

/**
 * Blob / data URL 下载桥接。
 *
 * 为什么需要它：`DownloadListener` 只对 http/https 有效，而页面上有相当一部分下载是
 * 前端自己 Blob 出来的（内存里生成的 zip/文本）。这类 URL 交给 `DownloadManager` 只会得到
 * 一个 "unsupported scheme" 静默失败。做法是在页面上下文里把内容读成 base64 再回传落盘。
 */
class BlobBridge(
    private val activity: WebActivity,
    initialProfile: Profile,
) {

    companion object {
        private const val TAG = "BlobBridge"

        /** 接口名与方法名是页面侧脚本约定的一部分，不要改。 */
        private const val JS_INTERFACE_NAME = "MoeFlowBridge"

        /**
         * base64 字符串长度上限。
         *
         * 这条路径要把整个文件当作一个 Java 字符串跨 JNI 传一次，超大字符串会直接 OOM。
         * 8 MB 的 base64 约合 6 MB 原文，作为"套壳浏览器"的兜底够用了。
         */
        private const val MAX_BASE64_LENGTH = 8 * 1024 * 1024
    }

    /**
     * 当前实例。
     *
     * 与 `MoeFlowWebViewClient` 同理：切换实例时只更新，不要重建 —— 重建意味着再来一次
     * `addJavascriptInterface`，而同名接口重复注册会被 WebView 忽略并打警告。
     */
    @Volatile
    private var profile: Profile = initialProfile

    fun updateProfile(profile: Profile) {
        this.profile = profile
    }

    fun installOn(webView: WebView) {
        webView.addJavascriptInterface(this, JS_INTERFACE_NAME)
    }

    @JavascriptInterface
    fun saveBlob(name: String, mime: String, base64: String) {
        // 本方法跑在 WebView 的私有线程上，不是 UI 线程。
        //
        // host 护栏读的是 WebActivity 在 UI 线程维护的 currentUrl，而不是 `WebView.getUrl()`：
        // 后者必须在 UI 线程读，而这里能用的 `runOnUiThread` 是**异步投递**的 ——
        // 用它"取"一个值，下一行读到的一定是 null，护栏就变成了"永远拒绝"。
        //
        // 局限：`addJavascriptInterface` 拿不到调用方 frame 的 origin，所以这只是近似护栏，
        // 挡不住同页 XSS。它能挡的是"页面被导航走之后残留的接口调用"。
        val currentHost = UrlTools.hostOf(activity.currentUrl ?: "")?.lowercase(Locale.ROOT)
        val expectedHost = profile.host?.lowercase(Locale.ROOT)
        if (currentHost != expectedHost) {
            Log.w(TAG, "saveBlob 来自非同源页面：$currentHost（期望 $expectedHost），已丢弃")
            return
        }

        if (base64.length > MAX_BASE64_LENGTH) {
            Log.w(TAG, "saveBlob 内容过大：${base64.length} 字符")
            toast(R.string.download_too_large, long = true)
            return
        }

        val data = try {
            // FileReader.readAsDataURL 产出的是 `data:<mime>;base64,<payload>`，前缀必须剥掉
            Base64.decode(base64.substringAfter(',', base64), Base64.DEFAULT)
        } catch (e: Exception) {
            Log.e(TAG, "base64 解码失败", e)
            toast(R.string.download_failed, arg = "base64 解码失败")
            return
        }

        val fileName = UrlTools.guessFileName("", null, name.ifBlank { "download" })

        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                saveViaMediaStore(fileName, mime, data)
            } else {
                saveViaAppDir(fileName, data)
            }
            toast(R.string.download_started, arg = fileName)
        } catch (e: Exception) {
            Log.e(TAG, "保存 blob 失败", e)
            toast(R.string.download_failed, arg = e.message ?: "未知错误")
        }
    }

    /** API 29+：写进公共「下载」目录。分区存储下不需要任何权限。 */
    @RequiresApi(Build.VERSION_CODES.Q)
    private fun saveViaMediaStore(fileName: String, mime: String, data: ByteArray) {
        val values = ContentValues().apply {
            put(MediaStore.MediaColumns.DISPLAY_NAME, fileName)
            put(MediaStore.MediaColumns.MIME_TYPE, mime.ifBlank { "application/octet-stream" })
            put(MediaStore.MediaColumns.RELATIVE_PATH, Environment.DIRECTORY_DOWNLOADS)
            // 两段式写入：写完再把 IS_PENDING 归零，否则别的应用会读到半个文件
            put(MediaStore.MediaColumns.IS_PENDING, 1)
        }

        val resolver = activity.contentResolver
        val uri = resolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, values)
            ?: throw IllegalStateException("MediaStore 没有返回 uri")

        try {
            resolver.openOutputStream(uri)?.use { it.write(data) }
                ?: throw IllegalStateException("打不开输出流")
        } catch (e: Exception) {
            // 别留下一个 0 字节的 pending 条目
            resolver.delete(uri, null, null)
            throw e
        }

        values.clear()
        values.put(MediaStore.MediaColumns.IS_PENDING, 0)
        resolver.update(uri, values, null, null)
    }

    /**
     * API 26-28 的退路。
     *
     * `MediaStore.Downloads` 这个类本身是 API 29 才加的，低版本上引用它会 `NoSuchFieldError`。
     * 而写公共目录又要 `WRITE_EXTERNAL_STORAGE` 的运行时授权 —— 为一条兜底下载路径去引一套
     * 权限流程不划算，所以退到应用专属目录：文件实实在在落盘了，只是不出现在系统「下载」里。
     */
    private fun saveViaAppDir(fileName: String, data: ByteArray) {
        val dir = activity.getExternalFilesDir(Environment.DIRECTORY_DOWNLOADS)
            ?: throw IllegalStateException("外部存储不可用")
        if (!dir.isDirectory && !dir.mkdirs()) {
            throw IllegalStateException("无法创建下载目录")
        }
        val target = File(dir, fileName)
        target.writeBytes(data)
        Log.i(TAG, "blob 已保存到 ${target.absolutePath}（API < 29 只能落到应用专属目录）")
    }

    /**
     * 在页面上下文里把 blob 读成 base64 并回传。
     *
     * 两处插值都过 `JSONObject.quote`：blob URL 和文件名都是外部输入，
     * 直接拼进单引号字符串里，一个 `'` 就能改掉整段脚本。
     */
    fun requestBlob(webView: WebView, blobUrl: String, suggestedName: String) {
        val script = """
            (function() {
                fetch(${JSONObject.quote(blobUrl)})
                    .then(function(r) { return r.blob(); })
                    .then(function(b) {
                        var fr = new FileReader();
                        fr.onload = function() {
                            window.$JS_INTERFACE_NAME.saveBlob(
                                ${JSONObject.quote(suggestedName)},
                                b.type,
                                String(fr.result)
                            );
                        };
                        fr.readAsDataURL(b);
                    })
                    .catch(function(e) { console.error('MoeFlow blob bridge failed:', e); });
            })();
        """.trimIndent()

        webView.evaluateJavascript(script, null)
    }

    private fun toast(resId: Int, arg: String? = null, long: Boolean = false) {
        activity.runOnUiThread {
            val text = if (arg == null) activity.getString(resId) else activity.getString(resId, arg)
            Toast.makeText(activity, text, if (long) Toast.LENGTH_LONG else Toast.LENGTH_SHORT).show()
        }
    }
}
