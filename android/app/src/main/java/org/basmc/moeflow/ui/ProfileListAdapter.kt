package org.basmc.moeflow.ui

import android.view.LayoutInflater
import android.view.View
import android.view.ViewGroup
import androidx.recyclerview.widget.RecyclerView
import org.basmc.moeflow.data.Profile
import org.basmc.moeflow.databinding.ItemProfileBinding

/**
 * 实例档案列表适配器。
 *
 * 可被 LauncherActivity 和 SettingsActivity 复用：
 * - 实例页：点行 = 连接（onRowClick）
 * - 设置页：点行 = 编辑（onEdit）
 */
class ProfileListAdapter(
    private val onRowClick: (Profile) -> Unit,
    private val onEdit: (Profile) -> Unit,
    private val onDelete: (Profile) -> Unit,
) : RecyclerView.Adapter<ProfileListAdapter.Holder>() {

    private var profiles: List<Profile> = emptyList()
    private var activeId: String? = null

    /**
     * 提交新数据并刷新列表。
     */
    fun submit(profiles: List<Profile>, activeId: String?) {
        this.profiles = profiles
        this.activeId = activeId
        notifyDataSetChanged()
    }

    override fun onCreateViewHolder(parent: ViewGroup, viewType: Int): Holder {
        val binding = ItemProfileBinding.inflate(
            LayoutInflater.from(parent.context),
            parent,
            false
        )
        return Holder(binding)
    }

    override fun onBindViewHolder(holder: Holder, position: Int) {
        holder.bind(profiles[position])
    }

    override fun getItemCount(): Int = profiles.size

    inner class Holder(private val binding: ItemProfileBinding) :
        RecyclerView.ViewHolder(binding.root) {

        fun bind(profile: Profile) {
            binding.profileName.text = profile.name
            binding.profileSiteUrl.text = profile.siteUrl

            // 活动徽章：当前活动实例才显示
            if (profile.id == activeId) {
                binding.activeBadge.visibility = View.VISIBLE
            } else {
                binding.activeBadge.visibility = View.GONE
            }

            // 点击整行 → 调用 onRowClick（实例页 = 连接，设置页通过传不同回调实现编辑）
            binding.root.setOnClickListener {
                onRowClick(profile)
            }

            // 编辑按钮
            binding.editButton.setOnClickListener {
                onEdit(profile)
            }

            // 删除按钮
            binding.deleteButton.setOnClickListener {
                onDelete(profile)
            }
        }
    }
}
