// 注意：Kotlin DSL 脚本里 `java` 这个名字被脚本作用域占着，写 `java.util.Properties` 会报
// "Unresolved reference: util"，必须显式 import。
import java.util.Properties

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

// ---------- release 签名 ----------
//
// 正式密钥不进仓库：凭据放在 android/keystore.properties（已 gitignore），
// 密钥库文件本身也已 gitignore。模板见 android/keystore.properties.example。
//
// ⚠️ 没有签名配置时 AGP 产出的是 app-release-unsigned.apk，而 **Android 一律拒绝安装未签名的包**
//    （安装器报"安装包无效 / 解析失败"）。所以这里给一条退路：凭据缺失时退回 debug 签名，
//    保证 assembleRelease 永远能出一个可安装的包 —— 但 debug 签名的包不适合分发，
//    而且将来换成正式密钥后同样覆盖安装不上（签名不一致）。
val keystorePropsFile = rootProject.file("keystore.properties")
val hasReleaseKeystore = keystorePropsFile.isFile
val keystoreProps = Properties().apply {
    if (hasReleaseKeystore) {
        keystorePropsFile.inputStream().use { load(it) }
    }
}
if (!hasReleaseKeystore) {
    logger.lifecycle(
        "警告：android/keystore.properties 不存在 —— release 将使用 debug 签名。" +
            "包可以安装，但不适合分发，且将来换正式密钥后无法覆盖安装。"
    )
}

android {
    namespace = "org.basmc.moeflow"
    compileSdk = 35

    signingConfigs {
        if (hasReleaseKeystore) {
            create("release") {
                storeFile = rootProject.file(keystoreProps.getProperty("storeFile"))
                storePassword = keystoreProps.getProperty("storePassword")
                keyAlias = keystoreProps.getProperty("keyAlias")
                keyPassword = keystoreProps.getProperty("keyPassword")
            }
        }
    }

    defaultConfig {
        applicationId = "org.basmc.moeflow"
        minSdk = 26
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro"
            )
            signingConfig = if (hasReleaseKeystore) {
                signingConfigs.getByName("release")
            } else {
                signingConfigs.getByName("debug")
            }
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }

    buildFeatures {
        viewBinding = true
        // AGP 8.x 起 buildConfig 默认关闭，而代码里要用 BuildConfig.VERSION_NAME
        // 拼版本号与 User-Agent。不开这一项会直接是 "Unresolved reference: BuildConfig"。
        buildConfig = true
    }
}

dependencies {
    implementation("androidx.core:core-ktx:1.13.1")
    implementation("androidx.appcompat:appcompat:1.7.0")
    implementation("androidx.activity:activity-ktx:1.9.3")
    implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.8.7")
    implementation("androidx.recyclerview:recyclerview:1.3.2")
    implementation("com.google.android.material:material:1.12.0")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.8.1")
}
