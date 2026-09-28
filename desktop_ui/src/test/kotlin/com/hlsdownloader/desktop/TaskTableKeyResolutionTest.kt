package com.hlsdownloader.desktop

import androidx.compose.ui.input.key.Key
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull

/**
 * 任务表键盘模型的映射表。
 *
 * 这些分支此前只存在于 `TaskTable` 的 `onPreviewKeyEvent` lambda 里，无法单测；
 * 本机 Xvfb 无窗口管理器，"在真机上按一遍"也做不到（Robot 按键无投递，
 * 见 `docs/v7-cross-platform-verification-record.md` 第 15 项）。因此映射被抽成
 * `resolveTaskTableKeyCommand` 纯函数，由本文件逐分支断言。
 *
 * 覆盖的是"哪个键产生哪个命令"，不含副作用本身（副作用仍由各回调执行）。
 */
class TaskTableKeyResolutionTest {
    @Test
    fun ctrl_a_selects_every_row() {
        assertEquals(TaskTableKeyCommand.SelectAll, resolveTaskTableKeyCommand(Key.A, ctrl = true, selectedCount = 0, taskCount = 12))
    }

    @Test
    fun ctrl_a_still_consumed_on_an_empty_table() {
        // 0 行时也必须消费该键：原实现走这一分支并把选择与锚点清空。
        assertEquals(TaskTableKeyCommand.SelectAll, resolveTaskTableKeyCommand(Key.A, ctrl = true, selectedCount = 3, taskCount = 0))
    }

    @Test
    fun plain_a_is_not_mine() {
        assertNull(resolveTaskTableKeyCommand(Key.A, ctrl = false, selectedCount = 0, taskCount = 12))
    }

    @Test
    fun escape_clears_only_when_something_is_selected() {
        assertEquals(TaskTableKeyCommand.ClearSelection, resolveTaskTableKeyCommand(Key.Escape, ctrl = false, selectedCount = 1, taskCount = 12))
        assertNull(resolveTaskTableKeyCommand(Key.Escape, ctrl = false, selectedCount = 0, taskCount = 12))
    }

    @Test
    fun delete_deletes_only_when_something_is_selected() {
        assertEquals(TaskTableKeyCommand.DeleteSelection, resolveTaskTableKeyCommand(Key.Delete, ctrl = false, selectedCount = 2, taskCount = 12))
        assertNull(resolveTaskTableKeyCommand(Key.Delete, ctrl = false, selectedCount = 0, taskCount = 12))
    }

    @Test
    fun enter_opens_details_only_when_something_is_selected() {
        assertEquals(TaskTableKeyCommand.OpenDetails, resolveTaskTableKeyCommand(Key.Enter, ctrl = false, selectedCount = 1, taskCount = 12))
        assertNull(resolveTaskTableKeyCommand(Key.Enter, ctrl = false, selectedCount = 0, taskCount = 12))
    }

    @Test
    fun arrows_move_only_when_the_table_has_rows() {
        assertEquals(TaskTableKeyCommand.MoveSelection, resolveTaskTableKeyCommand(Key.DirectionDown, ctrl = false, selectedCount = 0, taskCount = 12))
        assertEquals(TaskTableKeyCommand.MoveSelection, resolveTaskTableKeyCommand(Key.DirectionUp, ctrl = false, selectedCount = 0, taskCount = 12))
        assertNull(resolveTaskTableKeyCommand(Key.DirectionDown, ctrl = false, selectedCount = 1, taskCount = 0))
        assertNull(resolveTaskTableKeyCommand(Key.DirectionUp, ctrl = false, selectedCount = 1, taskCount = 0))
    }

    @Test
    fun ctrl_only_matters_for_select_all() {
        // Ctrl+Esc / Ctrl+Delete 与不带 Ctrl 时同义：Ctrl 不是全局修饰键。
        assertEquals(TaskTableKeyCommand.ClearSelection, resolveTaskTableKeyCommand(Key.Escape, ctrl = true, selectedCount = 1, taskCount = 12))
        assertEquals(TaskTableKeyCommand.DeleteSelection, resolveTaskTableKeyCommand(Key.Delete, ctrl = true, selectedCount = 1, taskCount = 12))
    }

    @Test
    fun unrelated_keys_fall_through_to_the_focus_chain() {
        assertNull(resolveTaskTableKeyCommand(Key.F5, ctrl = false, selectedCount = 1, taskCount = 12))
        assertNull(resolveTaskTableKeyCommand(Key.Spacebar, ctrl = false, selectedCount = 1, taskCount = 12))
        assertNull(resolveTaskTableKeyCommand(Key.Tab, ctrl = false, selectedCount = 1, taskCount = 12))
        // 文本键不得被表格吃掉：输入框聚焦时方向左右仍不属于本表。
        assertNull(resolveTaskTableKeyCommand(Key.DirectionLeft, ctrl = false, selectedCount = 1, taskCount = 12))
        assertNull(resolveTaskTableKeyCommand(Key.DirectionRight, ctrl = false, selectedCount = 1, taskCount = 12))
    }

    @Test
    fun shift_is_not_part_of_the_command_itself() {
        // Shift+下移 与下移 同为 MoveSelection；区间选择语义在调用方读 event.isShiftPressed。
        assertEquals(TaskTableKeyCommand.MoveSelection, resolveTaskTableKeyCommand(Key.DirectionDown, ctrl = false, selectedCount = 1, taskCount = 12))
    }
}
