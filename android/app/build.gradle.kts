plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

// Подпись релиза: ключ из переменных окружения (CI); без них — debug-ключ (только для локальных проверок:
// APK с разными ключами нельзя установить поверх друг друга).
val ksFile = System.getenv("ANDROID_KEYSTORE_FILE")

android {
    namespace = "online.point.ispcheck"
    compileSdk = 35

    defaultConfig {
        applicationId = "online.point.ispcheck"
        minSdk = 26
        // 34: на 35+ система принудительно включает edge-to-edge, а UI рассчитан на окно под системными панелями
        targetSdk = 34
        versionCode = (findProperty("versionCode") as String?)?.toInt() ?: 1
        versionName = (findProperty("versionName") as String?) ?: "1.0.0"
        ndk { abiFilters += listOf("arm64-v8a", "armeabi-v7a", "x86_64") }
    }

    signingConfigs {
        if (ksFile != null) {
            create("release") {
                storeFile = file(ksFile)
                storePassword = System.getenv("ANDROID_KS_PASS")
                keyAlias = System.getenv("ANDROID_KEY_ALIAS")
                keyPassword = System.getenv("ANDROID_KEY_PASS")
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false // MainActivity.isp* вызываются из Rust по имени
            signingConfig = signingConfigs.findByName("release") ?: signingConfigs.getByName("debug")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions { jvmTarget = "17" }

    // libispcheck.so кладёт сюда android/build-rust.sh
    sourceSets["main"].jniLibs.srcDir("src/main/jniLibs")
}

dependencies {
    implementation("androidx.appcompat:appcompat:1.7.0")
    implementation("androidx.core:core-ktx:1.13.1")
    // версия, с которой совместим android-activity 0.6 (без prefab)
    implementation("androidx.games:games-activity:4.4.0")
}
