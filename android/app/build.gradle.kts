plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

// Подпись релиза: ключ из переменных окружения (секреты CI) или постоянный android/ispcheck.jks из репозитория.
// Ключ должен быть один и тот же во всех сборках, иначе APK не обновится поверх установленного.
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
        create("release") {
            storeFile = if (ksFile != null) file(ksFile) else rootProject.file("ispcheck.jks")
            storePassword = env("ANDROID_KS_PASS") ?: "ispcheck"
            keyAlias = env("ANDROID_KEY_ALIAS") ?: "ispcheck"
            keyPassword = env("ANDROID_KEY_PASS") ?: "ispcheck"
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false // MainActivity.isp* вызываются из Rust по имени
            signingConfig = signingConfigs.getByName("release")
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
