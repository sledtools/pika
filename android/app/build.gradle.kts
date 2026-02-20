plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

val appVersionName = rootProject.file("../VERSION").readText().trim()
val appVersionMatch = Regex("""^(\d+)\.(\d+)\.(\d+)$""").matchEntire(appVersionName)
    ?: throw GradleException("VERSION must be in x.y.z format (found: $appVersionName)")
val (major, minor, patch) = appVersionMatch.destructured
val appVersionCode = major.toInt() * 10000 + minor.toInt() * 100 + patch.toInt()

android {
    namespace = "com.pika.app"
    compileSdk = 35
    ndkVersion = "28.2.13676358"

    defaultConfig {
        applicationId = "org.pikachat.pika"
        minSdk = 26
        targetSdk = 35
        versionCode = appVersionCode
        versionName = appVersionName
        testInstrumentationRunner = "com.pika.app.PikaTestRunner"

        vectorDrawables {
            useSupportLibrary = true
        }
    }

    signingConfigs {
        create("release") {
            storeFile = rootProject.file("pika-release.jks")
            storePassword = System.getenv("PIKA_KEYSTORE_PASSWORD")
            keyAlias = "pika"
            keyPassword = System.getenv("PIKA_KEYSTORE_PASSWORD")
        }
    }

    buildTypes {
        debug {
            // Avoid collisions with App Store / release builds (and signature mismatch failures)
            // when installing debug builds on real devices.
            applicationIdSuffix = ".dev"
            versionNameSuffix = "-dev"
        }
        release {
            isMinifyEnabled = false
            signingConfig = signingConfigs.getByName("release")
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
            )
        }
    }

    buildFeatures {
        compose = true
    }

    composeOptions {
        kotlinCompilerExtensionVersion = "1.5.14"
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }

    packaging {
        resources.excludes.addAll(
            listOf(
                "/META-INF/{AL2.0,LGPL2.1}",
                "META-INF/DEPENDENCIES",
            ),
        )
    }

    sourceSets {
        getByName("main") {
            jniLibs.srcDirs("src/main/jniLibs")
        }
    }
}

tasks.register("ensureUniffiGenerated") {
    doLast {
        val out = file("src/main/java/com/pika/app/rust/pika_core.kt")
        if (!out.exists()) {
            throw GradleException("Missing UniFFI Kotlin bindings. Run `just gen-kotlin` from the repo root.")
        }
    }
}

tasks.named("preBuild") {
    dependsOn("ensureUniffiGenerated")
}

dependencies {
    val composeBom = platform("androidx.compose:compose-bom:2024.06.00")
    implementation(composeBom)
    androidTestImplementation(composeBom)

    implementation("androidx.core:core-ktx:1.13.1")
    implementation("androidx.activity:activity-compose:1.9.0")
    implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.8.3")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.8.3")
    implementation("androidx.security:security-crypto:1.1.0-alpha06")

    implementation("com.google.android.material:material:1.12.0")
    implementation("com.google.zxing:core:3.5.3")

    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.material:material-icons-extended")

    implementation("androidx.camera:camera-camera2:1.4.2")
    implementation("androidx.camera:camera-lifecycle:1.4.2")
    implementation("androidx.camera:camera-view:1.4.2")
    implementation("com.google.mlkit:barcode-scanning:17.3.0")

    androidTestImplementation("androidx.compose.ui:ui-test-junit4")
    // Required for Android 16+ compatibility: Espresso 3.7.0 removes reflective
    // InputManager.getInstance usage (fixes NoSuchMethodException in Espresso.onIdle).
    androidTestImplementation("androidx.test.ext:junit:1.3.0")
    androidTestImplementation("androidx.test:runner:1.7.0")
    androidTestImplementation("androidx.test:rules:1.7.0")
    androidTestImplementation("androidx.test.espresso:espresso-core:3.7.0")

    debugImplementation("androidx.compose.ui:ui-tooling")
    debugImplementation("androidx.compose.ui:ui-test-manifest")

    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.8.1")

    // UniFFI Kotlin bindings default to JNA.
    implementation("net.java.dev.jna:jna:5.18.1@aar")

    // Markdown rendering in Compose
    implementation("com.github.jeziellago:compose-markdown:0.5.4")

    // Async image loading
    implementation("io.coil-kt:coil-compose:2.7.0")
}
