package com.ciaooo55.hlsdownloader

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

@Serializable
data class Health(val status: String = "", val version: String = "")

@Serializable
data class TaskItem(
    val id: String,
    @SerialName("task_type") val taskType: String = "auto",
    @SerialName("source_page_url") val sourcePageUrl: String = "",
    @SerialName("mime_type") val mimeType: String = "",
    val title: String = "",
    val filename: String = "",
    @SerialName("download_dir") val downloadDir: String = "",
    val url: String = "",
    val status: String = "queued",
    val stage: String = "",
    @SerialName("last_log") val lastLog: String = "",
    @SerialName("total_segments") val totalSegments: Int = 0,
    @SerialName("completed_segments") val completedSegments: Int = 0,
    @SerialName("failed_segments") val failedSegments: Int = 0,
    @SerialName("downloaded_bytes") val downloadedBytes: Long = 0,
    @SerialName("total_bytes") val totalBytes: Long = 0,
    @SerialName("speed_bytes_per_sec") val speedBytesPerSec: Double = 0.0,
    @SerialName("eta_seconds") val etaSeconds: Double = 0.0,
    @SerialName("active_workers") val activeWorkers: Int = 0,
    @SerialName("max_workers") val maxWorkers: Int = 0,
    @SerialName("active_slots") val activeSlots: Int = 0,
    val concurrency: Int = 0,
    @SerialName("reconnect_count") val reconnectCount: Int = 0,
    @SerialName("connection_status") val connectionStatus: String = "idle",
    @SerialName("post_percent") val postPercent: Double = 0.0,
    @SerialName("error_message") val errorMessage: String = "",
    @SerialName("error_code") val errorCode: String = "",
    @SerialName("error_stage") val errorStage: String = "",
    @SerialName("error_url") val errorUrl: String = "",
    @SerialName("error_hint") val errorHint: String = "",
    @SerialName("http_status") val httpStatus: Int = 0,
    @SerialName("error_attempt") val errorAttempt: Int = 0,
    @SerialName("output_path") val outputPath: String = "",
    @SerialName("output_is_file") val outputIsFile: Boolean = false,
    @SerialName("created_at") val createdAt: String = "",
    @SerialName("updated_at") val updatedAt: String = "",
    @SerialName("started_at") val startedAt: String = "",
    @SerialName("finished_at") val finishedAt: String = "",
    @SerialName("available_actions") val availableActions: List<String> = emptyList(),
    @SerialName("queue_position") val queuePosition: Int = 0,
    @SerialName("playable_segments") val playableSegments: Int = 0,
    @SerialName("playable_duration") val playableDuration: Double = 0.0,
    @SerialName("media_duration") val mediaDuration: Double = 0.0,
    @SerialName("playback_ready") val playbackReady: Boolean = false,
    @SerialName("progress_percent") val progressPercent: Double = 0.0,
    @SerialName("uploaded_bytes") val uploadedBytes: Long = 0,
    @SerialName("upload_speed_bytes_per_sec") val uploadSpeedBytesPerSec: Double = 0.0,
    @SerialName("peer_count") val peerCount: Int = 0,
    @SerialName("seed_count") val seedCount: Int = 0,
) {
    val displayName: String get() = filename.ifBlank { title.ifBlank { url.substringAfterLast('/').substringBefore('?').ifBlank { id } } }
    val progress: Float get() = (progressPercent.coerceIn(0.0, 100.0) / 100.0).toFloat()
}

