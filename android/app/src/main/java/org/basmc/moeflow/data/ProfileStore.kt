package org.basmc.moeflow.data

import android.content.Context
import android.content.SharedPreferences
import org.json.JSONArray
import org.json.JSONException
import org.json.JSONObject

/**
 * 实例档案的持久化存储。
 *
 * 用 SharedPreferences + org.json（Android 内置），不引入 Gson/Moshi。
 *
 * 键名（snake_case，与桌面端一致）：
 * - `profiles`：JSON 数组字符串
 * - `active_id`：当前活动实例的 ID
 * - `skip_launcher`：启动时是否跳过实例选择页
 *
 * 每个 profile 序列化为 JSON 对象，键名：
 * - `id` / `name` / `site_url` / `allow_invalid_certs`
 *
 * 早期版本还写过 `api_base` / `preset_name`。套壳版不需要 API 地址，这两个键已经去掉；
 * 读的时候忽略它们即可，所以**旧数据不需要迁移** —— 下次编辑保存时它们自然消失。
 *
 * **抗脏数据**：单条解析失败就跳过该条；整份 JSON 解析失败返回空列表（靠 seedIfEmpty 兜底）。
 * 任何情况下不能崩。
 */
class ProfileStore private constructor(private val prefs: SharedPreferences) {

    companion object {
        private const val PREFS_NAME = "moeflow"
        private const val KEY_PROFILES = "profiles"
        private const val KEY_ACTIVE_ID = "active_id"
        private const val KEY_SKIP_LAUNCHER = "skip_launcher"

        @Volatile
        private var instance: ProfileStore? = null

        /**
         * 单例入口。
         *
         * 为什么要单例：避免多个 Activity 各自创建 ProfileStore，导致内存状态不一致。
         */
        fun get(context: Context): ProfileStore {
            return instance ?: synchronized(this) {
                instance ?: ProfileStore(
                    context.applicationContext.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
                ).also { instance = it }
            }
        }
    }

    /**
     * 获取所有实例档案。
     *
     * 解析失败返回空列表（不崩溃），由 [seedIfEmpty] 兜底。
     */
    @Synchronized
    fun profiles(): List<Profile> {
        val json = prefs.getString(KEY_PROFILES, null) ?: return emptyList()
        return parseProfiles(json)
    }

    /**
     * 获取当前活动实例的 ID。
     */
    @Synchronized
    fun activeId(): String? {
        return prefs.getString(KEY_ACTIVE_ID, null)
    }

    /**
     * 获取当前活动实例。
     *
     * 如果 activeId 对应的实例不存在，返回列表第一个。
     */
    @Synchronized
    fun activeProfile(): Profile? {
        val id = activeId()
        val list = profiles()
        return if (id != null) {
            list.find { it.id == id } ?: list.firstOrNull()
        } else {
            list.firstOrNull()
        }
    }

    /**
     * 获取「启动时跳过实例选择页」开关状态。
     */
    @Synchronized
    fun skipLauncher(): Boolean {
        return prefs.getBoolean(KEY_SKIP_LAUNCHER, false)
    }

    /**
     * 不存在则追加，存在则按 id 覆盖。
     *
     * 返回写入后的完整列表。
     */
    @Synchronized
    fun upsert(profile: Profile): List<Profile> {
        val list = profiles().toMutableList()
        val index = list.indexOfFirst { it.id == profile.id }
        if (index >= 0) {
            list[index] = profile
        } else {
            list.add(profile)
        }
        saveProfiles(list)
        return list.toList()
    }

    /**
     * 删除实例。
     *
     * **只剩一个时拒绝**（返回 false），调用方负责提示。
     * 删掉的是当前活动的实例时，activeId 回退到列表第一个。
     *
     * 为什么必须保留一个：只剩下一个连不上任何地方的窗口是糟糕的用户体验。
     */
    @Synchronized
    fun delete(id: String): Boolean {
        val list = profiles().toMutableList()
        if (list.size <= 1) {
            return false // 拒绝删除最后一个
        }

        val removed = list.removeAll { it.id == id }
        if (!removed) {
            return false // ID 不存在
        }

        saveProfiles(list)

        // 如果删的是当前活动实例，回退到第一个
        if (activeId() == id && list.isNotEmpty()) {
            setActive(list.first().id)
        }

        return true
    }

    /**
     * 设置当前活动实例。
     *
     * id 不存在则不改变并返回 false。
     */
    @Synchronized
    fun setActive(id: String): Boolean {
        val list = profiles()
        if (list.none { it.id == id }) {
            return false
        }
        prefs.edit().putString(KEY_ACTIVE_ID, id).apply()
        return true
    }

    /**
     * 设置「启动时跳过实例选择页」开关。
     */
    @Synchronized
    fun setSkipLauncher(value: Boolean) {
        prefs.edit().putBoolean(KEY_SKIP_LAUNCHER, value).apply()
    }

    /**
     * 首次启动播种：列表为空时写入 [ServerPresets.seedProfile] 并设为活动。
     *
     * 返回是否播种过。
     */
    @Synchronized
    fun seedIfEmpty(): Boolean {
        if (profiles().isNotEmpty()) {
            return false
        }

        val seed = ServerPresets.seedProfile()
        saveProfiles(listOf(seed))
        setActive(seed.id)
        return true
    }

    // ========== 内部序列化 ==========

    /**
     * 解析 JSON 数组为 Profile 列表。
     *
     * 单条解析失败就跳过；整份解析失败返回空列表。任何情况下不崩溃。
     */
    private fun parseProfiles(json: String): List<Profile> {
        return try {
            val array = JSONArray(json)
            val result = mutableListOf<Profile>()
            for (i in 0 until array.length()) {
                try {
                    val obj = array.getJSONObject(i)
                    val profile = Profile(
                        id = obj.getString("id"),
                        name = obj.getString("name"),
                        siteUrl = obj.getString("site_url"),
                        allowInvalidCerts = obj.optBoolean("allow_invalid_certs", false),
                    )
                    result.add(profile)
                } catch (e: JSONException) {
                    // 单条解析失败，跳过该条
                    android.util.Log.w("ProfileStore", "跳过无法解析的 profile 条目：${e.message}")
                }
            }
            result
        } catch (e: JSONException) {
            // 整份解析失败，返回空列表
            android.util.Log.e("ProfileStore", "无法解析 profiles JSON，返回空列表：${e.message}")
            emptyList()
        }
    }

    /**
     * 保存 Profile 列表为 JSON 数组。
     */
    private fun saveProfiles(list: List<Profile>) {
        val array = JSONArray()
        for (profile in list) {
            val obj = JSONObject()
            obj.put("id", profile.id)
            obj.put("name", profile.name)
            obj.put("site_url", profile.siteUrl)
            obj.put("allow_invalid_certs", profile.allowInvalidCerts)
            array.put(obj)
        }
        prefs.edit().putString(KEY_PROFILES, array.toString()).apply()
    }
}
