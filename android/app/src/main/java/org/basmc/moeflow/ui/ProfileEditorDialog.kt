package org.basmc.moeflow.ui

import android.content.Context
import android.view.LayoutInflater
import android.view.View
import androidx.appcompat.app.AlertDialog
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.lifecycleScope
import com.google.android.material.dialog.MaterialAlertDialogBuilder
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.basmc.moeflow.R
import org.basmc.moeflow.data.Profile
import org.basmc.moeflow.data.ProfileStore
import org.basmc.moeflow.data.ProbeOutcome
import org.basmc.moeflow.data.ServerProbe
import org.basmc.moeflow.data.UrlTools
import org.basmc.moeflow.databinding.DialogProfileEditorBinding
import java.util.UUID

/**
 * 实例编辑/新增弹窗。
 *
 * 真正要填的只有两项：**名称**和**站点地址**。
 *
 * 这里刻意**没有**"预设服务器"选择，也**没有** API 地址栏。套壳版直接加载站点 origin，
 * API 地址不参与任何流量路由 —— 摆出来只会让人以为需要配（理由见 `Profile` 的类注释）。
 *
 * 「测试连接」保留，但它现在的语义只是"这个地址是不是一台可用的 MoeFlow 服务器"。
 */
object ProfileEditorDialog {

    /**
     * 显示编辑弹窗。
     *
     * @param context 上下文，必须是 LifecycleOwner（用于绑定协程生命周期，避免泄漏）
     * @param existing 编辑模式传入已有实例；null 为新增模式
     * @param onSaved 保存成功后的回调
     */
    fun show(context: Context, existing: Profile?, onSaved: (Profile) -> Unit) {
        require(context is LifecycleOwner) {
            "ProfileEditorDialog 需要 LifecycleOwner 上下文以绑定协程生命周期"
        }

        val binding = DialogProfileEditorBinding.inflate(LayoutInflater.from(context))
        val store = ProfileStore.get(context)

        if (existing != null) {
            binding.editorName.setText(existing.name)
            binding.editorSite.setText(existing.siteUrl)
            binding.editorAllowInvalidCerts.isChecked = existing.allowInvalidCerts
        }

        val dialog = MaterialAlertDialogBuilder(context)
            .setTitle(if (existing == null) R.string.editor_title_add else R.string.editor_title_edit)
            .setView(binding.root)
            // 先设为 null：下面接管点击，通过校验之后才关闭弹窗
            .setPositiveButton(R.string.action_save, null)
            .setNegativeButton(R.string.action_cancel, null)
            .create()

        binding.probeButton.setOnClickListener {
            val siteUrl = binding.editorSite.text?.toString().orEmpty()
            if (siteUrl.isBlank()) {
                binding.editorSiteLayout.error = context.getString(R.string.site_url_required)
                return@setOnClickListener
            }
            binding.editorSiteLayout.error = null

            binding.probeButton.isEnabled = false
            binding.probeButton.text = context.getString(R.string.probe_running)
            binding.probeResult.visibility = View.GONE

            // 绑在宿主 Activity 的 lifecycleScope 上，而不是弹窗自己起线程：
            // 弹窗跟着 Activity 走，Activity 没了协程也一起结束，不会泄漏它的 view。
            context.lifecycleScope.launch {
                val outcome = withContext(Dispatchers.IO) { ServerProbe.probe(siteUrl) }

                binding.probeButton.isEnabled = true
                binding.probeButton.text = context.getString(R.string.probe_button)

                val message: String
                val colorRes: Int
                when (outcome) {
                    ProbeOutcome.Ok -> {
                        message = context.getString(R.string.probe_ok)
                        colorRes = android.R.color.holo_green_dark
                    }
                    is ProbeOutcome.Failed -> {
                        message = context.getString(R.string.probe_failed, outcome.reason)
                        colorRes = android.R.color.holo_orange_dark
                    }
                }
                binding.probeResult.text = message
                binding.probeResult.setTextColor(context.getColor(colorRes))
                binding.probeResult.visibility = View.VISIBLE
            }
        }

        dialog.setOnShowListener {
            dialog.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener {
                binding.editorNameLayout.error = null
                binding.editorSiteLayout.error = null

                val name = binding.editorName.text?.toString()?.trim().orEmpty()
                val siteUrl = binding.editorSite.text?.toString()?.trim().orEmpty()

                if (name.isEmpty()) {
                    binding.editorNameLayout.error = context.getString(R.string.name_required)
                    return@setOnClickListener
                }
                if (siteUrl.isEmpty()) {
                    binding.editorSiteLayout.error = context.getString(R.string.site_url_required)
                    return@setOnClickListener
                }

                // 归一化成 scheme://host[:port]：用户写 `moetran.com`、带结尾斜杠、或带路径都要能存
                val normalized = UrlTools.normalizeSiteUrl(siteUrl)
                if (normalized == null) {
                    binding.editorSiteLayout.error =
                        context.getString(R.string.site_url_invalid, siteUrl)
                    return@setOnClickListener
                }

                // 探测失败**不**拦截保存：服务器临时离线、内网机器不在同一网段，都是
                // "此刻探不到但地址没错"的正常情况，拦下来只会让人存不进去。
                val profile = Profile(
                    id = existing?.id ?: UUID.randomUUID().toString(),
                    name = name,
                    siteUrl = normalized,
                    allowInvalidCerts = binding.editorAllowInvalidCerts.isChecked,
                )
                store.upsert(profile)
                onSaved(profile)
                dialog.dismiss()
            }
        }

        dialog.show()
    }
}
