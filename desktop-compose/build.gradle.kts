import org.jetbrains.compose.desktop.application.dsl.TargetFormat

plugins {
    kotlin("jvm") version "2.4.0"
    kotlin("plugin.serialization") version "2.4.0"
    id("org.jetbrains.kotlin.plugin.compose") version "2.4.0"
    id("org.jetbrains.compose") version "1.11.1"
}

group = "com.ciaooo55.hlsdownloader"
version = "1.4.0"

kotlin {
    jvmToolchain(21)
}

sourceSets.main {
    resources.srcDir("../assets")
}

dependencies {
    implementation(compose.desktop.currentOs)
    implementation(compose.material3)
    implementation(compose.materialIconsExtended)
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.11.0")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-swing:1.11.0")
    implementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.11.0")
    testImplementation(kotlin("test-junit5"))
    testRuntimeOnly("org.junit.platform:junit-platform-launcher:1.13.4")
}

compose.desktop {
    application {
        mainClass = "com.ciaooo55.hlsdownloader.MainKt"
        nativeDistributions {
            targetFormats(TargetFormat.AppImage)
            packageName = "HLSDownloader"
            packageVersion = project.version.toString()
            description = "Native desktop download manager for HLS, DASH, HTTP and BitTorrent"
            vendor = "HLS Downloader"
            modules("java.net.http", "java.desktop", "jdk.unsupported")
            windows {
                iconFile.set(project.file("../assets/app-icon.ico"))
                console = false
                menu = true
                shortcut = true
            }
        }
    }
}

tasks.test {
    useJUnitPlatform()
}
