package org.basmc.moeflow.ui

import android.content.Intent
import android.os.Bundle
import android.view.View
import android.widget.Toast
import androidx.appcompat.app.AppCompatActivity
import androidx.recyclerview.widget.LinearLayoutManager
import com.google.android.material.dialog.MaterialAlertDialogBuilder
import org.basmc.moeflow.R
import org.basmc.moeflow.data.Profile
import org.basmc.moeflow.data.ProfileStore
import org.basmc.moeflow.databinding.ActivitySettingsBinding

/**
 * 设置页。
 *
 * 与 LauncherActivity 的差异：
 * - 点击实例行 → 编辑（而非连接）
 * - 包含关于文本
 * - Toolbar 带返回箭头
 */
class SettingsActivity : AppCompatActivity() {

    private lateinit var binding: ActivitySettingsBinding
    private lateinit var store: ProfileStore
    private lateinit var adapter: ProfileListAdapter

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        binding = ActivitySettingsBinding.inflate(layoutInflater)
        setContentView(binding.root)

        store = ProfileStore.get(this)

        // 设置 Toolbar
        setSupportActionBar(binding.settingsToolbar)
        supportActionBar?.setDisplayHomeAsUpEnabled(true)

        // Toolbar 返回按钮
        binding.settingsToolbar.setNavigationOnClickListener {
            finish()
        }

        // 初始化列表
        setupRecyclerView()

        // 刷新列表
        refreshList()

        // skipLauncher 开关
        binding.skipLauncherSwitch.isChecked = store.skipLauncher()
        binding.skipLauncherSwitch.setOnCheckedChangeListener { _, isChecked ->
            store.setSkipLauncher(isChecked)
        }

        // 添加实例按钮
        binding.addProfileButton.setOnClickListener {
            showEditorDialog(null)
        }

        // 关于文本：拼接多段说明
        binding.aboutText.text = buildString {
            append(getString(R.string.about_shell_note))
            append("\n\n")
            append(getString(R.string.about_privacy))
        }
    }

    private fun setupRecyclerView() {
        adapter = ProfileListAdapter(
            onRowClick = { profile ->
                // 设置页：点行 = 编辑
                showEditorDialog(profile)
            },
            onEdit = { profile ->
                showEditorDialog(profile)
            },
            onDelete = { profile ->
                confirmDelete(profile)
            }
        )

        binding.settingsProfileList.layoutManager = LinearLayoutManager(this)
        binding.settingsProfileList.adapter = adapter
    }

    private fun refreshList() {
        val profiles = store.profiles()
        val activeId = store.activeId()
        adapter.submit(profiles, activeId)
    }

    private fun showEditorDialog(existing: Profile?) {
        ProfileEditorDialog.show(this, existing) { profile ->
            // 保存成功后刷新列表
            refreshList()
        }
    }

    private fun confirmDelete(profile: Profile) {
        val profiles = store.profiles()

        // 只剩一个实例时禁止删除（契约 §5）
        if (profiles.size <= 1) {
            Toast.makeText(this, R.string.last_instance_cannot_be_deleted, Toast.LENGTH_LONG).show()
            return
        }

        // 弹出确认对话框
        MaterialAlertDialogBuilder(this)
            .setTitle(R.string.delete_confirm_title)
            .setMessage(getString(R.string.delete_confirm_message, profile.name))
            .setPositiveButton(R.string.action_delete) { _, _ ->
                val success = store.delete(profile.id)
                if (success) {
                    Toast.makeText(
                        this,
                        getString(R.string.deleted, profile.name),
                        Toast.LENGTH_SHORT
                    ).show()
                    refreshList()
                }
            }
            .setNegativeButton(R.string.action_cancel, null)
            .show()
    }
}
