package com.hlsdownloader.desktop

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put
import java.io.EOFException
import java.io.File
import java.io.RandomAccessFile
import java.net.URI
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.util.concurrent.atomic.AtomicLong

val protocolJson: Json = Json { ignoreUnknownKeys = true; encodeDefaults = true }

private const val CORE_PROTOCOL = "hls-downloader-v7-core"
private const val CORE_PIPE = "\\\\.\\pipe\\HLSDownloader.v7"
private const val MAX_TASK_LOG_LINES = 500

object Product {
    const val version = "7.0.0"
    const val engineStarting = "下载引擎 · 启动中"
    const val engineConnected = "下载引擎 · 已连接"
    const val engineReconnecting = "下载引擎 · 重连中"
    const val extensionDisconnected = "浏览器插件 · 未连接"
}

@Serializable
data class CoreHello(
    val type: String = "hello",
    val protocol: String = CORE_PROTOCOL,
    val version: Int = 1,
)

@Serializable
data class TaskDto(
    @SerialName("task_id") val id: String,
    val filename: String,
    val title: String = "",
    val status: String,
    @SerialName("downloaded_bytes") val downloadedBytes: Long = 0,
    @SerialName("total_bytes") val totalBytes: Long? = null,
    @SerialName("speed_bytes_per_sec") val speedBytesPerSecond: Long = 0,
    @SerialName("peer_count") val peerCount: Int = 0,
    @SerialName("seed_count") val seedCount: Int = 0,
    @SerialName("uploaded_bytes") val uploadedBytes: Long = 0,
    @SerialName("upload_speed_bytes_per_sec") val uploadSpeedBytesPerSecond: Long = 0,
    @SerialName("eta_seconds") val etaSeconds: Long? = null,
    @SerialName("active_workers") val activeWorkers: Int = 0,
    @SerialName("completed_ranges") val completedRanges: Long = 0,
    @SerialName("total_ranges") val totalRanges: Long = 0,
    @SerialName("playback_ready") val playbackReady: Boolean = false,
    @SerialName("is_live") val isLive: Boolean = false,
    val stage: String = "",
    val url: String = "",
    @SerialName("resource_kind") val resourceKind: String = "file",
    @SerialName("available_actions") val availableActions: List<String> = emptyList(),
    @SerialName("error_code") val errorCode: String? = null,
    @SerialName("error_message") val errorMessage: String? = null,
    @SerialName("error_stage") val errorStage: String = "",
    @SerialName("error_url") val errorUrl: String = "",
    @SerialName("error_hint") val errorHint: String = "",
    @SerialName("http_status") val httpStatus: Int? = null,
    @SerialName("error_attempt") val errorAttempt: Int = 0,
    @SerialName("queue_index") val queueIndex: Long = 0,
    @SerialName("queue_id") val queueId: String = "default",
    @SerialName("output_missing") val outputMissing: Boolean = false,
    @SerialName("output_path") val outputPath: String = "",
    @SerialName("connection_hint") val connectionHint: String = "",
    @SerialName("connection_parts") val connectionParts: List<ConnectionPartDto> = emptyList(),
    @SerialName("log_tail") val logTail: List<String> = emptyList(),
    @SerialName("speed_history") val speedHistory: List<Long> = emptyList(),
    @SerialName("mirror_status") val mirrorStatus: List<MirrorStatusDto> = emptyList(),
    @SerialName("request_method") val requestMethod: String = "GET",
    @SerialName("download_dir") val downloadDirectory: String = "",
    @SerialName("speed_limit_kib") val speedLimitKib: Long = 0,
    @SerialName("expected_checksum") val expectedChecksum: String = "",
    @SerialName("checksum_algorithm") val checksumAlgorithm: String = "",
    @SerialName("checksum_actual") val checksumActual: String = "",
    @SerialName("checksum_verified") val checksumVerified: Boolean? = null,
    @SerialName("av_scan") val avScan: AvScanStatusDto? = null,
    @SerialName("max_workers") val maxWorkers: Int = 0,
    val mirrors: List<String> = emptyList(),
    @SerialName("scheduled_start_at") val scheduledStartAt: String = "",
    @SerialName("scheduled_stop_at") val scheduledStopAt: String = "",
)

@Serializable
data class ConnectionPartDto(
    val start: Long = 0,
    val end: Long = 0,
    val done: Long = 0,
    val state: String = "queued",
)

@Serializable
data class MirrorStatusDto(
    val url: String,
    @SerialName("final_url") val finalUrl: String = "",
    val state: String = "pending",
    val detail: String = "",
    val ranges: Boolean = false,
)

@Serializable
data class AvScanStatusDto(
    val state: String,
    val engine: String = "",
    val detail: String = "",
)

@Serializable data class EventEnvelopeDto(val sequence: Long, val event: JsonObject)
data class EngineSnapshot(val tasks: List<TaskDto>, val latestSequence: Long)
data class CommandResult(val requestId: Long, val events: List<EventEnvelopeDto>) {
    fun taskLogLines(): List<String> {
        val event = events.firstOrNull { it.event["kind"]?.jsonPrimitive?.content == "task_log" }?.event
        return event?.get("lines")?.jsonArray.orEmpty()
            .mapNotNull { runCatching { it.jsonPrimitive.content }.getOrNull() }
            .takeLast(MAX_TASK_LOG_LINES)
    }
}
data class TaskExportResult(val format: String, val data: String, val taskCount: Int)

@Serializable
data class EngineCapabilities(
    @SerialName("product_version") val productVersion: String = Product.version,
    @SerialName("protocol_version") val protocolVersion: Int = 1,
    val commands: List<String> = emptyList(),
    val settings: List<String> = emptyList(),
    @SerialName("max_frame_bytes") val maxFrameBytes: Long = EnginePipeClient.MAX_FRAME.toLong(),
)

