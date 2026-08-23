package com.hlsdownloader.desktop

import kotlin.system.measureNanoTime
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class PerformanceModelTest {
    @Test
    fun visual_fixture_has_stable_keys_and_hostile_content() {
        val tasks = auditTasks(1_000)
        assertEquals(1_000, tasks.size)
        assertEquals(1_000, tasks.map { it.id }.toSet().size)
        assertEquals(setOf("进行中", "排队中", "已暂停", "已完成", "失败"), tasks.map { it.status }.toSet())
        assertTrue(tasks.any { it.filename.startsWith("超长文件名") })
    }

    @Test
    fun thousand_task_mapping_and_filtering_p95_stays_under_100ms() {
        val source = (0 until 1_000).map { index ->
            TaskDto(
                id = "task-$index",
                filename = if (index % 4 == 0) "video-$index.mp4" else "archive-$index.zip",
                title = "Task $index",
                status = if (index % 3 == 0) "downloading" else "paused",
                downloadedBytes = index * 1024L,
                totalBytes = 1_048_576,
                speedBytesPerSecond = index * 10L,
                queueIndex = index.toLong(),
            )
        }
        val samples = ArrayList<Long>(60)
        repeat(60) {
            samples += measureNanoTime {
                val mapped = source.map(::downloadTask)
                val visible = visibleTasks(mapped, TaskFilter.ALL, TaskCategory.MEDIA, "video-")
                assertEquals(250, visible.size)
            }
        }
        samples.sort()
        val p95Ms = samples[(samples.size * 95 / 100).coerceAtMost(samples.lastIndex)] / 1_000_000.0
        println("thousand_task_model_p95_ms=%.3f".format(p95Ms))
        assertTrue(p95Ms <= 100.0, "1000-task model P95 was %.2fms".format(p95Ms))
    }
}
