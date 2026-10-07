package org.basmc.moeflow.web

import android.annotation.SuppressLint
import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.view.View
import android.webkit.CookieManager
import android.webkit.WebSettings
import android.webkit.WebView
import android.widget.PopupMenu
import android.widget.Toast
import androidx.activity.OnBackPressedCallback
import androidx.activity.result.ActivityResultLauncher
import androidx.activity.result.contract.ActivityResultContracts
import androidx.appcompat.app.AppCompatActivity
import com.google.android.material.dialog.MaterialAlertDialogBuilder
import org.basmc.moeflow.BuildConfig
import org.basmc.moeflow.R
import org.basmc.moeflow.data.Profile
import org.basmc.moeflow.data.ProfileStore
import org.basmc.moeflow.databinding.ActivityWebBinding
import org.basmc.moeflow.ui.LauncherActivity

/**
 * WebView 宿主 Activity。
 *
 * 实现契约 §6：直接加载站点 URL，处理下载、文件选择、导航、错误页。
 */
class WebActivity : AppCompatActivity() {

    companion object {
        const val EXTRA_PROFILE_ID = "org.basmc.moeflow.extra.PROFILE_ID"
        private const val TAG = "WebActivity"

        fun intent(context: Context, profileId: String): Intent {
            return Intent(context, WebActivity::class.java).apply {
                putExtra(EXTRA_PROFILE_ID, profileId)
            }
        }
    }

    private lateinit var binding: ActivityWebBinding
    private lateinit var profile: Profile
    private lateinit var blobBridge: BlobBridge
    private lateinit var webViewClient: MoeFlowWebViewClient
    private var sslToastShown = false

    /**
     * 当前页面地址，只在 UI 线程写入。
     *
     * 存在的唯一原因：`@JavascriptInterface` 方法跑在 WebView 自己的线程上，而 `WebView.getUrl()`
     * 必须在 UI 线程读。BlobBridge 的 host 护栏要用它，所以不能靠在 JS 线程里 `runOnUiThread` 去取 ——
     * 那是异步投递，下一行读到的永远是 null，结果就是 blob 下载被静默丢弃。
     */
    @Volatile
    internal var currentUrl: String? = null
        private set