@Serializable
data class StreamVariantDto(
    val label: String,
    val bandwidth: Long = 0,
    val height: Int = 0,
    val kind: String = "video",
    val name: String = "",
)

@Serializable
data class TorrentFileDto(
    val index: Int,
    val path: String,
    val size: Long = 0,
    val offset: Long = 0,
    val selected: Boolean = true,
)

@Serializable
data class TorrentProbeDto(
    val source: String,
    val name: String = "torrent",
    @SerialName("total_size") val totalSize: Long = 0,
    val files: List<TorrentFileDto> = emptyList(),
    val magnet: Boolean = false,
)

data class TaskTorrentFilesDto(
    val taskId: String,
    val source: String,
    val files: List<TorrentFileDto>,
    val totalSize: Long,
)

@Serializable
data class CastDeviceDto(
    val id: String,
    val label: String,
    val location: String = "",
    @SerialName("control_url") val controlUrl: String = "",
    @SerialName("service_type") val serviceType: String = "",
)

@Serializable
data class HandoffOfferDto(
    @SerialName("handoff_id") val handoffId: String,
    val url: String,
    @SerialName("resource_kind") val resourceKind: String = "file",
    val filename: String = "",
    val title: String = "",
    @SerialName("mime_type") val mimeType: String = "",
    val size: Long = 0,
    @SerialName("source_page_url") val sourcePageUrl: String = "",
    val status: String = "pending",
    val presentation: String = "queued",
)

@Serializable data class HandoffStatusDto(val id: String, val status: String = "pending")
@Serializable data class MediaPushRequestDto(
    val id: String,
    @SerialName("push_kind") val pushKind: String = "cast",
    val url: String,
    val title: String = "",
    val status: String = "pending",
    val message: String = "",
    val location: String = "",
    @SerialName("created_at_ms") val createdAtMs: Long = 0,
)

data class TaskDraft(
    val url: String,
    val kind: String = EnginePipeClient.recognizeResourceKind(url),
    val title: String = "",
    val filename: String = "",
    val downloadDirectory: String = "",
    val concurrency: Long = 0,
    val speedLimitKib: Long = 0,
    val checksum: String = "",
    val proxy: String = "",
    val mirrors: List<String> = emptyList(),
    val referer: String = "",
    val origin: String = "",
    val cookie: String = "",
    val userAgent: String = "",
    val requestHeaders: Map<String, String> = emptyMap(),
    val requestMethod: String = "GET",
    val curlCommand: String = "",
    val preferredBandwidth: Long = 0,
    val preferredHeight: Int = 0,
    val preferredAudio: String = "",
    val allowDuplicate: Boolean = false,
    val scheduledStartAt: String = "",
    val scheduledStopAt: String = "",
    val completionAction: String = "",
    val queueId: String = "default",
    val torrentSelection: List<TorrentFileDto> = emptyList(),
)

@Serializable
data class QueueProfileDto(
    val id: String = "default",
    val name: String = "默认队列",
    val enabled: Boolean = true,
    val priority: Int = 0,
    @SerialName("max_active") val maxActive: Int = 3,
    @SerialName("speed_limit_kib") val speedLimitKib: Long = 0,
    @SerialName("schedule_enabled") val scheduleEnabled: Boolean = false,
    @SerialName("start_time") val startTime: String = "00:00",
    @SerialName("stop_time") val stopTime: String = "23:59",
    @SerialName("active_days") val activeDays: String = "1,2,3,4,5,6,7",
    @SerialName("completion_action") val completionAction: String = "none",
)

@Serializable
data class SiteRuleDto(
    val host: String = "",
    val enabled: Boolean = true,
    @SerialName("speed_limit_kib") val speedLimitKib: Long = 0,
    val concurrency: Long = 0,
    val proxy: String = "",
    @SerialName("proxy_mode") val proxyMode: String = "",
    @SerialName("download_dir") val downloadDirectory: String = "",
    @SerialName("user_agent") val userAgent: String = "",
    val referer: String = "",
    val origin: String = "",
    @SerialName("credential_ref") val credentialRef: String = "",
)

