plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

// Подпись релиза: ключ только из секретов CI / переменных окружения, в репозитории ключей нет.
// ANDROID_KEYSTORE_FILE (путь к .jks), ANDROID_KS_PASS, ANDROID_KEY_ALIAS, ANDROID_KEY_PASS.
// Без них (локальная сборка) APK подписывается debug-ключом.
fun env(n: String) = System.getenv(n)?.takeIf { it.isNotEmpty() }
val ksFile = env("ANDROID_KEYSTORE_FILE")

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
                storePassword = env("ANDROID_KS_PASS")
                keyAlias = env("ANDROID_KEY_ALIAS")
                keyPassword = env("ANDROID_KEY_PASS")
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
