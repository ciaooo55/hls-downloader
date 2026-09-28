package com.hlsdownloader.desktop

import androidx.compose.ui.Modifier
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.test.ExperimentalTestApi
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performKeyInput
import androidx.compose.ui.test.runComposeUiTest
import kotlin.test.Test
import kotlin.test.assertFalse
import kotlin.test.assertTrue

/**
 * Esc 必须关掉所有模态弹窗。
 *
 * 回归背景：`Popup` 的 `onDismissRequest` 在桌面端只覆盖"点遮罩/失焦"，
 * 按键不映射到它。修复前这里第二条用例失败（Esc 后弹窗仍在）。
 * 需要图形环境（Linux 上为 Xvfb/DISPLAY），验证命令与证据见
 * `docs/v7-cross-platform-verification-record.md`。
 */
@OptIn(ExperimentalTestApi::class)
class DialogKeyboardDismissTest {
    @Test
    fun escape_dismisses_a_dismissible_workbench_dialog() = runComposeUiTest {
        var dismissed = false
        setContent {
            WorkbenchDialog(
                onDismiss = { dismissed = true },
                title = "标题",
                description = "描述",
                content = { Text("内容", modifier = Modifier.testTag("probe-content")) },
                actions = { DialogSecondary("取消") { } },
            )
        }
        onNodeWithTag("probe-content").assertIsDisplayed()
        // 先点一个真按钮把焦点移进弹窗：预览键事件只沿焦点树向上传播。
        onNodeWithText("取消").performClick()
        waitForIdle()
        onNodeWithText("取消").performKeyInput { keyDown(Key.Escape); keyUp(Key.Escape) }
        waitForIdle()
        assertTrue(dismissed, "Esc 必须关闭可关闭的 WorkbenchDialog")
    }

    @Test
    fun escape_does_not_dismiss_while_the_dialog_is_busy() = runComposeUiTest {
        var dismissed = false
        setContent {
            WorkbenchDialog(
                onDismiss = { dismissed = true },
                title = "标题",
                description = "描述",
                dismissible = false,
                content = { Text("内容", modifier = Modifier.testTag("probe-busy-content")) },
                actions = { DialogPrimary("确定", enabled = false) { } },
            )
        }
        onNodeWithTag("probe-busy-content").assertIsDisplayed()
        onNodeWithText("确定").performClick()
        waitForIdle()
        onNodeWithText("确定").performKeyInput { keyDown(Key.Escape); keyUp(Key.Escape) }
        waitForIdle()
        assertFalse(dismissed, "忙碌态（dismissible=false）弹窗不得被 Esc 关闭")
    }
}
