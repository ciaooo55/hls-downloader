package com.hlsdownloader.desktop

import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import kotlin.math.roundToInt

internal val TASK_TABLE_COMPACT_WIDTH = 930.dp

internal data class TaskColumns(val name: Dp, val progress: Dp, val status: Dp, val speed: Dp, val size: Dp, val actions: Dp, val compact: Boolean) {
    val requiredWidth: Dp get() = name + progress + status + speed + size + actions + 30.dp
}

internal fun taskColumnsForWidth(width: Dp) = if (width < TASK_TABLE_COMPACT_WIDTH)
    TaskColumns(maxOf(250.dp, width - 520.dp), 140.dp, 100.dp, 85.dp, 95.dp, 70.dp, true)
else
    TaskColumns(maxOf(330.dp, width - 600.dp), 180.dp, 110.dp, 100.dp, 110.dp, 70.dp, false)

internal data class ResolvedTaskColumn(val id: String, val label: String, val width: Dp)
internal data class ResolvedTaskColumns(val items: List<ResolvedTaskColumn>, val compact: Boolean) {
    val requiredWidth: Dp get() = items.fold(30.dp) { total, item -> total + item.width }
}

private data class TaskColumnSetting(val column: ResolvedTaskColumn, val visible: Boolean)

private fun taskColumnSettings(width: Dp, raw: String): List<TaskColumnSetting> {
    val responsive = taskColumnsForWidth(width)
    val defaults = listOf(
        ResolvedTaskColumn("name", "名称", responsive.name),
        ResolvedTaskColumn("progress", "进度", responsive.progress),
        ResolvedTaskColumn("status", "状态", responsive.status),
        ResolvedTaskColumn("speed", "速度", responsive.speed),
        ResolvedTaskColumn("size", "大小", responsive.size),
        ResolvedTaskColumn("actions", "操作", responsive.actions),
    )
    val automatic = defaults.map { TaskColumnSetting(it, true) }
    if (raw.isBlank()) return automatic
    val seen = mutableSetOf<String>()
    val configured = mutableListOf<TaskColumnSetting>()
    for (entry in raw.split(',')) {
        val parts = entry.split(':')
        if (parts.size != 3) return automatic
        val column = defaults.firstOrNull { it.id == parts[0] } ?: return automatic
        val widthDp = parts[1].toIntOrNull()?.takeIf { it in 48..800 } ?: return automatic
        if (!seen.add(column.id) || parts[2] !in setOf("0", "1")) return automatic
        configured += TaskColumnSetting(column.copy(width = widthDp.dp.coerceAtLeast(if (column.id == "name") 160.dp else 48.dp)), parts[2] == "1")
    }
    if (configured.none { it.column.id == "name" && it.visible }) return automatic
    return configured + automatic.filter { it.column.id !in seen }
}

internal fun resolveTaskColumns(width: Dp, layout: String = ""): ResolvedTaskColumns = ResolvedTaskColumns(
    taskColumnSettings(width, layout).filter { it.visible }.map { it.column },
    width < TASK_TABLE_COMPACT_WIDTH,
)

internal fun resizeTaskColumnLayout(layout: String, viewport: Dp, id: String, width: Dp): String =
    taskColumnSettings(viewport, layout).joinToString(",") { item ->
        val column = item.column
        val widthDp = (if (column.id == id) width else column.width).value.roundToInt()
            .coerceIn(if (column.id == "name") 160 else 48, 800)
        "${column.id}:$widthDp:${if (item.visible) 1 else 0}"
    }
