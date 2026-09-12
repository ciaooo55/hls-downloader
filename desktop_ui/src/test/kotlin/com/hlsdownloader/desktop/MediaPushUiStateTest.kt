package com.hlsdownloader.desktop

import kotlin.test.Test
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
}
