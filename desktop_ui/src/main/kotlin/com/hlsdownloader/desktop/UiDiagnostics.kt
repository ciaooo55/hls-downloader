package com.hlsdownloader.desktop

import java.nio.charset.StandardCharsets
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.StandardCopyOption
import java.time.Instant
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.TimeUnit
import kotlin.concurrent.thread
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

// 诊断写入专用后台线程：调用点位于 UI 调度回调，磁盘卡顿时不能阻塞主线程。
// 队列有界，溢出直接丢弃（诊断日志允许有损）。
internal object UiDiagnostics {
    private const val MAX_LOG_BYTES = 2L * 1024 * 1024
    private const val MAX_QUEUED_LINES = 256
    private val queue = ArrayBlockingQueue<String>(MAX_QUEUED_LINES)

    init {
        thread(start = true, isDaemon = true, name = "hls-ui-diagnostics") {
            while (true) {
                val line = runCatching { queue.poll(1, TimeUnit.SECONDS) }.getOrNull()
                if (line != null) runCatching { writeLine(line) }
            }
        }
    }

    fun error(event: String, error: Throwable, taskId: String = "", requestId: String = "") {
        write("error", event, error.message ?: error::class.java.simpleName, taskId, requestId, error::class.java.name)
    }

    fun warning(event: String, message: String, taskId: String = "", requestId: String = "") {
        write("warning", event, message, taskId, requestId, "")
    }

    private fun write(level: String, event: String, message: String, taskId: String, requestId: String, exception: String) {
        runCatching {
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
            queue.offer(protocolJson.encodeToString(record))
        }
    }

    private fun writeLine(line: String) {
        val path = logPath()
        Files.createDirectories(path.parent)
        if (Files.exists(path) && Files.size(path) >= MAX_LOG_BYTES) {
            Files.move(path, path.resolveSibling("workbench.previous.jsonl"), StandardCopyOption.REPLACE_EXISTING)
        }
        Files.writeString(
            path,
            line + System.lineSeparator(),
            StandardCharsets.UTF_8,
            java.nio.file.StandardOpenOption.CREATE,
            java.nio.file.StandardOpenOption.APPEND,
        )
    }

    fun logsDirectory(): Path = logPath().parent

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
