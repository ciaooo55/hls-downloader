package com.hlsdownloader.desktop

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.long

class ProtocolTest {
    @Test fun helloKeepsCoreCompatibility() {
        val encoded = protocolJson.encodeToString(CoreHello())
        assertTrue(encoded.contains("hls-downloader-v7-core"))
        assertEquals(1, protocolJson.decodeFromString<CoreHello>(encoded).version)
    }
    @Test fun productCopyDoesNotExposeInternalNames() {
        assertEquals("7.0.0", Product.version)
        assertEquals("下载引擎 · 已连接", Product.engineConnected)
        assertEquals("浏览器插件 · 未连接", Product.extensionDisconnected)
    }
    @Test fun eventEnvelopeDecodesTaskUpdate() {
        val envelope = protocolJson.decodeFromString<EventEnvelopeDto>("""{"sequence":42,"event":{"kind":"task_updated","snapshot":{"task_id":"t-1","filename":"clip.mp4","status":"running"}}}""")
        assertEquals(42, envelope.sequence)
        assertEquals("task_updated", envelope.event["kind"]?.toString()?.trim('"'))
        val task = protocolJson.decodeFromJsonElement(TaskDto.serializer(), envelope.event["snapshot"]!!.jsonObject)
        assertEquals("t-1", task.id)
    }
    @Test fun urlNormalizationTrimsAndRejectsUnsafeInput() {
        assertEquals("https://example.test/a.m3u8", EnginePipeClient.normalizeDownloadUrl("  https://example.test/a.m3u8  "))
        assertEquals("https://example.test/page", EnginePipeClient.normalizeHttpUrl("https://example.test/page"))
        assertFailsWith<IllegalArgumentException> { EnginePipeClient.normalizeDownloadUrl(" ") }
        assertFailsWith<IllegalArgumentException> { EnginePipeClient.normalizeDownloadUrl("javascript:alert(1)") }
        assertFailsWith<IllegalArgumentException> { EnginePipeClient.normalizeDownloadUrl("https://example.test/a\u0000.m3u8") }
        assertFailsWith<IllegalArgumentException> { EnginePipeClient.normalizeDownloadUrl("https://") }
        assertFailsWith<IllegalArgumentException> { EnginePipeClient.normalizeHttpUrl("file:///C:/Windows") }
        assertFailsWith<IllegalArgumentException> { EnginePipeClient.normalizeHttpUrl("https:///missing-host") }
    }
    @Test fun resourceRecognitionUsesUrlPathBeforeQuery() {
        assertEquals("hls", EnginePipeClient.recognizeResourceKind("https://cdn.test/live.M3U8?token=abc#v"))
        assertEquals("dash", EnginePipeClient.recognizeResourceKind("https://cdn.test/manifest.mpd?sig=1"))
        assertEquals("torrent", EnginePipeClient.recognizeResourceKind("https://cdn.test/file.TORRENT?download=1"))
        assertEquals("torrent", EnginePipeClient.recognizeResourceKind("magnet:?xt=urn:btih:abc"))
        assertEquals("sftp", EnginePipeClient.recognizeResourceKind("sftp://host/path/file.bin"))
        assertEquals("ftp", EnginePipeClient.recognizeResourceKind("ftp://host/path/file.bin"))
        assertEquals("file", EnginePipeClient.recognizeResourceKind("https://cdn.test/file.zip"))
    }
    @Test fun handoffOfferUsesPublicMetadataAndSafeFilename() {
        val event = protocolJson.decodeFromString<EventEnvelopeDto>("""{"sequence":43,"event":{"kind":"handoff_offered","offer":{"handoff_id":"offer-1","url":"https://cdn.test/setup.exe","resource_kind":"file","filename":"setup.exe","title":"Setup","size":2048}}}""")
        val offer = protocolJson.decodeFromJsonElement(HandoffOfferDto.serializer(), event.event["offer"]!!.jsonObject)
        assertEquals("offer-1", offer.handoffId)
        assertEquals(2048, offer.size)
        assertEquals("setup.exe", EnginePipeClient.normalizeHandoffFilename(" setup.exe "))
        assertFailsWith<IllegalArgumentException> { EnginePipeClient.normalizeHandoffFilename("..\\outside.exe") }
        assertFailsWith<IllegalArgumentException> { EnginePipeClient.normalizeHandoffFilename(" ") }
    }
    @Test fun handoffStatusDecodesWithoutOfferPayload() {
        val status = protocolJson.decodeFromString<HandoffStatusDto>("""{"id":"offer-1","status":"accepted","task_id":"task-1"}""")
        assertEquals("offer-1", status.id)
        assertEquals("accepted", status.status)
    }

