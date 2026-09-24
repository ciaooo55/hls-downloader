package com.hlsdownloader.desktop

/// How many times a terminal media-push status may be retried before the UI
/// gives up and tells the operator.  The backoff below tops out at 2s, so an
/// unbounded loop kept a coroutine plus pipe traffic alive for the lifetime of
/// the process after the picker that started it was already gone.
internal const val maxMediaPushResolutionAttempts: Int = 30

internal fun mediaPushResolutionRetryDelayMillis(failureCount: Int): Long = when {
    failureCount <= 1 -> 250L
    failureCount == 2 -> 500L
    failureCount == 3 -> 1_000L
    else -> 2_000L
}

internal fun shouldRetryMediaPushResolution(error: Throwable): Boolean =
    (error as? EngineProtocolException)?.code != "media_push_not_found"

internal fun shouldCloseMediaPushPicker(
    activeRequestId: String?,
    resolvedRequestId: String,
    status: String,
): Boolean {
    val terminal = status.lowercase() in setOf("done", "failed", "canceled")
    return terminal && activeRequestId != null && activeRequestId == resolvedRequestId
}
