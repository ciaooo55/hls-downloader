package com.hlsdownloader.desktop

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

class MotionPreferencesTest {
    @Test
    fun settingsDecodeSynchronizesReducedMotionOnlyAfterAValidResponse() {
        MotionPreferences.update(false)

        val decoded = decodeEngineSettings(buildJsonObject {
            put("reduce_motion", true)
        })

        assertTrue(decoded.reduceMotion)
        assertTrue(MotionPreferences.reduceMotion.value)
    }

    @Test
    fun reducedMotionSnapsTransitionsInsteadOfChangingNormalTiming() {
        assertEquals(0, motionDurationMillis(true, 180))
        assertEquals(180, motionDurationMillis(false, 180))
        assertEquals(0, motionDurationMillis(false, -25))
        MotionPreferences.update(false)
        assertFalse(MotionPreferences.reduceMotion.value)
    }
}
