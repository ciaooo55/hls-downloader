package com.hlsdownloader.desktop

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class TaskFailureUiTest {
    @Test
    fun structuredFailureShowsV3DiagnosticFieldsAndActionableSteps() {
        val source = TaskDto(
            id = "task-404",
            filename = "missing.mp4",
            status = "failed",
            url = "https://cdn.test/missing.mp4?token=secret",
            errorCode = "HTTP_404",
            errorMessage = "HTTP 404 for https://cdn.test/missing.mp4?token=secret",
            errorStage = "transfer",
            errorUrl = "https://cdn.test/missing.mp4?token=secret",
            errorHint = "重新识别资源",
            httpStatus = 404,
            errorAttempt = 2,
        )
        val details = taskFailureDetails(source)!!
        assertEquals("下载失败 · HTTP_404", details.title)
        assertTrue(details.items.contains("发生阶段" to "下载文件"))
        assertTrue(details.items.contains("HTTP 状态" to "404"))
        assertTrue(details.items.contains("尝试次数" to "2 次"))
        assertTrue(details.steps.any { it.contains("重新识别") })
    }

    @Test
    fun copiedDiagnosticsRedactQueryCredentials() {
        val source = TaskDto(
            id = "task-secret",
            filename = "video.mp4",
            status = "failed",
            url = "https://cdn.test/video.mp4?token=private",
            errorCode = "HTTP_403",
            errorMessage = "HTTP 403 https://cdn.test/video.mp4?token=private",
            errorUrl = "https://cdn.test/video.mp4?token=private",
            httpStatus = 403,
            logTail = listOf("failed https://cdn.test/video.mp4?token=private"),
        )
        val diagnostic = taskFailureDiagnostic(downloadTask(source))
        assertTrue(diagnostic.contains("HTTP 状态: 403"))
        assertFalse(diagnostic.contains("token=private"))
        assertFalse(diagnostic.contains("?"))
    }
}
