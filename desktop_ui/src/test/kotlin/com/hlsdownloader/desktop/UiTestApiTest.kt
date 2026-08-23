package com.hlsdownloader.desktop

import java.io.File
import java.awt.event.KeyEvent
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertNotNull
import kotlin.test.assertTrue

class UiTestApiTest {
    @Test
    fun action_validation_rejects_unsafe_or_incomplete_input() {
        assertNull(validateUiTestAction(UiTestAction("activate"), 1024, 600))
        assertNull(validateUiTestAction(UiTestAction("click", 100, 200), 1024, 600))
        assertNull(validateUiTestAction(UiTestAction("right_click", 1023, 599), 1024, 600))
        assertNull(validateUiTestAction(UiTestAction("drag", 10, 20, 300, 400), 1024, 600))
        assertNull(validateUiTestAction(UiTestAction(type = "select_task", index = 4, modifiers = listOf("CTRL")), 1024, 600))
        assertNull(validateUiTestAction(UiTestAction("key", key = "F5"), 1024, 600))
        assertNull(validateUiTestAction(UiTestAction("type", text = "测试 text"), 1024, 600))
        assertEquals("coordinates are outside the current window", validateUiTestAction(UiTestAction("click", 1024, 20), 1024, 600))
        assertEquals("click actions require x and y", validateUiTestAction(UiTestAction("click"), 1024, 600))
        assertEquals("drag actions require x, y, to_x and to_y", validateUiTestAction(UiTestAction("drag", 10, 20), 1024, 600))
        assertEquals("select_task requires a non-negative index", validateUiTestAction(UiTestAction(type = "select_task"), 1024, 600))
        assertEquals("key action requires key", validateUiTestAction(UiTestAction("key"), 1024, 600))
        assertEquals("text exceeds 8192 characters", validateUiTestAction(UiTestAction("type", text = "x".repeat(8193)), 1024, 600))
        assertEquals("unsupported action type", validateUiTestAction(UiTestAction("shell"), 1024, 600))
    }

    @Test
    fun api_is_explicitly_gated_authenticated_and_loopback_only() {
        val source = File("src/main/kotlin/com/hlsdownloader/desktop/UiTestApi.kt").readText()
        val main = File("src/main/kotlin/com/hlsdownloader/desktop/Main.kt").readText()
        val build = File("build.gradle.kts").readText()
        assertTrue(source.contains("System.getenv(\"HLS_UI_TEST_API\") != \"1\""))
        assertTrue(source.contains("token.length >= 16"))
        assertTrue(source.contains("InetAddress.getLoopbackAddress()"))
        assertTrue(source.contains("X-HLS-Test-Token"))
        assertTrue(source.contains("server.createContext(\"/screen\")"))
        assertTrue(source.contains("server.createContext(\"/state\")"))
        assertTrue(source.contains("window.iconImages"))
        assertFalse(source.contains("window.isAlwaysOnTop = true"))
        assertTrue(main.contains("UiTestApi.startIfEnabled(window)"))
        assertTrue(main.contains("transparent = false"))
        assertTrue(build.contains("\"jdk.httpserver\""))
    }

    @Test
    fun selection_state_is_stable_and_sorted_for_api_assertions() {
        UiTestState.updateSelection(setOf("task-c", "task-a", "task-b"))
        assertEquals(3, UiTestState.snapshot().selectedCount)
        assertEquals(listOf("task-a", "task-b", "task-c"), UiTestState.snapshot().selectedTaskIds)
    }

    @Test
    fun app_owned_popup_focus_is_preserved_by_the_test_api() {
        val source = File("src/main/kotlin/com/hlsdownloader/desktop/UiTestApi.kt").readText()
        assertTrue(source.contains("belongsToWindow(active, window)"))
        assertTrue(source.contains("current = current.owner"))
        assertFalse(source.contains("private fun focusWindow() {\n        onEventThread {\n            window.toFront()"))
    }

    @Test
    fun url_characters_have_deterministic_robot_key_strokes() {
        assertEquals(RobotKey(KeyEvent.VK_H), robotKeyForChar('h'))
        assertEquals(RobotKey(KeyEvent.VK_H, true), robotKeyForChar('H'))
        assertEquals(RobotKey(KeyEvent.VK_SEMICOLON, true), robotKeyForChar(':'))
        assertEquals(RobotKey(KeyEvent.VK_SLASH), robotKeyForChar('/'))
        assertEquals(RobotKey(KeyEvent.VK_SLASH, true), robotKeyForChar('?'))
        assertEquals(RobotKey(KeyEvent.VK_7, true), robotKeyForChar('&'))
        assertNull(robotKeyForChar('中'))
    }

    @Test
    fun product_icon_is_packaged_and_toolbar_does_not_repeat_the_brand() {
        val main = File("src/main/kotlin/com/hlsdownloader/desktop/Main.kt").readText()
        val build = File("build.gradle.kts").readText()
        val toolbar = main.substringAfter("@Composable private fun DesktopToolbar").substringBefore("@Composable private fun ToolbarSearchField")
        assertFalse(toolbar.contains("HLS Downloader"))
        assertTrue(main.contains("HLS Downloader ${'$'}{Product.version}"))
        assertTrue(build.contains("app-icon.ico"))
        assertNotNull(javaClass.classLoader.getResource("app-icon.png"))
    }
}
