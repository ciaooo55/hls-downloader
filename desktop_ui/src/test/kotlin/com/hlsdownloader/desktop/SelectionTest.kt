package com.hlsdownloader.desktop

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class SelectionTest {
    private val tasks = listOf("a", "b", "c", "d", "e")

    @Test fun plainClickReplacesSelection() {
        assertEquals(setOf("c"), selectionAfterClick(tasks, setOf("a", "b"), "b", "c", shift = false, toggle = false))
    }

    @Test fun ctrlClickTogglesOneRow() {
        assertEquals(setOf("a", "c"), selectionAfterClick(tasks, setOf("a"), "a", "c", shift = false, toggle = true))
        assertEquals(setOf("a"), selectionAfterClick(tasks, setOf("a", "c"), "c", "c", shift = false, toggle = true))
    }

    @Test fun shiftClickReplacesWithContiguousRange() {
        assertEquals(setOf("b", "c", "d"), selectionAfterClick(tasks, setOf("a", "e"), "b", "d", shift = true, toggle = false))
    }

    @Test fun ctrlShiftClickAddsContiguousRange() {
        assertEquals(setOf("a", "b", "c", "d"), selectionAfterClick(tasks, setOf("a"), "b", "d", shift = true, toggle = true))
    }

    @Test fun leftDragSelectsEveryCrossedRow() {
        assertEquals(setOf("b", "c", "d"), selectionAfterDrag(tasks, 3, 1, emptySet(), additive = false))
    }

    @Test fun ctrlLeftDragAddsToExistingSelection() {
        assertEquals(setOf("a", "c", "d"), selectionAfterDrag(tasks, 2, 3, setOf("a"), additive = true))
    }

    @Test fun archiveActionsNeverExposeMediaCommands() {
        val archive = TaskDto(id = "archive", filename = "release.zip", status = "completed", playbackReady = true, availableActions = listOf("open", "launch", "play", "cast", "push_tvbox"))
        val actions = taskMenuActions(archive)
        assertTrue("open" in actions)
        assertFalse("launch" in actions)
        assertFalse(actions.any { it in setOf("play", "cast", "push_tvbox") })
    }

    @Test fun playableMediaActionsRemainAvailable() {
        val media = TaskDto(id = "media", filename = "movie.mp4", status = "completed", playbackReady = true, availableActions = listOf("details", "open"))
        val actions = taskMenuActions(media)
        assertTrue(actions.containsAll(listOf("play", "cast", "push_tvbox")))
        assertFalse("details" in actions)
    }

    @Test fun executableFilesKeepLaunchAction() {
        val program = TaskDto(id = "program", filename = "setup.exe", status = "completed", availableActions = listOf("open", "launch"))
        assertTrue("launch" in taskMenuActions(program))
    }
}
