package com.hlsdownloader.desktop

import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.test.ExperimentalTestApi
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.runComposeUiTest
import kotlin.test.Test
import kotlin.test.assertEquals

/**
 * `/state` 的镜子（`UiTestState.snapshot()`）必须在选择/筛选被写入后跟着更新。
 *
 * 回归背景是一个真实缺陷：镜子原本是挂在 `AppShell` 里的三个 `SideEffect`，
 * 而 `selected` 只在内层的 `TaskTable(...)` 调用点被读取 ⇒ 外层作用域不会因它
 * 失效，SideEffect 在包含它的作用域之外再也不触发，`/state` 的 selectedCount
 * 于是永远停在 0。Linux 上实测复现：点中任务行 1 秒后仍报 selected=0，行已高亮。
 * 修法见 `UiTestStateMirror`，以及 `ResponsiveLayoutTest` 里的布线断言。
 *
 * 这里断言的形状：**写入发生在镜子自己的作用域之外**（对应真实代码里写入来自
 * `onSelection` 回调），镜子必须仍然跟得上。写成从测试侧直接改状态、而不是伪造
 * 一次点击，是因为点击派发在本机的 Compose UI 测试环境里并不可靠
 * （同一棵树上 filter 按钮的点击能派发、select 按钮的点击不能，属测试环境问题），
 * 而缺陷机制与"谁来写"无关，只与"作用域是否失效"有关。
 */
@OptIn(ExperimentalTestApi::class)
class UiStateMirrorEffectTest {
    @Test
    fun state_mirror_follows_values_written_outside_its_own_scope() = runComposeUiTest {
        val selected = mutableStateOf<Set<String>>(emptySet())
        val filterLabel = mutableStateOf("全部")
        setContent {
            UiTestStateMirror(selected.value, 12, filterLabel.value, "", null)
            Text("rows=${selected.value.size}")
        }
        waitForIdle()
        assertEquals(0, UiTestState.snapshot().selectedCount, "初始选择必须为空")

        // 写入发生在镜子之外（对应 AppShell 的 onSelection 回调）。
        selected.value = setOf("audit-task-0", "audit-task-5")
        waitForIdle()
        val snapshot = UiTestState.snapshot()
        assertEquals(2, snapshot.selectedCount, "外部写入选择后 /state 必须跟得上")
        assertEquals(listOf("audit-task-0", "audit-task-5"), snapshot.selectedTaskIds)
        onNodeWithText("rows=2").assertExists()

        filterLabel.value = "已暂停"
        waitForIdle()
        assertEquals("已暂停", UiTestState.snapshot().activeFilter, "筛选标签必须能读出来")

        // 清空同样要跟得上（Esc 清选、点遮罩清选都走这条路）。
        selected.value = emptySet()
        waitForIdle()
        assertEquals(0, UiTestState.snapshot().selectedCount, "外部清空选择后 /state 必须跟得上")
        onNodeWithText("rows=0").assertExists()
    }
}