    @Test fun mediaPushRequestEventKeepsBrowserRequestForDevicePicker() {
        val envelope = protocolJson.decodeFromString<EventEnvelopeDto>(
            """{"sequence":44,"event":{"kind":"media_push_requested","request":{"id":"media-push-1","push_kind":"tvbox","url":"https://cdn.test/video.mp4","title":"客厅影片","status":"pending","message":"等待选择设备"}}}""",
        )
        val request = protocolJson.decodeFromJsonElement(MediaPushRequestDto.serializer(), envelope.event["request"]!!.jsonObject)
        assertEquals("media-push-1", request.id)
        assertEquals("tvbox", request.pushKind)
        assertEquals("https://cdn.test/video.mp4", request.url)
        assertEquals("pending", request.status)
    }

    @Test fun mediaPushResolvedEventKeepsUserVisibleResult() {
        val envelope = protocolJson.decodeFromString<EventEnvelopeDto>(
            """{"sequence":45,"event":{"kind":"media_push_resolved","request":{"id":"media-push-1","push_kind":"cast","url":"https://cdn.test/video.mp4","title":"客厅影片","status":"done","message":"已发送到客厅电视","location":"dlna:living-room"}}}""",
        )
        val request = protocolJson.decodeFromJsonElement(MediaPushRequestDto.serializer(), envelope.event["request"]!!.jsonObject)
        assertEquals("media_push_resolved", envelope.event["kind"]?.toString()?.trim('"'))
        assertEquals("done", request.status)
        assertEquals("已发送到客厅电视", request.message)
        assertEquals("dlna:living-room", request.location)
    }

    @Test fun powerActionPendingEventKeepsConfirmationDetails() {
        val envelope = protocolJson.decodeFromString<EventEnvelopeDto>(
            """{"sequence":46,"event":{"kind":"power_action_pending","action":"sleep","title":"movie.mp4","delay_seconds":30}}""",
        )
        assertEquals("power_action_pending", envelope.event["kind"]?.toString()?.trim('"'))
        assertEquals("sleep", envelope.event["action"]?.toString()?.trim('"'))
        assertEquals("movie.mp4", envelope.event["title"]?.toString()?.trim('"'))
        assertEquals("30", envelope.event["delay_seconds"]?.toString())
    }

    @Test fun settingsUseTheCoreStorageContractInsteadOfResponseAliases() {
        val values = EngineSettingsDto(
            takeoverEnabled = false,
            takeoverMinimumBytes = 4096,
            speedLimitKib = 512,
            scheduleEnabled = true,
            categoryDirMedia = "D:/Media",
            categoryDirProgram = "D:/Apps",
            categoryDirArchive = "D:/Archives",
            categoryDirOther = "D:/Other",
            progressWindowEnabled = false,
            completePopupEnabled = false,
            resumeInterrupted = true,
            autoRetryMax = 7,
            tempDirectory = "E:/HLS/Cache",
            defaultOrigin = "https://player.example.test",
            allowedHosts = "*.example.test,cdn.test",
            torrentWatchEnabled = true,
            avScanFailOnThreat = false,
            btUploadLimitKib = 256,
            btMaxConnections = 64,
            btEnableDht = false,
            defaultCookieConfigured = true,
        ).toStorageMap()

        assertEquals(JsonPrimitive(false), values["browser_takeover_enabled"])
        assertEquals(JsonPrimitive(4096), values["browser_takeover_minimum_bytes"])
        assertEquals(JsonPrimitive(512), values["download_speed_limit_kib"])
        assertEquals(JsonPrimitive(true), values["download_speed_schedule_enabled"])
        assertEquals(
            JsonPrimitive("D:/Media|D:/Apps|D:/Archives|D:/Other"),
            values["browser_category_dirs"],
        )
        assertEquals(JsonPrimitive(false), values["download_progress_window_enabled"])
        assertEquals(JsonPrimitive(false), values["download_complete_popup_enabled"])
        assertEquals(JsonPrimitive(true), values["resume_interrupted_on_startup"])
        assertEquals(JsonPrimitive(7), values["auto_retry_failed_max"])
        assertEquals(JsonPrimitive("E:/HLS/Cache"), values["temp_dir"])
        assertEquals(JsonPrimitive("https://player.example.test"), values["default_origin"])
        assertEquals(JsonPrimitive("*.example.test,cdn.test"), values["allowed_hosts"])
        assertEquals(JsonPrimitive(true), values["watch_torrents"])
        assertEquals(JsonPrimitive(false), values["av_scan_fail_on_threat"])
        assertEquals(JsonPrimitive(256), values["bt_upload_limit_kib"])
        assertEquals(JsonPrimitive(64), values["bt_max_connections"])
        assertEquals(JsonPrimitive(false), values["bt_enable_dht"])
        assertFalse(values.containsKey("takeover_enabled"))
        assertFalse(values.containsKey("speed_limit_kib"))
        assertFalse(values.containsKey("progress_window_enabled"))
        assertFalse(values.containsKey("default_cookie"))
        assertFalse(values.values.any { it.toString().contains("cookie", ignoreCase = true) })
    }

