package com.hlsdownloader.desktop

import kotlin.test.Test
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class UiDiagnosticsTest {
    @Test
    fun diagnosticMessageDoesNotPersistRequestSecrets() {
        val cleaned = UiDiagnostics.clean("HTTP 403 https://user:password@cdn.test/media?signature=private\nAuthorization: Bearer private-key\nCookie: sid=private-cookie", 2000)
        for (secret in listOf("user:password", "signature=private", "private-key", "private-cookie")) {
            assertFalse(cleaned.contains(secret))
        }
        assertTrue(cleaned.contains("HTTP 403"))
    }
}
