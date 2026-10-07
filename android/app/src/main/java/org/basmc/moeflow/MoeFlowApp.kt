package org.basmc.moeflow

import android.app.Application
import org.basmc.moeflow.data.ProfileStore

/**
 * MoeFlow 应用程序入口。
 *
 * 职责：首次启动时播种默认实例（尨译 MoeTran），确保用户永远有至少一个可连接的实例。
 */
class MoeFlowApp : Application() {
    override fun onCreate() {
        super.onCreate()
        // 只做一件事：播种。不要在这里做任何网络请求或 WebView 初始化。
        ProfileStore.get(this).seedIfEmpty()
    }
}