@Serializable
data class AppSettings(
    val host: String = "127.0.0.1",
    val port: Int = 8765,
    val token: String = "55555",
    @SerialName("download_dir") val downloadDir: String = "",
    @SerialName("temp_dir") val tempDir: String = "",
    @SerialName("default_concurrency") val defaultConcurrency: Int = 12,
    @SerialName("max_concurrent_tasks") val maxConcurrentTasks: Int = 3,
    @SerialName("default_user_agent") val defaultUserAgent: String = "",
    @SerialName("default_referer") val defaultReferer: String = "",
    @SerialName("default_origin") val defaultOrigin: String = "",
    @SerialName("default_cookie") val defaultCookie: String = "",
    @SerialName("ffmpeg_path") val ffmpegPath: String = "",
    @SerialName("allowed_hosts") val allowedHosts: List<String> = emptyList(),
    @SerialName("keep_temp_files") val keepTempFiles: Boolean = false,
    @SerialName("http_chunk_size_mb") val httpChunkSizeMb: Int = 8,
    @SerialName("bt_upload_limit_kib") val btUploadLimitKib: Int = 1024,
    @SerialName("bt_max_connections") val btMaxConnections: Int = 80,
    @SerialName("bt_enable_dht") val btEnableDht: Boolean = true,
    @SerialName("browser_takeover_enabled") val browserTakeoverEnabled: Boolean = true,
    @SerialName("browser_takeover_min_mb") val browserTakeoverMinMb: Int = 0,
    @SerialName("browser_category_dirs") val browserCategoryDirs: Map<String, String> = emptyMap(),
)

@Serializable
data class TaskCreate(
    val url: String,
    @SerialName("task_type") val taskType: String = "auto",
    val filename: String = "",
    @SerialName("download_dir") val downloadDir: String = "",
    val concurrency: Int = 0,
    val referer: String = "",
    val origin: String = "",
    @SerialName("user_agent") val userAgent: String = "",
    val cookie: String = "",
)

@Serializable data class BatchCreate(val tasks: List<TaskCreate>)
@Serializable data class OkResult(val ok: Boolean = false, val count: Int = 0)
@Serializable data class LogResult(val log: String = "")

@Serializable
data class RecognitionCandidate(
    val url: String,
    val title: String = "",
    val quality: String = "",
    val bandwidth: Long = 0,
)

@Serializable
data class RecognitionResult(
    val candidates: List<RecognitionCandidate> = emptyList(),
    val message: String = "",
    val method: String = "",
)

@Serializable
data class BrowserHandoffDuplicate(
    val id: String,
    val status: String = "",
    val filename: String = "",
    @SerialName("output_path") val outputPath: String = "",
)

@Serializable
data class BrowserHandoff(
    val id: String,
    val url: String,
    val filename: String = "",
    val title: String = "",
    @SerialName("mime_type") val mimeType: String = "",
    @SerialName("source_page_url") val sourcePageUrl: String = "",
    val size: Long = 0,
    val status: String = "pending",
    val duplicate: Boolean = false,
    val duplicates: List<BrowserHandoffDuplicate> = emptyList(),
    @SerialName("duplicate_message") val duplicateMessage: String = "",
)

@Serializable
data class BrowserHandoffDecision(
    val filename: String,
    @SerialName("download_dir") val downloadDir: String,
    val category: String,
    val remember: Boolean = true,
)

@Serializable
data class DesktopCommand(
    val sequence: Long,
    val kind: String,
    @SerialName("handoff_id") val handoffId: String = "",
)

@Serializable
data class DesktopCommands(
    val ok: Boolean = false,
    val active: Boolean = false,
    val sequence: Long = 0,
    val commands: List<DesktopCommand> = emptyList(),
)

@Serializable
data class PresenterStatus(
    val ok: Boolean = false,
    val ready: Boolean = false,
    val session: Boolean = false,
    val mode: String = "",
)

@Serializable
data class BrowserStatus(
    val detected: Boolean = false,
    @SerialName("seen_before") val seenBefore: Boolean = false,
    val version: String = "",
    val state: String = "not_detected",
    val message: String = "",
)

@Serializable
data class UpdateInfo(
    @SerialName("current_version") val currentVersion: String = "",
    @SerialName("latest_version") val latestVersion: String = "",
    val available: Boolean = false,
    @SerialName("can_auto_install") val canAutoInstall: Boolean = false,
    @SerialName("release_url") val releaseUrl: String = "",
    val notes: String = "",
)

@Serializable
data class TorrentFileEntry(val index: Int, val path: String, val size: Long = 0)

@Serializable
data class TorrentFiles(val files: List<TorrentFileEntry> = emptyList(), val selected: List<Int> = emptyList())

@Serializable data class TorrentSelection(val indexes: List<Int>)
