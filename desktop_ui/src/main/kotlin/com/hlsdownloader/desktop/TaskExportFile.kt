package com.hlsdownloader.desktop

import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.StandardCopyOption

internal fun writeTaskExport(path: Path, data: String) {
    val target = path.toAbsolutePath()
    Files.createDirectories(target.parent)
    val staged = Files.createTempFile(target.parent, ".hls-export-", ".tmp")
    try {
        Files.writeString(staged, data, Charsets.UTF_8)
        // 同目录原子替换；文件系统不支持时明确失败，保留原文件。
        Files.move(staged, target, StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING)
    } finally {
        Files.deleteIfExists(staged)
    }
}
