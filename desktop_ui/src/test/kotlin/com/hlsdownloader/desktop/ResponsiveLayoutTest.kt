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
    fun task_columns_use_the_product_layout_at_every_width() {
        val columns = resolveTaskColumns(834.dp)
        assertEquals(listOf("name", "progress", "status", "speed", "size", "actions"), columns.items.map { it.id })
        assertEquals(225.dp, columns.items.first().width)
        assertTrue(columns.compact)
        assertEquals("name:desc", nextTaskSort("name:asc", "name"))
        assertEquals("size:asc", nextTaskSort("name:desc", "size"))
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
    fun the_settings_dialog_never_shrinks_at_the_minimum_workbench_width() {
        // 最小窗口是 1024dp（window.minimumSize，与采集脚本的 1024x600 下限一致），
        // 而设置弹窗请求 880dp：1024 - 32 = 992 > 880，所以弹窗宽度在任何可达窗口下都是 880dp。
        // 这条是"窄屏设置弹窗要不要改成顶部标签"的判据——内容列恒为 880 - 36 - 138 - 14 = 692dp，
        // 10 个标签横排需要约 900dp 放不下，所以竖排标签列在任何可达尺寸下都不是问题。
        listOf(1024.dp, 1110.dp, 1130.dp, 1400.dp, 1536.dp).forEach { viewport ->
            assertEquals(880.dp, dialogBounds(880.dp, viewport, 820.dp).width, "视口 ${viewport} 下弹窗被压窄了")
        }
    }

    @Test
    fun narrow_workspaces_collapse_the_sidebar_instead_of_dropping_table_columns() {
        // 折叠门槛是推导出来的，不是拍的：表格切精简列的门槛 + 侧栏宽度。
        assertEquals(TASK_TABLE_COMPACT_WIDTH + SIDEBAR_WIDTH, SIDEBAR_COLLAPSE_WIDTH)
        assertEquals(1120.dp, SIDEBAR_COLLAPSE_WIDTH)

        // 门槛之下：留着 190dp 侧栏会让表格跌破 930dp（切精简列），折叠成 56dp 才能回到门槛之上。
        val below = SIDEBAR_COLLAPSE_WIDTH - 10.dp
        assertTrue(resolveTaskColumns(below - SIDEBAR_WIDTH).compact, "不折叠时表格应当已经切精简列")
        assertFalse(resolveTaskColumns(below - SIDEBAR_RAIL_WIDTH).compact, "折叠后表格必须恢复完整列")

        // 门槛之上：折叠是纯损失，所以不折叠。
        val above = SIDEBAR_COLLAPSE_WIDTH + 10.dp
        assertFalse(resolveTaskColumns(above - SIDEBAR_WIDTH).compact)
        assertTrue(SIDEBAR_RAIL_WIDTH < SIDEBAR_WIDTH)
    }

    @Test
    fun the_minimum_window_size_never_exceeds_the_workspace() {
        // 本机工作区 1536×816（1920×1080@125%）⇒ 最小尺寸保持 1024×600，行为不变。
        assertEquals(1024, WindowGeometryStore.effectiveMinimum(1024, 1536))
        assertEquals(600, WindowGeometryStore.effectiveMinimum(600, 816))
        // 无图形环境：只夹下限，不夹上限。
        assertEquals(1024, WindowGeometryStore.effectiveMinimum(1024, null))

        // 工作区比最小尺寸还小的档位。本机只有 125%，这些档位测不到，
        // 只能按逻辑验证：`maximumWindowBounds` 返回逻辑值，所以缩放越大工作区越小。
        assertEquals(911, WindowGeometryStore.effectiveMinimum(1024, 911))   // 1366×768 @150%
        assertEquals(480, WindowGeometryStore.effectiveMinimum(600, 480))
        assertEquals(960, WindowGeometryStore.effectiveMinimum(1024, 960))   // 1920×1080 @200%
        assertEquals(540, WindowGeometryStore.effectiveMinimum(600, 540))
        assertEquals(568, WindowGeometryStore.effectiveMinimum(600, 568))    // 1600×900 @150%
    }

    @Test
    fun a_small_workspace_shrinks_the_window_instead_of_pushing_it_off_screen() {
        // 1366×768@150% ⇒ 工作区 911×480。旧写法（`coerceIn(1024, max(工作区,1024))`）
        // 在这里得到 1024×600：窗口比屏幕还大，右下角含状态栏永远够不到，而且用户缩不回来。
        assertEquals(911 to 480, WindowGeometryStore.clampToWorkArea(1400, 820, 911 to 480))
        assertEquals(911 to 480, WindowGeometryStore.clampToWorkArea(1024, 600, 911 to 480))
        // 1920×1080@200% ⇒ 工作区 960×540：宽高都放不下。
        assertEquals(960 to 540, WindowGeometryStore.clampToWorkArea(1400, 820, 960 to 540))
        // 1600×900@150% ⇒ 工作区 1067×568：宽度够，高度不够。
        assertEquals(1067 to 568, WindowGeometryStore.clampToWorkArea(1400, 820, 1067 to 568))

        // 本机 125% 下行为不变：下限 1024×600，上限工作区（820 > 816 ⇒ 高度夹到 816）。
        assertEquals(1400 to 816, WindowGeometryStore.clampToWorkArea(1400, 820, 1536 to 816))
        assertEquals(1024 to 600, WindowGeometryStore.clampToWorkArea(400, 300, 1536 to 816))
        assertEquals(1536 to 816, WindowGeometryStore.clampToWorkArea(4000, 3000, 1536 to 816))

        // 无图形环境：只夹下限，不夹上限。
        assertEquals(1024 to 600, WindowGeometryStore.clampToWorkArea(400, 300, null))
        assertEquals(4000 to 3000, WindowGeometryStore.clampToWorkArea(4000, 3000, null))
    }

    @Test
    fun the_sidebar_collapse_and_fixture_origin_stay_wired_up() {
        val source = java.io.File("src/main/kotlin/com/hlsdownloader/desktop/Main.kt").readText()
        // 折叠态与展开态共用同一套选中/回调，别让两套导航状态各走各的。
        assertTrue(source.contains("if (compact) {"))
        assertTrue(source.contains("SidebarRail(selected, selectedCategory, selectedQueueId, profiles, tasks"))
        assertTrue(source.contains("maxWidth < SIDEBAR_COLLAPSE_WIDTH"))
        assertTrue(source.contains("Modifier.width(SIDEBAR_RAIL_WIDTH)"))
        // 夹具模式必须锁在 (0,0)：平台默认位置是层叠的，实测出现过 (805,245)，
        // 1110dp 的窗口在那个位置右侧 379dp 出屏，截出来一片黑且不会报错。
        assertTrue(
            source.contains("if (pinnedGeometry) 0 to 0 else WindowGeometryStore.centeredPosition(initialWidth, initialHeight)"),
            "夹具窗口位置又交回平台默认了",
        )
        // 最小窗口尺寸必须按工作区夹过，不能退回写死的 1024×600。
        assertTrue(source.contains("window.minimumSize = Dimension(minWidth, minHeight)"))
        assertFalse(source.contains("window.minimumSize = Dimension(1024, 600)"))
        // 侧栏筛选要能被 /state 读出来：折叠栏把文字换成图标后，
        // "点得中、点得对"光靠截图证明不了，必须有可断言的字段。
        assertTrue(
            source.contains("UiTestState.updateFilter(filter.label)"),
            "侧栏筛选不再上报，折叠栏的点击行为就没法验证了",
        )
        assertTrue(
            source.contains("UiTestState.updateSidebarSelection(category?.label ?: \"\", selectedQueueId ?: \"\")"),
            "分类/队列不再上报，折叠栏另外两组入口的点击行为就没法验证了",
        )
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
        // 断言"用的是真实 Compose 无障碍语义"这件事本身，而不是某一行怎么换行。
        // 原先匹配字面量 `.toggleable(value = checked`：给 toggleable 补 interactionSource
        // 让参数换行之后，它就假失败了 —— 判据被排版绑死，属于假失败，与假通过一样有害。
        assertTrue(components.contains(".toggleable("))
        assertTrue(components.contains("role = Role.Checkbox"))
        assertTrue(components.contains("role = Role.Switch"))
        // 与上面 `.toggleable(` 同一处理：判据钉"用的是真实 Compose 语义"，不钉参数怎么换行。
        // RadioButton 的 selectable 调用补上 interactionSource/indication 后按仓库惯例多行书写
        // （Main.kt:2124、SettingsV7.kt:169、本文件 Checkbox/Switch 全是多行），继续钉单行字面量
        // 会让任何排版调整都变成假失败。role 单独断言，"单选"语义仍然被完整覆盖。
        assertTrue(components.contains(".selectable("))
        assertTrue(components.contains("role = Role.RadioButton"))
        assertTrue(components.contains("progressBarRangeInfo = ProgressBarRangeInfo"))
        assertTrue(components.contains("setProgress { target ->"))
        assertTrue(source.contains("paneTitle = title"))
        assertTrue(source.contains("contentDescription = taskAccessibilityLabel(task)"))
    }

    @Test
    fun startup_retries_without_raw_pipe_text_and_modals_hide_the_workbench_tree() {
        val source = java.io.File("src/main/kotlin/com/hlsdownloader/desktop/Main.kt").readText()
        assertTrue(source.contains("while (!snapshotReady)"))
        assertTrue(source.contains("ENGINE_RECONNECTING_NOTICE"))
        assertTrue(source.contains("EnginePipeClient.ensurePresenterStarted()"))
        assertTrue(source.contains("if (modalVisible) Modifier.clearAndSetSemantics"))
        assertFalse(source.contains("LegalAgreementDialog"))
        assertFalse(source.contains("error.message ?: \"下载引擎连接失败\""))
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

    @Test
    fun settings_visual_fixtures_open_the_requested_page_without_pointer_input() {
        assertEquals("通用", auditSettingsTab("settings"))
        assertEquals("下载", auditSettingsTab("settings_download"))
        assertEquals("网络", auditSettingsTab("settings_network"))
        assertEquals("投屏与推送", auditSettingsTab("settings_devices"))
        assertEquals("外观", auditSettingsTab("settings_appearance"))
        assertEquals(null, auditSettingsTab("tasks_1000"))
    }
}
