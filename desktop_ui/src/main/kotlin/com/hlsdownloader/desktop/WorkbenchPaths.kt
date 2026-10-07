package com.hlsdownloader.desktop

import java.nio.file.Files
import java.nio.file.Path

internal object WorkbenchPaths {
    private val localAppData = System.getenv("LOCALAPPDATA")?.takeIf(String::isNotBlank)?.let(Path::of)
        ?: Path.of(System.getProperty("user.home"), "AppData", "Local")
    private val configuredData = System.getenv("HLS_V7_DATA_DIR")?.takeIf(String::isNotBlank)?.let(Path::of)
    private val portableRoot = System.getProperty("compose.application.resources.dir")
        ?.takeIf(String::isNotBlank)?.let(Path::of)?.parent?.parent
        ?.takeIf { Files.isRegularFile(it.resolve("portable")) }

    // 与 Rust Core 的 portable 标记和显式数据目录保持一致，避免锁与日志使用另一个配置。
    val dataDirectory: Path = (configuredData ?: portableRoot?.resolve("data")
        ?: localAppData.resolve("HLS Downloader").resolve("v7")).toAbsolutePath().normalize()
    val uiDirectory: Path = (if (configuredData != null || portableRoot != null) dataDirectory.resolve("ui")
        else localAppData.resolve("HLSDownloader")).toAbsolutePath().normalize()
}
