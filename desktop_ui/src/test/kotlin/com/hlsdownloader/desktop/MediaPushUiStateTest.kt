package com.hlsdownloader.desktop

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class MediaPushUiStateTest {
    @Test
    fun terminal_resolution_closes_only_the_matching_active_picker() {
        assertTrue(shouldCloseMediaPushPicker("media-push-a", "media-push-a", "failed"))
        assertTrue(shouldCloseMediaPushPicker("media-push-a", "media-push-a", "done"))
        assertTrue(shouldCloseMediaPushPicker("media-push-a", "media-push-a", "CANCELED"))
        assertFalse(shouldCloseMediaPushPicker("media-push-b", "media-push-a", "failed"))
        assertFalse(shouldCloseMediaPushPicker(null, "media-push-a", "failed"))
        assertFalse(shouldCloseMediaPushPicker("media-push-a", "media-push-a", "pending"))
    }

    @Test
    fun terminal_sync_retries_unless_the_durable_request_is_gone() {
        assertFalse(shouldRetryMediaPushResolution(EngineProtocolException("media_push_not_found", "expired")))
        assertTrue(shouldRetryMediaPushResolution(EngineProtocolException("media_push_status_invalid", "invalid")))
        assertTrue(shouldRetryMediaPushResolution(EngineProtocolException("engine_busy", "busy")))
        assertTrue(shouldRetryMediaPushResolution(IllegalStateException("pipe closed")))
    }

    @Test
    fun terminal_sync_retry_backoff_caps_quickly() {
        assertEquals(250L, mediaPushResolutionRetryDelayMillis(1))
        assertEquals(500L, mediaPushResolutionRetryDelayMillis(2))
        assertEquals(1_000L, mediaPushResolutionRetryDelayMillis(3))
        assertEquals(2_000L, mediaPushResolutionRetryDelayMillis(4))
        assertEquals(2_000L, mediaPushResolutionRetryDelayMillis(20))
    }
}