    @Test fun settingsOnlyExposeWhetherTheDefaultCookieIsConfigured() {
        val settings = protocolJson.decodeFromString<EngineSettingsDto>(
            """{"default_cookie_configured":true,"default_cookie":"must-not-be-decoded"}""",
        )
        assertTrue(settings.defaultCookieConfigured)
        assertFalse(protocolJson.encodeToString(settings).contains("must-not-be-decoded"))
    }

    @Test fun taskSnapshotPreservesConnectionsAndPerformanceHistory() {
        val task = protocolJson.decodeFromString<TaskDto>(
            """{"task_id":"t-9","filename":"very-long-file-name.bin","status":"running","active_workers":12,"connection_parts":[{"start":0,"end":1023,"done":512,"state":"running"}],"speed_history":[10,20,30],"mirror_status":"mirror-2"}""",
        )
        assertEquals(12, task.activeWorkers)
        assertEquals(512, task.connectionParts.single().done)
        assertEquals(listOf(10L, 20L, 30L), task.speedHistory)
        assertEquals("mirror-2", task.mirrorStatus)
    }

    @Test fun queueProfilesRoundTripThroughSettingsStorageContract() {
        val profiles = listOf(
            QueueProfileDto(),
            QueueProfileDto(
                id = "night-media",
                name = "夜间媒体",
                priority = 20,
                maxActive = 2,
                speedLimitKib = 4096,
                scheduleEnabled = true,
                startTime = "23:00",
                stopTime = "07:00",
                activeDays = "1,2,3,4,5",
                completionAction = "hibernate",
            ),
        )
        val stored = EngineSettingsDto(queueProfiles = profiles).toStorageMap()["queue_profiles"]!!.jsonArray
        assertEquals(2, stored.size)
        assertEquals("night-media", stored[1].jsonObject["id"]?.jsonPrimitive?.content)
        assertEquals(4096, stored[1].jsonObject["speed_limit_kib"]?.jsonPrimitive?.long)
        assertEquals("hibernate", stored[1].jsonObject["completion_action"]?.jsonPrimitive?.content)

        val decoded = protocolJson.decodeFromString<EngineSettingsDto>(
            """{"queue_profiles":${stored}}""",
        )
        assertEquals(profiles, decoded.queueProfiles)
    }

    @Test fun queueProfileValidationRejectsAmbiguousOrInvalidSchedules() {
        assertTrue(queueProfilesValid(listOf(QueueProfileDto())))
        assertFalse(queueProfilesValid(listOf(QueueProfileDto(), QueueProfileDto(id = "other", name = "默认队列"))))
        assertFalse(queueProfilesValid(listOf(QueueProfileDto(activeDays = "1,1"))))
        assertFalse(queueProfilesValid(listOf(QueueProfileDto(startTime = "25:00"))))
        assertFalse(queueProfilesValid(listOf(QueueProfileDto(id = "INVALID"))))
        assertTrue(queueProfilesValid(listOf(QueueProfileDto(completionAction = "hibernate"))))
        assertFalse(queueProfilesValid(listOf(QueueProfileDto(speedLimitKib = 1_048_577))))
    }
}
