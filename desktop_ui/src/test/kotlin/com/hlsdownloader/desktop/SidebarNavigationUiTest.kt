package com.hlsdownloader.desktop

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.requiredSize
import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.test.ExperimentalTestApi
import androidx.compose.ui.test.SemanticsMatcher
import androidx.compose.ui.test.hasClickAction
import androidx.compose.ui.test.hasContentDescription
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performSemanticsAction
import androidx.compose.ui.test.runComposeUiTest
import androidx.compose.ui.unit.dp
import org.junit.jupiter.api.condition.EnabledIfEnvironmentVariable
import kotlin.test.Test
import kotlin.test.assertEquals

/** 使用现有离线任务夹具，直接调用 Compose 语义动作，不注入 Windows 鼠标。 */
@OptIn(ExperimentalTestApi::class)
@EnabledIfEnvironmentVariable(named = "HLS_UI_AUDIT_SURFACE", matches = "tasks_1000")
class SidebarNavigationUiTest {
    @Test
    fun switching_navigation_groups_clears_the_previous_group_in_both_layouts() = runComposeUiTest {
        // 运行中任务含持续动画；冻结测试时钟，不能把“动画永不空闲”误判成导航卡死。
        mainClock.autoAdvance = false
        val width = mutableStateOf(1400)
        setContent {
            Box(Modifier.requiredSize(width.value.dp, 820.dp)) { AppShell() }
        }
        val navigationTab = hasClickAction() and SemanticsMatcher.expectValue(SemanticsProperties.Role, Role.Tab)

        for (size in listOf(1400, 1024)) {
            width.value = size
            mainClock.advanceTimeByFrame()
            waitForIdle()
            onNode((hasText("失败") or hasContentDescription("失败")) and navigationTab).performSemanticsAction(SemanticsActions.OnClick) { it() }
            mainClock.advanceTimeByFrame()
            waitForIdle()
            assertEquals("失败", UiTestState.snapshot().activeFilter)

            onNode((hasText("媒体") or hasContentDescription("媒体")) and navigationTab).performSemanticsAction(SemanticsActions.OnClick) { it() }
            mainClock.advanceTimeByFrame()
            waitForIdle()
            assertEquals("全部", UiTestState.snapshot().activeFilter)
            assertEquals("媒体", UiTestState.snapshot().activeCategory)
            // 夹具有 400 项媒体；旧代码保留“失败”筛选时这里为空。
            onNodeWithText("400 项").assertExists()

            if (size == 1400) {
                onNodeWithText("下载队列").performSemanticsAction(SemanticsActions.OnClick) { it() }
                mainClock.advanceTimeByFrame()
            }
            onNode((hasText("默认队列") or hasContentDescription("默认队列")) and navigationTab).performSemanticsAction(SemanticsActions.OnClick) { it() }
            mainClock.advanceTimeByFrame()
            waitForIdle()
            assertEquals("全部", UiTestState.snapshot().activeFilter)
            assertEquals("", UiTestState.snapshot().activeCategory)
            assertEquals("default", UiTestState.snapshot().activeQueueId)

            onNode((hasText("媒体") or hasContentDescription("媒体")) and navigationTab).performSemanticsAction(SemanticsActions.OnClick) { it() }
            mainClock.advanceTimeByFrame()
            waitForIdle()
            assertEquals("", UiTestState.snapshot().activeQueueId)
            assertEquals("媒体", UiTestState.snapshot().activeCategory)

            onNode((hasText("失败") or hasContentDescription("失败")) and navigationTab).performSemanticsAction(SemanticsActions.OnClick) { it() }
            mainClock.advanceTimeByFrame()
            waitForIdle()
            assertEquals("", UiTestState.snapshot().activeCategory)
            assertEquals("失败", UiTestState.snapshot().activeFilter)
        }
    }
}