    // ActivityResultLauncher 必须在 onCreate 之前注册
    private val fileChooserLauncher: ActivityResultLauncher<Intent> =
        registerForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
            fileChooserCallback?.onReceiveValue(
                MoeFlowWebChromeClient.parseFileChooserResult(result.resultCode, result.data)
            )
            fileChooserCallback = null
        }

    private var fileChooserCallback: android.webkit.ValueCallback<Array<Uri>>? = null

    @SuppressLint("SetJavaScriptEnabled")
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        binding = ActivityWebBinding.inflate(layoutInflater)
        setContentView(binding.root)

        // 读取 profile
        val profileId = intent.getStringExtra(EXTRA_PROFILE_ID)
        val store = ProfileStore.get(this)
        profile = store.profiles().find { it.id == profileId }
            ?: run {
                Toast.makeText(this, R.string.instance_missing, Toast.LENGTH_SHORT).show()
                finish()
                return
            }

        setupWebView()
        setupUI()
        setupBackPress()

        // 旋转等配置变更已被 configChanges 吃掉，不会重建；这里只为进程被回收后的重建兜底。
        // restoreState 返回 null 表示没有可恢复的历史，那就按正常流程落到入口页。
        val restored = savedInstanceState?.let { binding.webView.restoreState(it) }
        if (restored == null) {
            binding.webView.loadUrl(profile.entryUrl)
        }
    }

    @SuppressLint("SetJavaScriptEnabled")
    private fun setupWebView() {
        val webView = binding.webView

        // WebView 设置
        webView.settings.apply {
            javaScriptEnabled = true
            // domStorageEnabled 覆盖 localStorage / sessionStorage，前端的会话态要用。
            // 不要再设 `databaseEnabled` —— 它对应的是早已废弃的 WebSQL，AGP 会报弃用警告，
            // 而现代 WebView 里它没有任何效果。
            domStorageEnabled = true
            loadsImagesAutomatically = true
            setSupportMultipleWindows(false)
            javaScriptCanOpenWindowsAutomatically = false
            mediaPlaybackRequiresUserGesture = true
            mixedContentMode = WebSettings.MIXED_CONTENT_COMPATIBILITY_MODE
            userAgentString = "$userAgentString MoeFlowAndroid/${BuildConfig.VERSION_NAME}"
            setSupportZoom(false)
            builtInZoomControls = false
            displayZoomControls = false
        }

        // Cookie 设置
        // 不做实例间的 cookie 隔离：Cookie 天然按 host 隔离，而套壳版每个实例是各自的真实域名，
        // 所以多实例共用一个 cookie jar 也不会串号。
        // （桌面端需要按端口隔离，恰恰是因为它把所有实例都反代到了同一个 127.0.0.1。）
        CookieManager.getInstance().apply {
            setAcceptCookie(true)
            setAcceptThirdPartyCookies(webView, true)
        }

        // WebViewClient
        webViewClient = MoeFlowWebViewClient(
            activity = this,
            profile = profile,
            onPageStarted = { url ->
                currentUrl = url
                binding.webProgress.visibility = View.VISIBLE
                binding.errorView.visibility = View.GONE
            },
            onPageFinished = { url ->
                // 这里的 currentUrl 只服务于 BlobBridge 的 host 护栏，而站内 SPA 跳转是同 host 的，
                // 所以整页加载时更新一次就够，不需要再挂 doUpdateVisitedHistory。
                currentUrl = url
                binding.webProgress.visibility = View.GONE
            },
            onError = { kind, description, statusCode ->
                showError(kind, description, statusCode)
            },
            onSslToastNeeded = {
                if (!sslToastShown) {
                    Toast.makeText(this, R.string.allow_invalid_certs_toast, Toast.LENGTH_LONG).show()
                    sslToastShown = true
                }
            }
        )

        // WebChromeClient
        webView.webChromeClient = MoeFlowWebChromeClient(
            activity = this,
            onProgress = { progress ->
                if (progress < 100) {
                    binding.webProgress.visibility = View.VISIBLE
                    binding.webProgress.setProgressCompat(progress, true)
                } else {
                    binding.webProgress.visibility = View.GONE
                }
            },
            fileChooserLauncher = fileChooserLauncher,
            fileChooserCallbackSetter = { callback ->
                fileChooserCallback = callback
            }
        )

        // DownloadListener
        webView.setDownloadListener { url, userAgent, contentDisposition, mimeType, contentLength ->
            DownloadSupport.handle(
                activity = this,
                webView = webView,
                url = url,
                userAgent = userAgent,
                contentDisposition = contentDisposition,
                mimeType = mimeType,
                contentLength = contentLength,
                profile = profile
            )
        }

        // BlobBridge
        blobBridge = BlobBridge(this, profile)
        blobBridge.installOn(webView)
    }

    private fun setupUI() {
        // FAB 单击 → 切换实例
        binding.instanceButton.setOnClickListener {
            val intent = Intent(this, LauncherActivity::class.java).apply {
                flags = Intent.FLAG_ACTIVITY_REORDER_TO_FRONT
            }
            startActivity(intent)
        }

        // FAB 长按 → 弹出菜单
        binding.instanceButton.setOnLongClickListener {
            showFabMenu()
            true
        }

        // 错误页重试按钮
        binding.retryButton.setOnClickListener {
            binding.errorView.visibility = View.GONE
            binding.webView.reload()
        }
    }

    private fun showFabMenu() {
        val popup = PopupMenu(this, binding.instanceButton)
        popup.menuInflater.inflate(R.menu.menu_web_fab, popup.menu)
        popup.setOnMenuItemClickListener { item ->
            when (item.itemId) {
                R.id.action_refresh -> {
                    binding.webView.reload()
                    true
                }
                R.id.action_switch_instance -> {
                    val intent = Intent(this, LauncherActivity::class.java).apply {
                        flags = Intent.FLAG_ACTIVITY_REORDER_TO_FRONT
                    }
                    startActivity(intent)
                    true
                }
                R.id.action_open_in_browser -> {
                    val currentUrl = binding.webView.url
                    if (currentUrl != null) {
                        try {
                            val intent = Intent(Intent.ACTION_VIEW, Uri.parse(currentUrl))
                            startActivity(intent)
                        } catch (e: ActivityNotFoundException) {
                            Toast.makeText(this, R.string.external_open_failed, Toast.LENGTH_SHORT).show()
                        }
                    }
                    true
                }
                R.id.action_about -> {
                    showAboutDialog()
                    true
                }
                else -> false
            }
        }
        popup.show()
    }

    private fun showAboutDialog() {
        val message = getString(R.string.about_version, BuildConfig.VERSION_NAME) + "\n\n" +
                getString(R.string.about_shell_note) + "\n\n" +
                getString(R.string.about_export_hint)

        MaterialAlertDialogBuilder(this)
            .setTitle(R.string.about_title)
            .setMessage(message)
            .setPositiveButton(android.R.string.ok, null)
            .show()
    }

    private fun setupBackPress() {
        onBackPressedDispatcher.addCallback(this, object : OnBackPressedCallback(true) {
            override fun handleOnBackPressed() {
                if (binding.webView.canGoBack()) {
                    binding.webView.goBack()
                } else {
                    finish()
                }
            }
        })
    }

    private fun showError(kind: MoeFlowWebViewClient.ErrorKind, description: String?, statusCode: Int) {
        binding.errorView.visibility = View.VISIBLE
        binding.errorView.bringToFront()

        val message = when (kind) {
            MoeFlowWebViewClient.ErrorKind.OFFLINE -> {
                getString(R.string.error_offline, profile.siteUrl)
            }
            MoeFlowWebViewClient.ErrorKind.HTTP -> {
                getString(R.string.error_http, statusCode)
            }
            MoeFlowWebViewClient.ErrorKind.SSL -> {
                getString(R.string.error_ssl)
            }
            MoeFlowWebViewClient.ErrorKind.GENERIC -> {
                getString(R.string.error_generic, description ?: "Unknown error")
            }
        }

        binding.errorText.text = message
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        val newProfileId = intent.getStringExtra(EXTRA_PROFILE_ID) ?: return
        val store = ProfileStore.get(this)
        val newProfile = store.profiles().find { it.id == newProfileId } ?: return

        if (newProfile.id != profile.id) {
            profile = newProfile
            sslToastShown = false
            // client 与 bridge 都是 stateful 的：只更新它们持有的实例，不要重建。
            // 尤其不能再来一次 addJavascriptInterface —— 同名接口重复注册会被 WebView 忽略并打警告。
            webViewClient.updateProfile(newProfile)
            blobBridge.updateProfile(newProfile)
            binding.webView.loadUrl(profile.entryUrl)
        }
    }

    override fun onPause() {
        super.onPause()
        // 必须 flush：否则进程被回收时会话 cookie 直接丢，用户下次打开发现被登出
        CookieManager.getInstance().flush()
    }

    override fun onSaveInstanceState(outState: Bundle) {
        super.onSaveInstanceState(outState)
        binding.webView.saveState(outState)
    }

    override fun onDestroy() {
        // 文件选择器还挂着的话，一定要先把它的回调结掉：不回调的话，
        // 页面上那个 <input type=file> 会永久停在"等待选择"的状态，连重新点都不响应。
        fileChooserCallback?.onReceiveValue(null)
        fileChooserCallback = null

        // 防泄漏：摘下 WebView、清空并销毁
        val parent = binding.webView.parent as? android.view.ViewGroup
        parent?.removeView(binding.webView)
        binding.webView.loadUrl("about:blank")
        binding.webView.destroy()
        currentUrl = null

        super.onDestroy()
    }

    // BlobBridge 需要访问 WebView 和 profile
    fun getBlobBridge(): BlobBridge = blobBridge
}
