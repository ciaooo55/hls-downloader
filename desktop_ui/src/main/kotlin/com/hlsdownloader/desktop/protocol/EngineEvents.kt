package com.hlsdownloader.desktop

internal sealed interface UiSignal {
    data class Notice(val level: String, val message: String) : UiSignal
    data class Clipboard(val urls: List<String>) : UiSignal
    data class Probe(val url: String, val variants: List<StreamVariantDto>) : UiSignal
    data class TorrentProbe(val data: TorrentProbeDto) : UiSignal
    data class TorrentSelection(val source: String, val files: List<TorrentFileDto>, val totalSize: Long) : UiSignal
    data class Devices(val devices: List<CastDeviceDto>) : UiSignal
    data class MediaPush(val request: MediaPushRequestDto) : UiSignal
    data class MediaPushResolved(val request: MediaPushRequestDto) : UiSignal
    data class Harvest(val url: String, val links: List<HarvestCandidateUi>) : UiSignal
    data class Log(val taskId: String, val lines: List<String>) : UiSignal
    data class Duplicate(val taskId: String, val action: String, val message: String) : UiSignal
    data class Update(
        val current: String,
        val latest: String,
        val notes: String,
        val releaseUrl: String,
        val installerName: String,
        val installerSize: Long,
        val sha256Verified: Boolean,
    ) : UiSignal
    data class UpdatePrepared(
        val latest: String,
        val installerPath: String,
        val sha256: String,
        val productName: String,
        val productVersion: String,
        val upgradeCode: String,
    ) : UiSignal
    data class PowerPending(val action: String, val title: String, val delaySeconds: Long) : UiSignal
    data class Cast(
        val active: Boolean,
        val title: String,
        val device: String,
        val status: String,
        val taskId: String = "",
        val mediaUrl: String = "",
        val deviceKind: String = "",
        val supportedActions: List<String> = emptyList(),
        val playing: Boolean = false,
        val paused: Boolean = false,
        val positionSeconds: Long = 0,
        val durationSeconds: Long = 0,
        val positionAvailable: Boolean = false,
    ) : UiSignal
}
internal fun applyEngineEvent(
    tasks: MutableList<DownloadTask>,
    taskPositions: MutableMap<String, Int>,
    event: kotlinx.serialization.json.JsonObject,
    setExtension: (String) -> Unit,
    offerHandoff: (HandoffOfferDto) -> Unit,
    resolveHandoff: (String) -> Unit,
    taskDeleted: (String) -> Unit,
): UiSignal? {
    return when (event.stringOrNull("kind")) {
        // This was the only decode in this `when` without `runCatching`, so a
        // Core snapshot that failed to decode threw straight out of the poller
        // above.  The snapshot is untrusted pipe input; a bad one now skips that
        // event instead of killing the stream.
        "task_created", "task_updated", "task_progress" -> event.objectOrNull("snapshot")?.let { snapshot ->
            val task = runCatching {
                downloadTask(protocolJson.decodeFromJsonElement(TaskDto.serializer(), snapshot))
            }.getOrNull() ?: return@let null
            upsertDownloadTask(tasks, taskPositions, task)
            null
        }
        "task_deleted" -> event.stringOrNull("task_id")?.takeIf(String::isNotBlank)?.let { id -> removeDownloadTask(tasks, taskPositions, id); taskDeleted(id); null }
        "browser_status" -> { setExtension(if (event.stringOrNull("connected") == "true") "浏览器插件 · 已连接" else Product.extensionDisconnected); null }
        "clipboard_offer" -> UiSignal.Clipboard(
            event.arrayOrNull("urls").orEmpty().mapNotNull { item -> item.stringOrNull() }.filter(String::isNotBlank),
        )
        "handoff_offered" -> event.objectOrNull("offer")?.let { offer ->
            runCatching { protocolJson.decodeFromJsonElement(HandoffOfferDto.serializer(), offer) }.getOrNull()?.let(offerHandoff)
            null
        }
        "handoff_resolved" -> event.stringOrNull("handoff_id")?.takeIf(String::isNotBlank)?.let { resolveHandoff(it); null }
        "probe_result" -> UiSignal.Probe(
            event.string("url"),
            event.arrayOrNull("variants").orEmpty().mapNotNull { runCatching { protocolJson.decodeFromJsonElement(StreamVariantDto.serializer(), it) }.getOrNull() },
        )
        "torrent_probe_result" -> UiSignal.TorrentProbe(
            TorrentProbeDto(
                source = event.string("source"),
                name = event.stringOrNull("name") ?: "torrent",
                totalSize = event.longOrNull("total_size") ?: 0,
                files = event.arrayOrNull("files").orEmpty().mapNotNull { item -> runCatching { protocolJson.decodeFromJsonElement(TorrentFileDto.serializer(), item) }.getOrNull() },
                magnet = event.stringOrNull("magnet") == "true",
            ),
        )
        "torrent_selection_result" -> UiSignal.TorrentSelection(
            source = event.string("source"),
            files = event.arrayOrNull("selections").orEmpty().mapNotNull { item -> runCatching { protocolJson.decodeFromJsonElement(TorrentFileDto.serializer(), item) }.getOrNull() },
            totalSize = event.longOrNull("total_size") ?: 0,
        )
        "cast_devices" -> UiSignal.Devices(event.arrayOrNull("devices").orEmpty().mapNotNull { runCatching { protocolJson.decodeFromJsonElement(CastDeviceDto.serializer(), it) }.getOrNull() })
        "media_push_requested" -> event.objectOrNull("request")?.let { request -> runCatching { UiSignal.MediaPush(protocolJson.decodeFromJsonElement(MediaPushRequestDto.serializer(), request)) }.getOrNull() }
        "media_push_resolved" -> event.objectOrNull("request")?.let { request -> runCatching { UiSignal.MediaPushResolved(protocolJson.decodeFromJsonElement(MediaPushRequestDto.serializer(), request)) }.getOrNull() }
        "power_action_pending" -> UiSignal.PowerPending(
            event.string("action"),
            event.string("title"),
            event.longOrNull("delay_seconds") ?: 30,
        )
        "harvest_result" -> UiSignal.Harvest(
            event.string("url"),
            event.arrayOrNull("links").orEmpty().mapNotNull { item ->
                (item as? kotlinx.serialization.json.JsonObject)?.let { value ->
                    HarvestCandidateUi(
                        url = value.string("url"),
                        filename = value.string("filename"),
                        category = value.string("category"),
                        size = value.longOrNull("size") ?: 0,
                        extension = value.string("extension"),
                    )
                }?.takeIf { it.url.isNotBlank() }
            },
        )
        "task_log" -> UiSignal.Log(event.string("task_id"), event.arrayOrNull("lines").orEmpty().mapNotNull { it.stringOrNull() })
        "duplicate_offered" -> UiSignal.Duplicate(event.string("task_id"), event.string("action"), event.stringOrNull("message") ?: "发现重复任务")
        "toast" -> UiSignal.Notice(event.stringOrNull("level") ?: "info", event.stringOrNull("message") ?: "操作已完成")
        "update_available" -> UiSignal.Update(
            current = event.string("current"),
            latest = event.string("latest"),
            notes = event.string("notes"),
            releaseUrl = event.string("release_url"),
            installerName = event.string("installer_name"),
            installerSize = event.longOrNull("installer_size") ?: 0,
            sha256Verified = event.stringOrNull("sha256_verified") == "true",
        )
        "update_current" -> UiSignal.Notice("success", "已是最新版本 ${event.string("current")}")
        "update_ready" -> UiSignal.UpdatePrepared(
            latest = event.string("latest"),
            installerPath = event.string("installer_path"),
            sha256 = event.string("sha256"),
            productName = event.string("product_name"),
            productVersion = event.string("product_version"),
            upgradeCode = event.string("upgrade_code"),
        )
        "update_install_started" -> UiSignal.Notice("info", "任务断点已保存，正在关闭并安装版本 ${event.string("latest")}")
        "update_install_result" -> UiSignal.Notice(
            if (event.stringOrNull("status") == "success") "success" else "error",
            event.stringOrNull("message") ?: "覆盖升级结果未知",
        )
        "error" -> UiSignal.Notice("error", event.stringOrNull("message") ?: "操作失败")
        "cast_session" -> UiSignal.Cast(
            active = event.stringOrNull("active") == "true",
            title = event.string("title"),
            device = event.string("device"),
            status = event.string("status"),
            taskId = event.string("task_id"),
            mediaUrl = event.string("media_url"),
            deviceKind = event.string("device_kind"),
            supportedActions = event.arrayOrNull("supported_actions").orEmpty().mapNotNull { it.stringOrNull() },
            playing = event.stringOrNull("playing") == "true",
            paused = event.stringOrNull("paused") == "true",
            positionSeconds = event.longOrNull("position_seconds") ?: 0,
            durationSeconds = event.longOrNull("duration_seconds") ?: 0,
            positionAvailable = event.stringOrNull("position_available") == "true",
        )
        else -> null
    }
}
