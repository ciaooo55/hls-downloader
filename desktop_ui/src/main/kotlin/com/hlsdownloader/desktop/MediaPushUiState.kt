package com.hlsdownloader.desktop

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
