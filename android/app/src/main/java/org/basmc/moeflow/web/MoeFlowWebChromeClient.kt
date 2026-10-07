package org.basmc.moeflow.web

import android.content.Intent
import android.net.Uri
import android.webkit.MimeTypeMap
import android.webkit.PermissionRequest
import android.webkit.ValueCallback
import android.webkit.WebChromeClient
import android.webkit.WebView
import android.widget.Toast
import androidx.activity.result.ActivityResultLauncher
import org.basmc.moeflow.R
import java.util.Locale

/**
 * MoeFlow WebChromeClient。
 *
 * 实现契约 §6：
 * - onProgressChanged：进度条显示/隐藏
 * - onShowFileChooser：文件选择，必须同时接住单选与多选
 * - onPermissionRequest：摄像头/麦克风一律拒绝
 * - onCreateWindow：不允许弹新窗口
 */
class MoeFlowWebChromeClient(
    private val activity: WebActivity,
    private val onProgress: (Int) -> Unit,
    private val fileChooserLauncher: ActivityResultLauncher<Intent>,
    private val fileChooserCallbackSetter: (ValueCallback<Array<Uri>>?) -> Unit,
) : WebChromeClient() {

    companion object {
        /**
         * 解析文件选择结果，同时接住单选与多选。
         */
        fun parseFileChooserResult(resultCode: Int, data: Intent?): Array<Uri>? {
            if (resultCode != android.app.Activity.RESULT_OK || data == null) {
                return null
            }

            val uriList = mutableListOf<Uri>()

            // 单选：data.data
            data.data?.let { uriList.add(it) }

            // 多选：data.clipData
            data.clipData?.let { clipData ->
                for (i in 0 until clipData.itemCount) {
                    clipData.getItemAt(i)?.uri?.let { uriList.add(it) }
                }
            }

            return if (uriList.isNotEmpty()) uriList.toTypedArray() else null
        }

        /**
         * 归一化 MIME 类型。
         *
         * `ACTION_GET_CONTENT` 的 `type` 只接受**一个** MIME，所以多值一律退到 `* / *`。
         * 另外 HTML 规范允许 `<input accept>` 写扩展名（`.png`），而扩展名直接当 MIME 用会让
         * 系统文件选择器筛不出任何东西 —— 这里用 MimeTypeMap 转一道。
         */
        private fun normalizeMimeType(acceptTypes: Array<String>?): String {
            if (acceptTypes.isNullOrEmpty() || acceptTypes.size > 1) return "*/*"
            val type = acceptTypes[0].trim()
            if (type.isBlank()) return "*/*"
            if (type.startsWith(".")) {
                return MimeTypeMap.getSingleton()
                    .getMimeTypeFromExtension(type.substring(1).lowercase(Locale.ROOT))
                    ?: "*/*"
            }
            return type
        }
    }

    override fun onProgressChanged(view: WebView, newProgress: Int) {
        super.onProgressChanged(view, newProgress)
        onProgress(newProgress)
    }

    override fun onShowFileChooser(
        webView: WebView,
        filePathCallback: ValueCallback<Array<Uri>>,
        fileChooserParams: FileChooserParams
    ): Boolean {
        // 取消之前的回调
        fileChooserCallbackSetter(null)

        // 设置新回调
        fileChooserCallbackSetter(filePathCallback)

        // 构造 intent
        val intent = Intent(Intent.ACTION_GET_CONTENT).apply {
            type = normalizeMimeType(fileChooserParams.acceptTypes)
            addCategory(Intent.CATEGORY_OPENABLE)
            putExtra(Intent.EXTRA_ALLOW_MULTIPLE, fileChooserParams.mode == FileChooserParams.MODE_OPEN_MULTIPLE)
        }

        try {
            fileChooserLauncher.launch(intent)
        } catch (e: Exception) {
            fileChooserCallbackSetter(null)
            Toast.makeText(activity, R.string.external_open_failed, Toast.LENGTH_SHORT).show()
            return false
        }

        return true
    }

    override fun onPermissionRequest(request: PermissionRequest) {
        // 当前版本的已知限制：摄像头/麦克风未启用
        request.deny()
        Toast.makeText(activity, R.string.permission_denied, Toast.LENGTH_SHORT).show()
    }

    override fun onCreateWindow(
        view: WebView,
        isDialog: Boolean,
        isUserGesture: Boolean,
        resultMsg: android.os.Message?
    ): Boolean {
        // 不允许弹新窗口
        return false
    }
}
