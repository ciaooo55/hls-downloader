package com.hlsdownloader.desktop

import java.nio.file.Files
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFails
import kotlin.test.assertTrue

class TaskExportFileTest {
    @Test
    fun replacesExistingExportAndCleansStageOnFailure() {
        val root = Files.createTempDirectory("hls-task-export-")
        try {
            val target = root.resolve("tasks.json")
            Files.writeString(target, "previous")
            writeTaskExport(target, "new list")
            assertEquals("new list", Files.readString(target))
            val occupied = Files.createDirectory(root.resolve("occupied"))
            Files.writeString(occupied.resolve("keep.txt"), "user file")
            assertFails { writeTaskExport(occupied, "must not replace directory") }
            assertEquals("user file", Files.readString(occupied.resolve("keep.txt")))
            Files.list(root).use { entries -> assertTrue(entries.noneMatch { it.fileName.toString().startsWith(".hls-export-") }) }
        } finally {
            root.toFile().deleteRecursively()
        }
    }
}
