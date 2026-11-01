package com.hlsdownloader.desktop

import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.test.ExperimentalTestApi
import androidx.compose.ui.test.SemanticsMatcher
import androidx.compose.ui.test.hasClickAction
import androidx.compose.ui.test.hasContentDescription
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performMouseInput
import androidx.compose.ui.test.v2.runSkikoComposeUiTest
import org.junit.jupiter.api.condition.EnabledIfEnvironmentVariable
import kotlin.test.Test

/** Real pointer dispatch in an offscreen scene; never creates or controls an OS window. */
@OptIn(ExperimentalTestApi::class)
@EnabledIfEnvironmentVariable(named = "HLS_UI_AUDIT_SURFACE", matches = "tasks_1000")
class WorkbenchPointerInputTest {
    @Test
    fun first_click_opens_and_closes_dialogs_in_wide_layout() = checkFirstClicks(1400)

    @Test
    fun first_click_opens_and_closes_dialogs_in_compact_layout() = checkFirstClicks(1024)

    private fun checkFirstClicks(width: Int) = runSkikoComposeUiTest(size = Size(width.toFloat(), 820f)) {
        mainClock.autoAdvance = false
        setContent { AppShell() }

        fun clickControl(matcher: SemanticsMatcher, edge: Boolean = false) {
            val node = onNode(matcher and hasClickAction())
            node.performMouseInput { moveTo(if (edge) Offset(.25f, center.y) else center) }
            mainClock.advanceTimeBy(600) // Include a tooltip if the control has one.
            waitForIdle()
            node.performMouseInput { press() }
            mainClock.advanceTimeBy(150)
            waitForIdle()
            node.performMouseInput { moveBy(Offset(.1f, 0f)); release() }
            mainClock.advanceTimeBy(250)
            waitForIdle()
        }

        repeat(2) {
            clickControl(hasText("新建"), edge = true)
            onNodeWithText("新建下载").assertExists()
            clickControl(hasText("取消"), edge = true)
            onNodeWithText("新建下载").assertDoesNotExist()

            clickControl(hasText("设置") or hasContentDescription("设置"), edge = true)
            onNodeWithText("保存设置").assertExists()
            clickControl(hasText("取消"), edge = true)
            onNodeWithText("保存设置").assertDoesNotExist()

            clickControl(hasText("工具"), edge = true)
            clickControl(hasText("批量添加"))
            onNodeWithText("每行一个下载链接，请修正无效链接后创建").assertExists()
            clickControl(hasContentDescription("关闭"))
            onNodeWithText("每行一个下载链接，请修正无效链接后创建").assertDoesNotExist()
        }
    }
}
