package org.basmc.moeflow.ui

import android.content.ActivityNotFoundException
import android.content.Intent
import android.os.Bundle
import android.view.View
import android.widget.Toast
import androidx.appcompat.app.AppCompatActivity
import androidx.recyclerview.widget.LinearLayoutManager
import com.google.android.material.dialog.MaterialAlertDialogBuilder
import org.basmc.moeflow.BuildConfig
import org.basmc.moeflow.R
import org.basmc.moeflow.data.Profile
import org.basmc.moeflow.data.ProfileStore
import org.basmc.moeflow.databinding.ActivityLauncherBinding
import org.basmc.moeflow.web.WebActivity

/**
 * 实例选择页（主入口）。
 *
 * 职责：
 * 1. 展示所有实例档案
 * 2. 支持添加/编辑/删除实例
 * 3. 点击实例 → 连接（启动 WebActivity）
 * 4. skipLauncher 开关：启动时直接进入上次使用的实例
 */
class LauncherActivity : AppCompatActivity() {

    private lateinit var binding: ActivityLauncherBinding
    private lateinit var store: ProfileStore
    private lateinit var adapter: ProfileListAdapter

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        binding = ActivityLauncherBinding.inflate(layoutInflater)
        setContentView(binding.root)

        store = ProfileStore.get(this)

        // 设置 Toolbar
        setSupportActionBar(binding.launcherToolbar)

        // skipLauncher 逻辑：开关打开且存在活动实例 → 直接跳转到 WebActivity 并 finish
        // 注意：必须在 startActivity 之后 finish，避免同步 finish 到还未建好的窗口
        if (store.skipLauncher()) {
            val activeProfile = store.activeProfile()
            if (activeProfile != null) {
                startWebActivity(activeProfile)
                finish()
                return
            }
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

        // 版本信息
        binding.versionText.text = getString(R.string.about_version, BuildConfig.VERSION_NAME)

        // Toolbar 菜单
        binding.launcherToolbar.setOnMenuItemClickListener { menuItem ->
            when (menuItem.itemId) {
                R.id.action_settings -> {
                    startActivity(Intent(this, SettingsActivity::class.java))
                    true
                }
                else -> false
            }
        }
    }

    override fun onResume() {
        super.onResume()
        // 从设置页返回时刷新列表（可能有编辑/删除操作）
        refreshList()
    }

    private fun setupRecyclerView() {
        adapter = ProfileListAdapter(
            onRowClick = { profile ->
                // 实例页：点行 = 连接
                connectToProfile(profile)
            },
            onEdit = { profile ->
                showEditorDialog(profile)
            },
            onDelete = { profile ->
                confirmDelete(profile)
            }
        )

        binding.profileList.layoutManager = LinearLayoutManager(this)
        binding.profileList.adapter = adapter
    }

    private fun refreshList() {
        // skipLauncher 打开时 onCreate 会直接跳去 WebActivity 并 finish()，列表根本还没建起来。
        // 但系统仍可能走到 onResume——没有这道判断，那就是一次
        // UninitializedPropertyAccessException，表现为"打开就闪退"。
        if (!::adapter.isInitialized) return

        val profiles = store.profiles()
        val activeId = store.activeId()

        adapter.submit(profiles, activeId)

        // 列表为空时显示空视图（正常不会发生，因为播种）
        if (profiles.isEmpty()) {
            binding.emptyView.visibility = View.VISIBLE
            binding.profileList.visibility = View.GONE
        } else {
            binding.emptyView.visibility = View.GONE
            binding.profileList.visibility = View.VISIBLE
        }
    }

    private fun connectToProfile(profile: Profile) {
        // 设为活动实例
        store.setActive(profile.id)
        // 启动 WebActivity
        startWebActivity(profile)
    }

    private fun startWebActivity(profile: Profile) {
        // 直接引用 WebActivity，不用反射：反射会把编译期错误推迟成运行时才暴露，
        // 而且 release 版走 minify，按类名查找更容易出意外。
        //
        // CLEAR_TOP | SINGLE_TOP：栈里已经有一层 WebActivity 时复用它（由 onNewIntent 换实例），
        // 而不是再叠一层。实例页在它下面，而 CLEAR_TOP 只弹掉目标之上的 Activity，实例页会留下。
        val intent = WebActivity.intent(this, profile.id)
        intent.addFlags(Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP)
        try {
            startActivity(intent)
        } catch (e: ActivityNotFoundException) {
            Toast.makeText(this, R.string.instance_missing, Toast.LENGTH_SHORT).show()
        }
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
