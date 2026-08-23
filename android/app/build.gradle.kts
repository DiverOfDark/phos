plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.compose)
    alias(libs.plugins.hilt)
    alias(libs.plugins.ksp)
    id("org.openapi.generator") version "7.22.0"
}

android {
    namespace = "dev.phos.android"
    compileSdk = 36

    defaultConfig {
        applicationId = "dev.phos.android"
        minSdk = 26
        targetSdk = 36
        versionCode = (project.findProperty("versionCode") as String?)?.toIntOrNull() ?: 1
        versionName = (project.findProperty("versionName") as String?) ?: "1.0.0"

        manifestPlaceholders["appAuthRedirectScheme"] = "dev.phos.android"
    }

    signingConfigs {
        create("release") {
            val password = System.getenv("KEYSTORE_PASSWORD")
            if (password != null) {
                storeFile = rootProject.file("phos-release.keystore")
                storePassword = password
                keyAlias = "phos"
                keyPassword = password
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro"
            )
            if (System.getenv("KEYSTORE_PASSWORD") != null) {
                signingConfig = signingConfigs.getByName("release")
            }
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildFeatures {
        compose = true
        // AGP 8+ only generates BuildConfig on request. The in-app updater needs
        // BuildConfig.VERSION_CODE to compare against the server's advertised build
        // and BuildConfig.VERSION_NAME to show the user what they are running, so the
        // values set in defaultConfig have to reach app code.
        buildConfig = true
    }

    testOptions {
        unitTests.isReturnDefaultValues = true
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
        // Generate the Retrofit interfaces too, not just the models — the API
        // surface the app uses is no longer small enough to hand-maintain, and a
        // hand-written method that drifts from the spec fails at runtime instead
        // of at compile time.
        //
        // Filtered to the tags this app actually calls, capitalised — the filter
        // matches the generated *class* prefix, not the lowercase tag in the spec,
        // and an unmatched name silently generates nothing. ComfyUI, WebDAV/S3
        // settings and import are web-only; generating them would compile dead
        // code and let an unrelated backend endpoint break the Android build.
        "apis" to "Shots,People,Files,Faces,Client,Auth,System",
        // The generated interfaces `import ...CollectionFormats.*`, so that one
        // supporting file has to come along. Naming it explicitly rather than
        // generating all supporting files, which would drag in an ApiClient,
        // server configuration and OAuth plumbing this app has no use for — it
        // builds its own Retrofit with the app's OkHttp stack. (StringUtil is
        // CollectionFormats' own dependency.)
        "supportingFiles" to "CollectionFormats.java,StringUtil.java",
    ))
}

// Add generated sources to build
android {
    sourceSets {
        getByName("main") {
            java.srcDir("${layout.buildDirectory.get().asFile}/generated/openapi/src/main/java")
        }
    }
}

tasks.named("preBuild") {
    dependsOn("openApiGenerate")
}

tasks.configureEach {
    if (name.startsWith("ksp") && name.endsWith("Kotlin")) {
        dependsOn("openApiGenerate")
    }
    // UpdateRepositoryTest builds a GENERATED model (ClientVersionResponse), so
    // unit-test compilation cannot start before the generator has run. `preBuild`
    // already pulls it in via the app variants; saying so on the unit-test tasks
    // themselves keeps `:app:testDebugUnitTest` correct in a clean tree.
    if (name.startsWith("compile") && name.contains("UnitTest")) {
        dependsOn("openApiGenerate")
    }
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
    implementation(libs.lifecycle.process)

    // Navigation
    implementation(libs.navigation.compose)

    // Hilt
    implementation(libs.hilt.android)
    ksp(libs.hilt.compiler)
    implementation(libs.hilt.navigation.compose)
    implementation(libs.hilt.work)

    // Generated code annotations
    compileOnly("javax.annotation:javax.annotation-api:1.3.2")

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

    // Testing
    testImplementation(libs.junit)
    testImplementation(libs.mockk)
    testImplementation(libs.coroutines.test)
}
