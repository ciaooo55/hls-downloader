package com.hlsdownloader.desktop

import androidx.compose.ui.unit.dp
import java.awt.event.KeyEvent
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class ResponsiveLayoutTest {
    @Test
    fun compact_task_columns_fit_the_1024_workbench_content_area() {
        val columns = taskColumnsForWidth(834.dp)
        assertTrue(columns.compact)
        assertTrue(columns.requiredWidth <= 834.dp)
        assertTrue(columns.actions >= 42.dp)
    }

    @Test
    fun wide_task_columns_preserve_the_full_density_layout() {
        val columns = taskColumnsForWidth(1210.dp)
        assertFalse(columns.compact)
        assertTrue(columns.name >= 280.dp)
        assertTrue(columns.progress >= 220.dp)
    }

    @Test
    fun task_rows_keep_the_real_file_suffix_visible_in_metadata() {
        assertEquals(".mp4", taskExtensionLabel(TaskDto("video", "movie.mp4", status = "done")))
        assertEquals(".m3u8", taskExtensionLabel(TaskDto("live", "live-stream", status = "进行中", resourceKind = "hls")))
        assertEquals(".torrent", taskExtensionLabel(TaskDto("torrent", "download", status = "排队中", resourceKind = "torrent")))
    }

    @Test
    fun dialogs_shrink_inside_narrow_and_short_workspaces() {
        val compact = dialogBounds(880.dp, 640.dp, 480.dp)
        assertEquals(608.dp, compact.width)
        assertEquals(448.dp, compact.maxHeight)
        val normal = dialogBounds(520.dp, 1024.dp, 600.dp)
        assertEquals(520.dp, normal.width)
        assertEquals(568.dp, normal.maxHeight)
    }

    @Test
    fun displayed_resource_locations_hide_credentials_and_signed_parameters() {
        val shown = safeResourceLocation("https://name:secret@cdn.example.test/media/1080/movie.mp4?token=private&expires=9#track")
        assertEquals("cdn.example.test/1080/movie.mp4", shown)
        assertFalse(shown.contains("secret"))
        assertFalse(shown.contains("token"))
        assertEquals("magnet · BT 资源", safeResourceLocation("magnet:?xt=urn:btih:private"))
    }

    @Test
    fun completed_file_preview_only_accepts_supported_image_suffixes() {
        assertTrue(isPreviewableImage("C:\\Downloads\\capture.PNG"))
        assertTrue(isPreviewableImage("C:\\Downloads\\photo.webp"))
        assertFalse(isPreviewableImage("C:\\Downloads\\archive.zip"))
        assertFalse(isPreviewableImage("C:\\Downloads\\image.png.exe"))
    }

    @Test
    fun desktop_shortcuts_cover_the_high_frequency_workflow() {
        assertEquals("new", workbenchShortcut(ctrl = true, shift = false, KeyEvent.VK_N))
        assertEquals("batch", workbenchShortcut(ctrl = true, shift = true, KeyEvent.VK_N))
        assertEquals("settings", workbenchShortcut(ctrl = true, shift = false, KeyEvent.VK_COMMA))
        assertEquals("refresh", workbenchShortcut(ctrl = false, shift = false, KeyEvent.VK_F5))
        assertEquals("escape", workbenchShortcut(ctrl = false, shift = false, KeyEvent.VK_ESCAPE))
    }

    @Test
    fun desktop_shortcut_dispatcher_reads_the_latest_dialog_state() {
        val source = java.io.File("src/main/kotlin/com/hlsdownloader/desktop/Main.kt").readText()
        assertTrue(source.contains("rememberUpdatedState<(String?) -> Boolean>"))
        assertTrue(source.contains("currentShortcutHandler.value("))
        assertTrue(source.contains("KeyboardFocusManager.getCurrentKeyboardFocusManager()"))
    }

    @Test
    fun desktop_runtime_uses_the_real_compose_accessibility_contract() {
        val source = java.io.File("src/main/kotlin/com/hlsdownloader/desktop/Main.kt").readText()
        val components = java.io.File("src/main/kotlin/com/hlsdownloader/desktop/WorkbenchComponents.kt").readText()
        val build = java.io.File("build.gradle.kts").readText()
        assertTrue(source.contains("compose.accessibility.enable"))
        assertFalse(source.contains("compose.accessibility.enabled"))
        assertTrue(source.contains("javax.accessibility.assistive_technologies"))
        assertTrue(build.contains("com.sun.java.accessibility.AccessBridge"))
        assertTrue(build.contains("\"jdk.accessibility\""))
        assertTrue(build.contains("\"jdk.httpserver\""))
        assertTrue(components.contains(".toggleable(value = checked"))
        assertTrue(components.contains(".selectable(selected = selected"))
        assertTrue(components.contains("progressBarRangeInfo = ProgressBarRangeInfo"))
        assertTrue(components.contains("setProgress { target ->"))
        assertTrue(source.contains("paneTitle = title"))
        assertTrue(source.contains("contentDescription = taskAccessibilityLabel(task)"))
    }

    @Test
    fun task_accessibility_label_exposes_the_scannable_row_state() {
        val source = TaskDto(
            id = "task-1",
            filename = "movie.mp4",
            status = "downloading",
            totalBytes = 10L * 1024 * 1024,
            url = "https://example.com/movie.mp4",
        )
        val task = DownloadTask("task-1", "movie.mp4", "进行中", .42f, "2.00 MB/s", 2L * 1024 * 1024, "3 秒", "4 分段", "刚刚", source)
        assertEquals("movie.mp4，HTTP .mp4，进行中，进度 42%，速度 2.00 MB/s，大小 10.00 MB", taskAccessibilityLabel(task))
    }

    @Test
    fun settings_content_keeps_a_dedicated_scroll_viewport() {
        val source = java.io.File("src/main/kotlin/com/hlsdownloader/desktop/SettingsV7.kt").readText()
        assertTrue(source.contains("VerticalScrollbar"))
        assertTrue(source.contains("contentScroll"))
        assertTrue(source.contains("LaunchedEffect(selected) { contentScroll.scrollTo(0) }"))
    }
}
