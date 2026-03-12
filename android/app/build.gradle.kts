plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
    alias(libs.plugins.hilt)
    alias(libs.plugins.ksp)
    id("org.openapi.generator") version "7.10.0"
}

android {
    namespace = "dev.phos.android"
    compileSdk = 35

    defaultConfig {
        applicationId = "dev.phos.android"
        minSdk = 26
        targetSdk = 35
        versionCode = 1
        versionName = "1.0.0"

        manifestPlaceholders["appAuthRedirectScheme"] = "dev.phos.android"
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro"
            )
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlin {
        jvmToolchain(17)
    }

    buildFeatures {
        compose = true
    }
}

// OpenAPI Generator configuration
openApiGenerate {
    generatorName.set("java")
    inputSpec.set("${rootProject.projectDir}/openapi.json")
    outputDir.set("${layout.buildDirectory.get().asFile}/generated/openapi")
    apiPackage.set("dev.phos.android.data.remote.api")
    modelPackage.set("dev.phos.android.data.remote.model")
    invokerPackage.set("dev.phos.android.data.remote")
    skipValidateSpec.set(true)
    configOptions.set(mapOf(
        "library" to "retrofit2",
        "useCoroutines" to "true",
        "serializationLibrary" to "jackson",
        "dateLibrary" to "java8",
        "sourceFolder" to "src/main/java",
        "useJakartaEe" to "false",
        "openApiNullable" to "false",
        "documentationProvider" to "none",
        "annotationLibrary" to "none",
    ))
    globalProperties.set(mapOf(
        "models" to "",
        "apis" to "",
    ))
}

// Add generated sources to build
kotlin {
    sourceSets {
        main {
            kotlin.srcDir("${layout.buildDirectory.get().asFile}/generated/openapi/src/main/java")
        }
    }
}

tasks.named("preBuild") {
    dependsOn("openApiGenerate")
}

dependencies {
    // Compose
    implementation(platform(libs.compose.bom))
    implementation(libs.compose.ui)
    implementation(libs.compose.ui.graphics)
    implementation(libs.compose.ui.tooling.preview)
    implementation(libs.compose.material3)
    implementation(libs.compose.foundation)
    implementation(libs.compose.icons.extended)
    implementation(libs.compose.activity)
    debugImplementation(libs.compose.ui.tooling)

    // Lifecycle
    implementation(libs.lifecycle.runtime)
    implementation(libs.lifecycle.viewmodel)

    // Navigation
    implementation(libs.navigation.compose)

    // Hilt
    implementation(libs.hilt.android)
    ksp(libs.hilt.compiler)
    implementation(libs.hilt.navigation.compose)
    implementation(libs.hilt.work)

    // Room
    implementation(libs.room.runtime)
    implementation(libs.room.ktx)
    ksp(libs.room.compiler)

    // Networking
    implementation(libs.okhttp)
    implementation(libs.okhttp.logging)
    implementation(libs.retrofit)
    implementation(libs.retrofit.jackson)
    implementation(libs.retrofit.scalars)
    implementation(libs.jackson.databind)
    implementation(libs.jackson.kotlin)

    // Images
    implementation(libs.coil.compose)
    implementation(libs.coil.network.okhttp)

    // Zoom
    implementation(libs.telephoto.zoomable.coil)

    // Video
    implementation(libs.media3.exoplayer)
    implementation(libs.media3.ui)
    implementation(libs.media3.datasource.okhttp)

    // Auth
    implementation(libs.appauth)
    implementation(libs.security.crypto)

    // WorkManager
    implementation(libs.work.runtime)

    // Core
    implementation(libs.core.ktx)
    implementation(libs.core.splashscreen)
}
