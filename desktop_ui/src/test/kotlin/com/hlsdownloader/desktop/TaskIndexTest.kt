package com.hlsdownloader.desktop

import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.snapshots.Snapshot
import androidx.compose.runtime.snapshots.SnapshotStateObserver
import kotlin.test.Test
import kotlin.test.assertEquals

class TaskIndexTest {
    private fun task(id: String) = downloadTask(TaskDto(
        id = id, filename = "$id.mp4", status = "downloading",
    ))

    @Test
    fun deleting_and_recreating_tasks_never_updates_a_different_row() {
        val tasks = mutableListOf<DownloadTask>()
        val positions = mutableMapOf<String, Int>()
        listOf("a", "b", "c", "d").forEach { upsertDownloadTask(tasks, positions, task(it)) }
        removeDownloadTask(tasks, positions, "b")
        removeDownloadTask(tasks, positions, "a")
        upsertDownloadTask(tasks, positions, task("d").copy(filename = "changed.mp4"))
        upsertDownloadTask(tasks, positions, task("b"))
        removeDownloadTask(tasks, positions, "already-deleted")
        assertEquals(listOf("c", "d", "b"), tasks.map { it.id })
        assertEquals(listOf("c.mp4", "changed.mp4", "b.mp4"), tasks.map { it.filename })
        assertEquals(mapOf("c" to 0, "d" to 1, "b" to 2), positions)
    }

    @Test
    fun navigation_ignores_speed_updates_but_tracks_membership_changes() {
        val tasks = mutableStateListOf(
            task("a"),
            downloadTask(TaskDto(id = "b", filename = "b.zip", status = "merging", queueId = "later")),
            downloadTask(TaskDto(id = "c", filename = "c.exe", status = "paused")),
        )
        val counts = taskNavigationCounts(tasks)
        var invalidations = 0
        val observer = SnapshotStateObserver { it() }
        val changed: (Any) -> Unit = { invalidations++ }
        val scope = Any()
        observer.start()
        try {
            observer.observeReads(scope, changed) { counts.value }
            assertEquals(3, counts.value.count(TaskFilter.ALL))
            assertEquals(2, counts.value.count(TaskFilter.RUNNING))
            repeat(20) { index ->
                tasks[0] = tasks[0].copy(speedBytes = (index + 1L) * 1024, progress = index / 100f)
                Snapshot.sendApplyNotifications()
            }
            assertEquals(0, invalidations, "仅速度和进度变化不应使侧栏失效")

            tasks[0] = downloadTask(tasks[0].source.copy(status = "completed", filename = "a.zip", queueId = "later"))
            Snapshot.sendApplyNotifications()
            assertEquals(1, invalidations)
            assertEquals(1, counts.value.count(TaskFilter.RUNNING))
            assertEquals(1, counts.value.count(TaskFilter.COMPLETED))
            assertEquals(2, counts.value.queues["later"])
            assertEquals(0, counts.value.categories[TaskCategory.MEDIA] ?: 0)
            assertEquals(2, counts.value.categories[TaskCategory.ARCHIVE])

            observer.observeReads(scope, changed) { counts.value }
            tasks.clear()
            Snapshot.sendApplyNotifications()
            assertEquals(2, invalidations)
            assertEquals(0, counts.value.count(TaskFilter.ALL))
            assertEquals(emptyMap(), counts.value.queues)
            assertEquals(emptyMap(), counts.value.categories)
        } finally {
            observer.stop()
            observer.clear()
        }
    }
}
