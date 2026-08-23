import org.jetbrains.compose.desktop.application.dsl.TargetFormat
plugins {
    kotlin("jvm") version "2.4.10"
    id("org.jetbrains.compose") version "1.11.1"
    id("org.jetbrains.kotlin.plugin.compose") version "2.4.10"
    kotlin("plugin.serialization") version "2.4.10"
}
group = "com.hlsdownloader"
version = "7.0.0"
// jlink consumes an argument file; keep generated paths ASCII-safe on zh-CN Windows.
layout.buildDirectory.set(file("D:/HLSDownloaderBuildCache/compose-build"))
dependencies {
    implementation(compose.desktop.currentOs)
    implementation("org.jetbrains.compose.material:material-icons-extended:1.7.3")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-swing:1.10.2")
    implementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.9.0")
    testImplementation(kotlin("test"))
}
kotlin { jvmToolchain(21) }
tasks.test {
    useJUnitPlatform()
    // Keep benchmark measurements in CI and local verification logs.
    testLogging { showStandardStreams = true }
}
tasks.processResources {
    from("../assets/app-icon.png")
}
compose.desktop { application {
    mainClass = "com.hlsdownloader.desktop.MainKt"
    jvmArgs += listOf(
        "-Dcompose.accessibility.enable=true",
        "-Djavax.accessibility.assistive_technologies=com.sun.java.accessibility.AccessBridge",
    )
    nativeDistributions {
        modules("jdk.accessibility", "jdk.httpserver")
        appResourcesRootDir.set(project.layout.projectDirectory.dir("resources"))
        targetFormats(TargetFormat.Msi, TargetFormat.Exe)
        packageName = "HLSDownloader"
        packageVersion = "7.0.0"
        description = "HLS Downloader 7.0.0"
        vendor = "HLS Downloader"
        windows {
            menuGroup = "HLS Downloader"
            upgradeUuid = "1c80d5f7-a1ec-4bae-a4a6-e010c5a3ee6b"
            iconFile.set(project.file("../assets/app-icon.ico"))
        }
    }
} }