@Serializable
data class EngineSettingsDto(
    @SerialName("takeover_enabled") val takeoverEnabled: Boolean = true,
    @SerialName("takeover_minimum_bytes") val takeoverMinimumBytes: Long = 0,
    @SerialName("legal_accepted") val legalAccepted: Boolean = false,
    @SerialName("legal_terms_version") val legalTermsVersion: String = "",
    @SerialName("speed_limit_kib") val speedLimitKib: Long = 0,
    @SerialName("hourly_quota_mib") val hourlyQuotaMib: Long = 0,
    @SerialName("schedule_enabled") val scheduleEnabled: Boolean = false,
    @SerialName("schedule_start") val scheduleStart: String = "22:00",
    @SerialName("schedule_end") val scheduleEnd: String = "08:00",
    @SerialName("schedule_kib") val scheduleKib: Long = 0,
    @SerialName("auto_category") val autoCategory: Boolean = false,
    @SerialName("category_dir_media") val categoryDirMedia: String = "",
    @SerialName("category_dir_program") val categoryDirProgram: String = "",
    @SerialName("category_dir_archive") val categoryDirArchive: String = "",
    @SerialName("category_dir_other") val categoryDirOther: String = "",
    @SerialName("queue_max") val queueMax: Long = 3,
    @SerialName("queue_profiles") val queueProfiles: List<QueueProfileDto> = listOf(QueueProfileDto()),
    @SerialName("site_rules") val siteRules: String = "",
    @SerialName("av_scan_enabled") val avScanEnabled: Boolean = false,
    @SerialName("av_scan_command") val avScanCommand: String = "",
    @SerialName("torrent_watch") val torrentWatch: String = "",
    @SerialName("torrent_watch_enabled") val torrentWatchEnabled: Boolean = false,
    @SerialName("download_dir") val downloadDirectory: String = "",
    @SerialName("temp_dir") val tempDirectory: String = "",
    @SerialName("default_concurrency") val defaultConcurrency: Long = 12,
    @SerialName("proxy_url") val proxyUrl: String = "",
    @SerialName("ffmpeg_path") val ffmpegPath: String = "",
    @SerialName("clipboard_watch") val clipboardWatch: Boolean = false,
    @SerialName("completion_sound_enabled") val completionSoundEnabled: Boolean = false,
    @SerialName("progress_window_enabled") val progressWindowEnabled: Boolean = true,
    @SerialName("complete_popup_enabled") val completePopupEnabled: Boolean = true,
    @SerialName("resume_interrupted") val resumeInterrupted: Boolean = false,
    @SerialName("auto_retry_max") val autoRetryMax: Long = 0,
    @SerialName("existing_file_policy") val existingFilePolicy: String = "rename",
    @SerialName("live_record_max_minutes") val liveRecordMaxMinutes: Long = 0,
    @SerialName("download_subtitles") val downloadSubtitles: Boolean = true,
    @SerialName("skip_ad_segments") val skipAdSegments: Boolean = true,
    @SerialName("keep_temp_files") val keepTempFiles: Boolean = false,
    @SerialName("default_user_agent") val defaultUserAgent: String = "",
    @SerialName("tvbox_endpoint") val tvboxEndpoint: String = "",
    @SerialName("dark_mode") val darkMode: Boolean = false,
    @SerialName("allow_duplicate") val allowDuplicate: Boolean = false,
    @SerialName("queue_auto_start_enabled") val queueAutoStartEnabled: Boolean = false,
    @SerialName("queue_auto_start_time") val queueAutoStartTime: String = "00:00",
    @SerialName("queue_auto_stop_enabled") val queueAutoStopEnabled: Boolean = false,
    @SerialName("queue_auto_stop_time") val queueAutoStopTime: String = "07:30",
    @SerialName("default_referer") val defaultReferer: String = "",
    @SerialName("default_origin") val defaultOrigin: String = "",
    @SerialName("allowed_hosts") val allowedHosts: String = "",
    @SerialName("http_chunk_size_mb") val httpChunkSizeMb: Long = 8,
    @SerialName("completion_power_action") val completionPowerAction: String = "none",
    @SerialName("start_on_login") val startOnLogin: Boolean = false,
    @SerialName("queue_active_days") val queueActiveDays: String = "1,2,3,4,5,6,7",
    @SerialName("proxy_mode") val proxyMode: String = "system",
    @SerialName("proxy_bypass") val proxyBypass: String = "",
    @SerialName("reduce_motion") val reduceMotion: Boolean = false,
    @SerialName("harvest_minimum_bytes") val harvestMinimumBytes: Long = 0,
    @SerialName("av_scan_fail_on_threat") val avScanFailOnThreat: Boolean = true,
    @SerialName("bt_upload_limit_kib") val btUploadLimitKib: Long = 1024,
    @SerialName("bt_max_connections") val btMaxConnections: Long = 200,
    @SerialName("bt_enable_dht") val btEnableDht: Boolean = true,
    @SerialName("preferred_cast_device_id") val preferredCastDeviceId: String = "",
    @SerialName("task_column_layout") val taskColumnLayout: String = "",
    @SerialName("toolbar_actions") val toolbarActions: String = "",
    @SerialName("task_sort") val taskSort: String = "queue:asc",
    @SerialName("default_cookie_configured") val defaultCookieConfigured: Boolean = false,
)

