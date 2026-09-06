package com.hlsdownloader.desktop

import androidx.compose.runtime.State
import androidx.compose.runtime.mutableStateOf
import kotlinx.serialization.json.JsonObject

/**
 * UI-only projection of the Core-owned reduced-motion setting.
 *
 * The Rust Core remains the single settings owner. This state is refreshed only
 * after a settings response has been successfully decoded, so a failed save
 * never changes local motion behavior ahead of persisted Core state.
 */
internal object MotionPreferences {
    private val reducedMotion = mutableStateOf(false)

    val reduceMotion: State<Boolean>
        get() = reducedMotion

    fun update(value: Boolean) {
        reducedMotion.value = value
    }
}

internal fun decodeEngineSettings(response: JsonObject): EngineSettingsDto =
    protocolJson.decodeFromJsonElement(EngineSettingsDto.serializer(), response).also {
        MotionPreferences.update(it.reduceMotion)
    }

internal fun motionDurationMillis(reduceMotion: Boolean, normalMillis: Int): Int =
    if (reduceMotion) 0 else normalMillis.coerceAtLeast(0)
