from pathlib import Path

main = Path("desktop_ui/src/main/kotlin/com/hlsdownloader/desktop/Main.kt")
text = main.read_text(encoding="utf-8")
old = '''            if (result.isSuccess) return
            failures++
            if (failures == 1) {
                result.exceptionOrNull()?.let { UiDiagnostics.error("media_push.resolve_terminal", it, requestId) }
                notice = UiSignal.Notice("error", "投送结果已确定，但浏览器状态同步失败，正在自动重试")
            }
            delay(mediaPushResolutionRetryDelayMillis(failures))'''
new = '''            if (result.isSuccess) return
            val error = result.exceptionOrNull() ?: return
            if (!shouldRetryMediaPushResolution(error)) {
                UiDiagnostics.warning(
                    "media_push.resolve_terminal_rejected",
                    error.message ?: "媒体推送终态同步被下载引擎拒绝",
                    requestId = requestId,
                )
                notice = UiSignal.Notice(
                    "error",
                    if ((error as? EngineProtocolException)?.code == "media_push_not_found") {
                        "投送请求已过期，浏览器端将按失败处理"
                    } else {
                        "投送状态同步被下载引擎拒绝"
                    },
                )
                return
            }
            failures++
            if (failures == 1) {
                UiDiagnostics.error("media_push.resolve_terminal", error, requestId = requestId)
                notice = UiSignal.Notice("error", "投送结果已确定，但浏览器状态同步失败，正在自动重试")
            }
            delay(mediaPushResolutionRetryDelayMillis(failures))'''
if text.count(old) != 1:
    raise SystemExit(f"Main retry anchor count={text.count(old)}")
main.write_text(text.replace(old, new, 1), encoding="utf-8")

state = Path("desktop_ui/src/main/kotlin/com/hlsdownloader/desktop/MediaPushUiState.kt")
text = state.read_text(encoding="utf-8")
marker = '''internal fun mediaPushResolutionRetryDelayMillis(failureCount: Int): Long = when {
    failureCount <= 1 -> 250L
    failureCount == 2 -> 500L
    failureCount == 3 -> 1_000L
    else -> 2_000L
}
'''
addition = marker + '''
internal fun shouldRetryMediaPushResolution(error: Throwable): Boolean {
    val code = (error as? EngineProtocolException)?.code ?: return true
    return code !in setOf("media_push_not_found", "media_push_status_invalid")
}
'''
if text.count(marker) != 1:
    raise SystemExit(f"MediaPushUiState anchor count={text.count(marker)}")
state.write_text(text.replace(marker, addition, 1), encoding="utf-8")

test = Path("desktop_ui/src/test/kotlin/com/hlsdownloader/desktop/MediaPushUiStateTest.kt")
text = test.read_text(encoding="utf-8")
marker = '''    @Test
    fun terminal_sync_retry_backoff_caps_quickly() {'''
addition = '''    @Test
    fun terminal_sync_stops_retrying_definitive_core_rejections() {
        assertFalse(shouldRetryMediaPushResolution(EngineProtocolException("media_push_not_found", "expired")))
        assertFalse(shouldRetryMediaPushResolution(EngineProtocolException("media_push_status_invalid", "invalid")))
        assertTrue(shouldRetryMediaPushResolution(EngineProtocolException("engine_busy", "busy")))
        assertTrue(shouldRetryMediaPushResolution(IllegalStateException("pipe closed")))
    }

'''
if text.count(marker) != 1:
    raise SystemExit(f"MediaPushUiStateTest anchor count={text.count(marker)}")
test.write_text(text.replace(marker, addition + marker, 1), encoding="utf-8")