class EnginePipeClient(
    private val pipePath: String = System.getProperty("hls.engine.pipe") ?: CORE_PIPE,
) {
    fun snapshotState(): EngineSnapshot = session { connection ->
        val response = connection.request(request("snapshot"))
        response.requireType("snapshot", "读取任务失败")
        EngineSnapshot(
            response["tasks"]?.jsonArray.orEmpty().map { protocolJson.decodeFromJsonElement(TaskDto.serializer(), it) },
            response["latest_sequence"]?.jsonPrimitive?.content?.toLongOrNull() ?: 0,
        )
    }

    fun snapshot(): List<TaskDto> = snapshotState().tasks

    fun capabilities(): EngineCapabilities = session { connection ->
        val response = connection.request(request("capabilities"))
        response.requireType("capabilities", "读取下载引擎能力失败")
        protocolJson.decodeFromJsonElement(EngineCapabilities.serializer(), response)
    }

    fun createTask(url: String) = createTask(TaskDraft(url = url))

    fun createTask(draft: TaskDraft): CommandResult {
        val normalized = normalizeDownloadUrl(draft.url)
        return command(buildJsonObject {
            put("kind", "create_task")
            put("spec", taskSpecJson(draft, normalized))
        })
    }

    fun importCurl(draft: TaskDraft): CommandResult {
        require(isCurlCommand(draft.curlCommand)) { "输入内容不是 cURL 命令" }
        return command(buildJsonObject {
            put("kind", "import_curl")
            put("command", draft.curlCommand.trim())
            put("options", taskSpecJson(draft.copy(url = ""), ""))
        })
    }

    private fun taskSpecJson(draft: TaskDraft, normalizedUrl: String) = buildJsonObject {
        val fallbackName = normalizedUrl.takeIf(String::isNotBlank)?.let(::filenameFromUrl).orEmpty()
        put("url", normalizedUrl)
        put("resource_kind", draft.kind.ifBlank { normalizedUrl.takeIf(String::isNotBlank)?.let(::recognizeResourceKind) ?: "file" })
        put("title", draft.title.ifBlank { fallbackName })
        put("filename", draft.filename.trim().takeIf(String::isNotBlank)?.let(::normalizeHandoffFilename) ?: fallbackName)
        put("download_dir", draft.downloadDirectory.trim())
        put("concurrency", draft.concurrency.coerceIn(0, 128))
        put("speed_limit_kib", draft.speedLimitKib.coerceAtLeast(0))
        if (draft.checksum.isNotBlank()) put("checksum", draft.checksum.trim())
        put("proxy", draft.proxy.trim())
        put("mirrors", buildJsonArray { draft.mirrors.map(String::trim).filter(String::isNotBlank).distinct().forEach { add(JsonPrimitive(it)) } })
        put("request_method", draft.requestMethod.trim().uppercase())
        put("headers", buildJsonObject {
            draft.requestHeaders.forEach { (name, value) -> put(name.trim(), value.trim()) }
            if (draft.referer.isNotBlank()) put("Referer", draft.referer.trim())
            if (draft.origin.isNotBlank()) put("Origin", draft.origin.trim())
            if (draft.cookie.isNotBlank()) put("Cookie", draft.cookie.trim())
            if (draft.userAgent.isNotBlank()) put("User-Agent", draft.userAgent.trim())
        })
        put("preferred_bandwidth", draft.preferredBandwidth.coerceAtLeast(0))
        put("preferred_height", draft.preferredHeight.coerceAtLeast(0))
        put("preferred_audio", draft.preferredAudio.trim())
        put("allow_duplicate", draft.allowDuplicate)
        put("scheduled_start_at", draft.scheduledStartAt.trim())
        put("scheduled_stop_at", draft.scheduledStopAt.trim())
        put("completion_action", draft.completionAction.trim())
        put("queue_id", draft.queueId.trim().ifBlank { "default" })
        if (draft.torrentSelection.isNotEmpty()) put("torrent_selection", buildJsonArray {
            draft.torrentSelection.forEach { item -> add(buildJsonObject {
                put("index", item.index)
                put("path", item.path)
                put("selected", item.selected)
            }) }
        })
    }

    fun taskAction(taskId: String, action: String) = command(commandOf("task_action", "task_id" to taskId, "action" to action))
    fun refreshTaskRequest(taskId: String, url: String, cookie: String = "", autoResume: Boolean = true) = command(buildJsonObject {
        put("kind", "refresh_task_request")
        put("task_id", requireId(taskId))
        put("url", normalizeDownloadUrl(url))
        put("cookie", cookie)
        put("auto_resume", autoResume)
    })
    fun reorderQueue(taskId: String, delta: Int) = command(commandOf("reorder_queue", "task_id" to taskId, "delta" to delta))
    fun placeQueue(taskId: String, beforeId: String) = command(commandOf("place_queue", "task_id" to taskId, "before_id" to beforeId))
    fun assignQueue(taskIds: Collection<String>, queueId: String) = command(buildJsonObject {
        put("kind", "assign_queue")
        put("task_ids", buildJsonArray { taskIds.map(String::trim).filter(String::isNotBlank).distinct().forEach { add(JsonPrimitive(it)) } })
        put("queue_id", queueId.trim())
    })
    fun playTask(taskId: String) = command(commandOf("play_task", "task_id" to taskId))
    fun castTask(taskId: String) = command(commandOf("cast_task", "task_id" to taskId))
    fun playerControl(action: String) = command(commandOf("player_control", "action" to action))
    fun discoverCastDevices(mode: String = "") = command(commandOf("discover_cast_devices", "mode" to mode.trim().lowercase()))
    fun castToDevice(taskId: String, deviceId: String) = command(commandOf("cast_to_device", "task_id" to taskId, "device_id" to deviceId))
    fun shareMedia(path: String, url: String, title: String, deviceId: String) = command(commandOf(
        "share_media", "path" to path.trim(), "url" to url.trim(), "title" to title.trim(), "device_id" to deviceId.trim(),
    ))
    fun controlCast(action: String, seconds: Long = 0) = command(commandOf(
        "control_cast",
        "action" to when (action) {
            "seek" -> "seek:$seconds"
            "seek_to" -> "seek_to:$seconds"
            else -> action
        },
    ))
    fun resolveMediaPush(requestId: String, status: String, message: String = "", location: String = "") = command(buildJsonObject {
        put("kind", "resolve_media_push")
        put("request_id", requestId)
        put("status", status)
        put("message", message)
        put("location", location)
    })
    fun probeUrl(url: String) = probeUrl(TaskDraft(url = url))

    fun probeUrl(draft: TaskDraft): CommandResult {
        val normalized = normalizeDownloadUrl(draft.url)
        return command(buildJsonObject {
            put("kind", "probe_url")
            put("url", normalized)
            put("spec", taskSpecJson(draft, normalized))
        })
    }
    fun probeTorrent(source: String) = command(commandOf("probe_torrent", "source" to source.trim()))
    fun selectTorrentFiles(source: String, selections: List<TorrentFileDto>) = command(buildJsonObject {
        put("kind", "select_torrent_files")
        put("source", source.trim())
        put("selections", buildJsonArray {
            selections.forEach { item -> add(buildJsonObject {
                put("index", item.index)
                put("path", item.path)
                put("selected", item.selected)
            }) }
        })
    })
    fun getTaskTorrentFiles(taskId: String): TaskTorrentFilesDto = taskTorrentFilesCommand("get_task_torrent_files", taskId, emptyList())
    fun setTaskTorrentFiles(taskId: String, selections: List<TorrentFileDto>): TaskTorrentFilesDto = taskTorrentFilesCommand("set_task_torrent_files", taskId, selections)

    private fun taskTorrentFilesCommand(kind: String, taskId: String, selections: List<TorrentFileDto>): TaskTorrentFilesDto {
        val result = command(buildJsonObject {
            put("kind", kind)
            put("task_id", requireId(taskId))
            if (kind == "set_task_torrent_files") put("selections", buildJsonArray {
                selections.forEach { item -> add(buildJsonObject {
                    put("index", item.index)
                    put("path", item.path)
                    put("selected", item.selected)
                }) }
            })
        })
        val event = result.events.firstOrNull { it.event["kind"]?.jsonPrimitive?.content == "task_torrent_files" }?.event
            ?: error("下载引擎未返回 BT 文件清单")
        val selected = event["selections"]?.jsonArray.orEmpty().mapNotNull { item ->
            runCatching { protocolJson.decodeFromJsonElement(TorrentFileDto.serializer(), item) }.getOrNull()
        }.associateBy { it.index to it.path }
        val files = event["files"]?.jsonArray.orEmpty().mapNotNull { item ->
            runCatching { protocolJson.decodeFromJsonElement(TorrentFileDto.serializer(), item) }.getOrNull()
        }.map { file -> file.copy(selected = selected[file.index to file.path]?.selected ?: true) }
        return TaskTorrentFilesDto(
            taskId = event["task_id"]?.jsonPrimitive?.content.orEmpty(),
            source = event["source"]?.jsonPrimitive?.content.orEmpty(),
            files = files,
            totalSize = event["total_size"]?.jsonPrimitive?.content?.toLongOrNull() ?: 0,
        )
    }
    fun getTaskLog(taskId: String): CommandResult = command(commandOf("get_task_log", "task_id" to taskId))
    fun openCompleted(taskId: String, folder: Boolean) = command(commandOf("open_completed", "task_id" to taskId, "folder" to folder))
    fun saveSiteProfile(taskId: String) = command(commandOf("save_site_profile", "task_id" to taskId))
    fun checkUpdate(silent: Boolean = false) = command(commandOf("check_update", "silent" to silent))
    fun downloadUpdate() = command(commandOf("download_update"))
    fun installUpdate(workbenchPid: Long) = command(commandOf("install_update", "workbench_pid" to workbenchPid.coerceAtLeast(0)))
    fun confirmPowerAction() = command(commandOf("confirm_power_action"))
    fun cancelPowerAction() = command(commandOf("cancel_power_action"))
    fun clearCompleted() = command(commandOf("clear_completed"))

    fun importPaths(paths: List<String>) = command(buildJsonObject {
        put("kind", "import_paths")
        put("paths", buildJsonArray { paths.map(String::trim).filter(String::isNotBlank).forEach { add(JsonPrimitive(it)) } })
    })

    fun exportTasks(taskIds: List<String>, format: String): TaskExportResult {
        val result = command(buildJsonObject {
            put("kind", "export_tasks")
            put("task_ids", buildJsonArray { taskIds.map(String::trim).filter(String::isNotBlank).distinct().forEach { add(JsonPrimitive(it)) } })
            put("format", format.trim().lowercase())
        })
        val event = result.events.firstOrNull { it.event["kind"]?.jsonPrimitive?.content == "task_export" }?.event
            ?: error("下载引擎未返回导出数据")
        return TaskExportResult(
            format = event["format"]?.jsonPrimitive?.content ?: format,
            data = event["data"]?.jsonPrimitive?.content.orEmpty(),
            taskCount = event["task_count"]?.jsonPrimitive?.content?.toIntOrNull() ?: 0,
        )
    }

    fun harvestPage(
        url: String,
        referer: String = "",
        probeUrls: List<String> = emptyList(),
    ) = command(buildJsonObject {
        val normalizedUrl = normalizeHttpUrl(url)
        val normalizedReferer = referer.trim().ifBlank { normalizedUrl }.let(::normalizeHttpUrl)
        val candidates = probeUrls.map(String::trim).filter(String::isNotBlank).distinct()
        require(candidates.size <= 100) { "一次最多读取 100 个链接的大小" }
        put("kind", "harvest_page")
        put("url", normalizedUrl)
        put("referer", normalizedReferer)
        put("probe_urls", buildJsonArray {
            candidates.forEach { candidate ->
                require(candidate.length <= 8192 && candidate.none(Char::isISOControl)) { "资源链接无效" }
                add(JsonPrimitive(candidate))
            }
        })
    })

    fun probeHarvestSizes(url: String, referer: String, probeUrls: List<String>): Map<String, Long> {
        val result = harvestPage(url, referer, probeUrls)
        val event = result.events.firstOrNull {
            it.event["kind"]?.jsonPrimitive?.content == "harvest_probe_result"
        }?.event ?: error("下载引擎未返回文件大小")
        return event["links"]?.jsonArray.orEmpty().mapNotNull { item ->
            val value = item.jsonObject
            val candidate = value["url"]?.jsonPrimitive?.content.orEmpty()
            val size = value["size"]?.jsonPrimitive?.content?.toLongOrNull() ?: 0
            (candidate to size).takeIf { candidate.isNotBlank() && size > 0 }
        }.toMap()
    }
    fun presentHandoff(handoffId: String, presented: Boolean = true, presenterId: String = "") = command(commandOf(
        "present_handoff", "handoff_id" to requireId(handoffId), "ok" to presented, "presenter_id" to presenterId,
    ))
    fun rejectHandoff(handoffId: String, suppressSiteKind: Boolean = false) = command(commandOf(
        "reject_handoff", "handoff_id" to requireId(handoffId), "suppress_site_kind" to suppressSiteKind,
    ))
    fun acceptHandoff(handoffId: String, filename: String, downloadDirectory: String) = command(commandOf(
        "accept_handoff", "handoff_id" to requireId(handoffId), "filename" to normalizeHandoffFilename(filename), "download_dir" to downloadDirectory.trim(), "trusted_ui" to true,
    ))

    fun loadSettings(): EngineSettingsDto = session { connection ->
        val response = connection.request(request("load_settings"))
        response.requireType("settings", "读取设置失败")
        protocolJson.decodeFromJsonElement(EngineSettingsDto.serializer(), response)
    }

    fun saveSettings(settings: EngineSettingsDto): EngineSettingsDto {
        val response = storeSettings(settings.toStorageMap())
        return protocolJson.decodeFromJsonElement(EngineSettingsDto.serializer(), response)
    }

    fun saveDefaultCookie(cookie: String): EngineSettingsDto = session { connection ->
        val response = connection.request(buildJsonObject {
            put("type", "set_default_cookie"); put("request_id", nextRequestId()); put("cookie", cookie)
        })
        response.requireType("settings", "默认 Cookie 保存失败")
        protocolJson.decodeFromJsonElement(EngineSettingsDto.serializer(), response)
    }

    internal fun saveSiteRuleCredential(edit: SiteRuleCredentialEdit): EngineSettingsDto = session { connection ->
        val response = connection.request(buildJsonObject {
            put("type", "set_site_rule_credential")
            put("request_id", nextRequestId())
            put("host", edit.host.trim())
            put("cookie", edit.cookie)
            put("request_headers", buildJsonObject {
                edit.requestHeaders.forEach { (name, value) -> put(name, value) }
            })
            put("clear", edit.clear)
        })
        response.requireType("settings", "站点凭据保存失败")
        protocolJson.decodeFromJsonElement(EngineSettingsDto.serializer(), response)
    }

    fun storeSettings(values: Map<String, JsonElement>): JsonObject = session { connection ->
        val response = connection.request(buildJsonObject {
            put("type", "store_settings"); put("request_id", nextRequestId()); put("values", JsonObject(values))
        })
        response.requireType("settings", "设置保存失败")
        response
    }

    fun storeSetting(key: String, value: JsonElement) = storeSettings(mapOf(key to value))
    fun storeSetting(key: String, value: Boolean) = storeSetting(key, JsonPrimitive(value))
    fun storeSetting(key: String, value: Long) = storeSetting(key, JsonPrimitive(value))
    fun storeSetting(key: String, value: String) = storeSetting(key, JsonPrimitive(value))

    fun waitEvents(afterSequence: Long, timeoutMs: Long = 20_000): List<EventEnvelopeDto> = session { connection ->
        val response = connection.request(buildJsonObject {
            put("type", "wait_events"); put("request_id", nextRequestId()); put("after_sequence", afterSequence); put("timeout_ms", timeoutMs.coerceIn(1, 30_000))
        })
        response.requireType("events", "事件订阅失败")
        response["events"]?.jsonArray.orEmpty().map { protocolJson.decodeFromJsonElement(EventEnvelopeDto.serializer(), it) }
    }

    fun loadHandoffStatuses(): List<HandoffStatusDto> = session { connection ->
        val response = connection.request(request("load_handoffs"))
        response.requireType("handoffs", "读取浏览器接管状态失败")
        response["items"]?.jsonArray.orEmpty().mapNotNull { item ->
            runCatching { protocolJson.decodeFromString(HandoffStatusDto.serializer(), item.jsonPrimitive.content) }.getOrNull()
        }
    }

    fun loadHandoffs(): List<HandoffOfferDto> = session { connection ->
        val response = connection.request(request("load_handoffs"))
        response.requireType("handoffs", "读取浏览器接管请求失败")
        response["items"]?.jsonArray.orEmpty().mapNotNull { item ->
            runCatching {
                val handoff = protocolJson.parseToJsonElement(item.jsonPrimitive.content).jsonObject
                if (handoff["status"]?.jsonPrimitive?.content != "pending") return@runCatching null
                val offer = protocolJson.decodeFromJsonElement(
                    HandoffOfferDto.serializer(),
                    handoff["offer"]?.jsonObject ?: return@runCatching null,
                )
                offer.copy(
                    handoffId = handoff["id"]?.jsonPrimitive?.content ?: offer.handoffId,
                    filename = handoff["filename"]?.jsonPrimitive?.content ?: offer.filename,
                    title = handoff["title"]?.jsonPrimitive?.content ?: offer.title,
                    mimeType = handoff["mime_type"]?.jsonPrimitive?.content ?: offer.mimeType,
                    size = handoff["size"]?.jsonPrimitive?.content?.toLongOrNull() ?: offer.size,
                    status = "pending",
                    presentation = handoff["presentation"]?.jsonPrimitive?.content ?: offer.presentation,
                )
            }.getOrNull()
        }
    }

    fun loadMediaPushRequests(): List<MediaPushRequestDto> = session { connection ->
        val response = connection.request(request("load_handoffs"))
        response.requireType("handoffs", "读取媒体推送状态失败")
        response["items"]?.jsonArray.orEmpty().mapNotNull { item ->
            runCatching { protocolJson.decodeFromString(MediaPushRequestDto.serializer(), item.jsonPrimitive.content) }.getOrNull()
        }.filter { it.id.startsWith("media-push-") && it.status == "pending" }
    }

    private fun command(payload: JsonObject): CommandResult = session { connection ->
        val requestId = nextRequestId()
        val response = connection.request(buildJsonObject {
            put("type", "command"); put("request_id", requestId); put("command", payload)
        })
        response.requireType("events", "下载引擎命令失败")
        CommandResult(requestId, response["events"]?.jsonArray.orEmpty().map { protocolJson.decodeFromJsonElement(EventEnvelopeDto.serializer(), it) })
    }

    private fun <T> session(block: (PipeConnection) -> T): T = connect().use { connection -> connection.hello(); block(connection) }
    private fun request(type: String) = buildJsonObject { put("type", type); put("request_id", nextRequestId()) }
    private fun connect() = PipeConnection(RandomAccessFile(pipePath, "rw"))

    private class PipeConnection(private val pipe: RandomAccessFile) : AutoCloseable {
        fun hello() {
            request(buildJsonObject { put("type", "hello"); put("protocol", CORE_PROTOCOL); put("version", 1) })
                .requireType("hello", "下载引擎握手失败")
        }

        fun request(message: JsonObject): JsonObject {
            val payload = protocolJson.encodeToString(JsonObject.serializer(), message).encodeToByteArray()
            require(payload.size <= MAX_FRAME) { "下载引擎请求过大" }
            pipe.write(ByteBuffer.allocate(4).order(ByteOrder.LITTLE_ENDIAN).putInt(payload.size).array())
            pipe.write(payload)
            val header = ByteArray(4)
            try { pipe.readFully(header) } catch (_: EOFException) { throw EOFException("下载引擎连接已关闭") }
            val size = ByteBuffer.wrap(header).order(ByteOrder.LITTLE_ENDIAN).int
            require(size in 1..MAX_FRAME) { "下载引擎响应长度无效" }
            val body = ByteArray(size)
            try { pipe.readFully(body) } catch (_: EOFException) { throw EOFException("下载引擎响应不完整") }
            val response = protocolJson.parseToJsonElement(body.decodeToString()).jsonObject
            if (response["type"]?.jsonPrimitive?.content == "error") {
                throw EngineProtocolException(
                    response["code"]?.jsonPrimitive?.content ?: "engine_error",
                    response["message"]?.jsonPrimitive?.content ?: "下载引擎返回错误",
                )
            }
            return response
        }
        override fun close() = pipe.close()
    }

    private fun filenameFromUrl(url: String) = url.substringBefore('?').substringBefore('#').substringAfterLast('/').ifBlank { "download" }

    companion object {
        @Volatile private var engineProcess: Process? = null
        @Volatile private var presenterProcess: Process? = null
        const val MAX_FRAME = 4 * 1024 * 1024
        private val requestIds = AtomicLong(100)
        private fun nextRequestId() = requestIds.incrementAndGet()

        fun isCurlCommand(value: String): Boolean =
            value.trimStart().lineSequence().firstOrNull().orEmpty()
                .trimStart().let { it.startsWith("curl ", true) || it.startsWith("curl.exe ", true) }

        fun recognizeResourceKind(url: String): String {
            val path = url.substringBefore('?').substringBefore('#').lowercase()
            return when {
                url.startsWith("magnet:", true) || path.endsWith(".torrent") -> "torrent"
                path.endsWith(".m3u8") -> "hls"
                path.endsWith(".mpd") -> "dash"
                url.startsWith("sftp:", true) -> "sftp"
                url.startsWith("ftp:", true) -> "ftp"
                else -> "file"
            }
        }

        fun normalizeDownloadUrl(value: String): String {
            val url = value.trim()
            require(url.isNotEmpty()) { "下载链接不能为空" }
            require(url.none(Char::isISOControl)) { "下载链接包含无效控制字符" }
            val localTorrent = url.substringBefore('?').substringBefore('#').endsWith(".torrent", true)
            require(url.startsWith("http://", true) || url.startsWith("https://", true) || url.startsWith("ftp://", true) || url.startsWith("sftp://", true) || url.startsWith("magnet:", true) || localTorrent) { "不支持的下载链接格式" }
            if (!url.startsWith("magnet:", true) && !localTorrent) {
                require(runCatching { URI(url) }.getOrNull()?.host?.isNotBlank() == true) { "下载链接缺少有效主机名" }
            }
            return url
        }

        fun normalizeHttpUrl(value: String): String {
            val url = value.trim()
            require(url.isNotEmpty()) { "网页地址不能为空" }
            require(url.none(Char::isISOControl)) { "网页地址包含无效控制字符" }
            require(url.startsWith("http://", true) || url.startsWith("https://", true)) { "页面抓取仅支持 HTTP 或 HTTPS 地址" }
            require(runCatching { URI(url) }.getOrNull()?.host?.isNotBlank() == true) { "网页地址缺少有效主机名" }
            return url
        }

        fun normalizeHandoffFilename(value: String): String {
            val filename = value.trim()
            require(filename.isNotEmpty()) { "文件名不能为空" }
            require(filename.length <= 240) { "文件名过长" }
            require(filename.none(Char::isISOControl)) { "文件名包含无效控制字符" }
            require('/' !in filename && '\\' !in filename && filename != "." && filename != "..") { "文件名不能包含路径" }
            return filename
        }

        @Synchronized
        fun ensureStarted(): Boolean {
            if (engineProcess?.isAlive == true) return true
            val working = File(System.getProperty("user.dir"))
            val configured = System.getenv("HLS_ENGINE_PATH")?.takeIf(String::isNotBlank)?.let(::File)
            val packaged = System.getProperty("compose.application.resources.dir")?.takeIf(String::isNotBlank)?.let { File(it, "HLSDownloaderEngine.exe") }
            val candidate = listOfNotNull(
                configured, packaged, File(working, "HLSDownloaderEngine.exe"), File(working, "app/resources/HLSDownloaderEngine.exe"),
                working.parentFile?.let { File(it, "HLSDownloaderEngine.exe") }, working.parentFile?.let { File(it, "app/resources/HLSDownloaderEngine.exe") },
            ).firstOrNull(File::isFile) ?: return false
            engineProcess = ProcessBuilder(candidate.absolutePath)
                .directory(candidate.parentFile)
                .redirectOutput(ProcessBuilder.Redirect.DISCARD)
                .redirectError(ProcessBuilder.Redirect.DISCARD)
                .start()
            return true
        }

        @Synchronized
        fun ensurePresenterStarted(): Boolean {
            if (presenterProcess?.isAlive == true) return true
            val working = File(System.getProperty("user.dir"))
            val configured = System.getenv("HLS_PRESENTER_PATH")?.takeIf(String::isNotBlank)?.let(::File)
            val packaged = System.getProperty("compose.application.resources.dir")?.takeIf(String::isNotBlank)?.let { File(it, "HLSDownloaderPresenter.exe") }
            val candidate = listOfNotNull(
                configured, packaged, File(working, "HLSDownloaderPresenter.exe"), File(working, "app/resources/HLSDownloaderPresenter.exe"),
                working.parentFile?.let { File(it, "HLSDownloaderPresenter.exe") }, working.parentFile?.let { File(it, "app/resources/HLSDownloaderPresenter.exe") },
            ).firstOrNull(File::isFile) ?: return false
            presenterProcess = ProcessBuilder(candidate.absolutePath)
                .directory(candidate.parentFile)
                .redirectOutput(ProcessBuilder.Redirect.DISCARD)
                .redirectError(ProcessBuilder.Redirect.DISCARD)
                .start()
            return true
        }
    }
}

