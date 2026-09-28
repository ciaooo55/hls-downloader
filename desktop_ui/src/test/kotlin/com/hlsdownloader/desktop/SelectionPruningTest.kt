package com.hlsdownloader.desktop

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertSame
import kotlin.test.assertTrue

/**
 * 选择集在"可见集变化"后的收敛规则（`selectionAfterVisibleSetChange`）。
 *
 * 背景是一个真实缺陷：`selected` 从不随筛选收敛，于是
 * - 表头的"N 已选"读 `selected.isNotEmpty()`，屏幕上却一行都没选中；
 * - 批量删除走 `tasks.filter { it.id in selected }`，会命中被筛掉的任务。
 * Linux 上连点 全部→进行中→排队中→已暂停→已完成→失败，`selectedCount` 一直是 4
 * （见 `docs/v7-iteration-log.md` 第三十四轮）。
 *
 * 同时盯住两条容易在重构里悄悄改变的边界：可见集为空时不动选择，
 * 以及没有变化时必须返回同一实例（否则每次列表刷新都会触发一次多余重组）。
 */
class SelectionPruningTest {
    @Test
    fun selection_is_pruned_to_the_visible_rows() {
        val pruned = selectionAfterVisibleSetChange(
            setOf("a", "b", "c"),
            setOf("b", "c", "d"),
            3,
        )
        assertEquals(setOf("b", "c"), pruned)
    }

    @Test
    fun selection_can_become_empty_when_nothing_visible_is_selected() {
        val pruned = selectionAfterVisibleSetChange(setOf("a", "b"), setOf("c"), 1)
        assertTrue(pruned.isEmpty())
    }

    @Test
    fun an_empty_selection_is_returned_untouched() {
        val empty = emptySet<String>()
        assertSame(empty, selectionAfterVisibleSetChange(empty, setOf("a"), 1))
    }

    @Test
    fun an_empty_visible_set_keeps_the_selection() {
        // 筛选无结果 / 列表正在重载：此时清空只会让用户丢选择。
        val selected = setOf("a", "b")
        assertSame(selected, selectionAfterVisibleSetChange(selected, emptySet(), 0))
    }

    @Test
    fun an_unchanged_selection_returns_the_same_instance() {
        // 进度变化同样会重建 visible；返回同一实例，调用方跳过写入，避免空转重组。
        val selected = setOf("a", "b")
        assertSame(selected, selectionAfterVisibleSetChange(selected, setOf("a", "b", "z"), 3))
    }

    @Test
    fun pruning_keeps_at_most_the_visible_count() {
        val pruned = selectionAfterVisibleSetChange(setOf("a", "b", "c", "d"), setOf("a"), 1)
        assertEquals(1, pruned.size)
        assertEquals(setOf("a"), pruned)
    }
}
