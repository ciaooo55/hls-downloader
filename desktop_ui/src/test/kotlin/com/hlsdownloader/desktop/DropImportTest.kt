package com.hlsdownloader.desktop

import java.io.File
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class DropImportTest {
    @Test
    fun extracts_only_distinct_file_paths() {
        val first = File("drop-fixtures/one.torrent").absoluteFile
        val second = File("drop-fixtures/list.txt").absoluteFile
        val result = droppedFilePaths(listOf(first, "not-a-file", first, second, null))
        assertEquals(listOf(first.absolutePath, second.absolutePath), result)
    }

    @Test
    fun rejects_non_list_payloads() {
        assertTrue(droppedFilePaths("C:/not-a-java-file-list.txt").isEmpty())
        assertTrue(droppedFilePaths(null).isEmpty())
    }
}
