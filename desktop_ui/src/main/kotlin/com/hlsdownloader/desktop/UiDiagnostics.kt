package com.hlsdownloader.desktop

import java.nio.charset.StandardCharsets
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.StandardCopyOption
import java.time.Instant
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

internal object UiDiagnostics {
    private const val MAX_LOG_BYTES = 2L * 1024 * 1024
    private val lock = Any()

    fun error(event: String, error: Throwable, taskId: String = "", requestId: String = "") {
        write("error", event, error.message ?: error::class.java.simpleName, taskId, requestId, error::class.java.name)
    }

    fun warning(event: String, message: String, taskId: String = "", requestId: String = "") {
        write("warning", event, message, taskId, requestId, "")
    }

    private fun write(level: String, event: String, message: String, taskId: String, requestId: String, exception: String) {
        runCatching {
            synchronized(lock) {
                val path = logPath()
                Files.createDirectories(path.parent)
                if (Files.exists(path) && Files.size(path) >= MAX_LOG_BYTES) {
                    Files.move(path, path.resolveSibling("workbench.previous.jsonl"), StandardCopyOption.REPLACE_EXISTING)
                }
                val record = buildJsonObject {
                    put("timestamp", Instant.now().toString())
                    put("level", level)
                    put("component", "compose_workbench")
                    put("event", clean(event, 120))
                    if (taskId.isNotBlank()) put("task_id", clean(taskId, 160))
                    if (requestId.isNotBlank()) put("request_id", clean(requestId, 160))
                    put("message", clean(message, 2_000))
                    if (exception.isNotBlank()) put("exception", clean(exception, 240))
                }
                Files.writeString(
                    path,
                    protocolJson.encodeToString(record) + System.lineSeparator(),
                    StandardCharsets.UTF_8,
                    java.nio.file.StandardOpenOption.CREATE,
                    java.nio.file.StandardOpenOption.APPEND,
                )
            }
        }
    }

    private fun logPath(): Path {
        val configured = System.getenv("HLS_V7_DATA_DIR")?.takeIf(String::isNotBlank)
        val root = configured?.let(Path::of) ?: Path.of(
            System.getenv("LOCALAPPDATA") ?: System.getProperty("user.home"),
            "HLS Downloader",
            "v7",
        )
        return root.toAbsolutePath().normalize().resolve("logs").resolve("workbench.jsonl")
    }

    private fun clean(value: String, limit: Int): String = value
        .replace('\r', ' ')
        .replace('\n', ' ')
        .take(limit)
}