class EngineProtocolException(val code: String, message: String) : IllegalStateException(message)

private fun JsonObject.requireType(expected: String, fallback: String) {
    require(this["type"]?.jsonPrimitive?.content == expected) { this["message"]?.jsonPrimitive?.content ?: fallback }
}

private fun requireId(value: String): String = value.trim().also { require(it.isNotEmpty()) { "请求缺少编号" } }

private fun commandOf(kind: String, vararg fields: Pair<String, Any>) = buildJsonObject {
    put("kind", kind)
    fields.forEach { (key, value) ->
        when (value) {
            is String -> put(key, value)
            is Boolean -> put(key, value)
            is Int -> put(key, value)
            is Long -> put(key, value)
            else -> error("Unsupported command value for $key")
        }
    }
}

fun EngineSettingsDto.toStorageMap(): Map<String, JsonElement> = linkedMapOf(
    "browser_takeover_enabled" to JsonPrimitive(takeoverEnabled),
    "browser_takeover_minimum_bytes" to JsonPrimitive(takeoverMinimumBytes),
    "download_speed_limit_kib" to JsonPrimitive(speedLimitKib),
    "download_hourly_quota_mib" to JsonPrimitive(hourlyQuotaMib),
    "download_speed_schedule_enabled" to JsonPrimitive(scheduleEnabled),
    "download_speed_schedule_start" to JsonPrimitive(scheduleStart),
    "download_speed_schedule_end" to JsonPrimitive(scheduleEnd),
    "download_speed_schedule_kib" to JsonPrimitive(scheduleKib),
    "auto_category_dirs" to JsonPrimitive(autoCategory),
    "browser_category_dirs" to JsonPrimitive(listOf(categoryDirMedia, categoryDirProgram, categoryDirArchive, categoryDirOther).joinToString("|")),
    "queue_max_active" to JsonPrimitive(queueMax),
    "queue_profiles" to buildJsonArray { queueProfiles.forEach { add(protocolJson.encodeToJsonElement(QueueProfileDto.serializer(), it)) } },
    "site_rules" to JsonPrimitive(siteRules),
    "av_scan_enabled" to JsonPrimitive(avScanEnabled), "av_scan_command" to JsonPrimitive(avScanCommand),
    "torrent_watch_dir" to JsonPrimitive(torrentWatch), "watch_torrents" to JsonPrimitive(torrentWatchEnabled),
    "download_dir" to JsonPrimitive(downloadDirectory), "temp_dir" to JsonPrimitive(tempDirectory),
    "default_concurrency" to JsonPrimitive(defaultConcurrency), "proxy_url" to JsonPrimitive(proxyUrl),
    "ffmpeg_path" to JsonPrimitive(ffmpegPath), "clipboard_watch" to JsonPrimitive(clipboardWatch),
    "completion_sound_enabled" to JsonPrimitive(completionSoundEnabled), "download_progress_window_enabled" to JsonPrimitive(progressWindowEnabled),
    "download_complete_popup_enabled" to JsonPrimitive(completePopupEnabled), "resume_interrupted_on_startup" to JsonPrimitive(resumeInterrupted),
    "auto_retry_failed_max" to JsonPrimitive(autoRetryMax), "existing_file_policy" to JsonPrimitive(existingFilePolicy),
    "live_record_max_minutes" to JsonPrimitive(liveRecordMaxMinutes), "download_subtitles" to JsonPrimitive(downloadSubtitles),
    "skip_ad_segments" to JsonPrimitive(skipAdSegments), "keep_temp_files" to JsonPrimitive(keepTempFiles),
    "default_user_agent" to JsonPrimitive(defaultUserAgent), "tvbox_endpoint" to JsonPrimitive(tvboxEndpoint),
    "dark_mode" to JsonPrimitive(darkMode), "allow_duplicate" to JsonPrimitive(allowDuplicate),
    "queue_auto_start_enabled" to JsonPrimitive(queueAutoStartEnabled), "queue_auto_start_time" to JsonPrimitive(queueAutoStartTime),
    "queue_auto_stop_enabled" to JsonPrimitive(queueAutoStopEnabled), "queue_auto_stop_time" to JsonPrimitive(queueAutoStopTime),
    "default_referer" to JsonPrimitive(defaultReferer), "default_origin" to JsonPrimitive(defaultOrigin),
    "allowed_hosts" to JsonPrimitive(allowedHosts), "http_chunk_size_mb" to JsonPrimitive(httpChunkSizeMb),
    "completion_power_action" to JsonPrimitive(completionPowerAction), "start_on_login" to JsonPrimitive(startOnLogin),
    "queue_active_days" to JsonPrimitive(queueActiveDays), "proxy_mode" to JsonPrimitive(proxyMode),
    "proxy_bypass" to JsonPrimitive(proxyBypass), "reduce_motion" to JsonPrimitive(reduceMotion),
    "harvest_minimum_bytes" to JsonPrimitive(harvestMinimumBytes), "av_scan_fail_on_threat" to JsonPrimitive(avScanFailOnThreat),
    "bt_upload_limit_kib" to JsonPrimitive(btUploadLimitKib), "bt_max_connections" to JsonPrimitive(btMaxConnections),
    "bt_enable_dht" to JsonPrimitive(btEnableDht), "preferred_cast_device_id" to JsonPrimitive(preferredCastDeviceId),
    "task_column_layout" to JsonPrimitive(taskColumnLayout), "toolbar_actions" to JsonPrimitive(toolbarActions),
    "task_sort" to JsonPrimitive(taskSort),
)
