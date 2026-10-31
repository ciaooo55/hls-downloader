package com.hlsdownloader.desktop

import androidx.compose.runtime.State
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.structuralEqualityPolicy
import java.awt.event.KeyEvent as AwtKeyEvent

enum class TaskFilter(val label: String) { ALL("全部"), RUNNING("进行中"), QUEUED("排队中"), PAUSED("已暂停"), COMPLETED("已完成"), FAILED("失败") }
enum class TaskCategory(val label: String) { MEDIA("媒体"), PROGRAM("程序"), ARCHIVE("压缩包"), OTHER("其他") }
private val activeTaskStatuses = setOf("进行中", "录制中", "合并中", "校验中", "解析中", "探测中")
internal fun isActiveTaskStatus(status: String): Boolean = status in activeTaskStatuses
internal fun connectionStateOf(label: String): Boolean = label.contains("已连接")
data class DownloadTask(val id: String, val filename: String, val status: String, val progress: Float, val speed: String, val speedBytes: Long, val remaining: String, val segments: String, val updated: String, val source: TaskDto)
private val mediaResourceKinds = setOf("hls", "dash", "live")
internal val catMEDIA_EXTENSIONS = setOf(
    "mp4", "mkv", "webm", "mov", "avi", "m4v", "ts", "mp3", "m4a", "flac", "wav",
    "ac3", "aac", "3gp", "flv", "m2ts", "mka", "mpd", "mpeg", "mpg", "ogg", "opus",
    "wma", "wmv", "jpg", "png", "gif", "webp",
)
internal val catPROGRAM_EXTENSIONS = setOf("exe", "msi", "msix", "appx", "bat", "cmd", "com", "ps1")
internal val catARCHIVE_EXTENSIONS = setOf("zip", "7z", "rar", "tar", "gz", "bz2", "xz", "iso")

private val mediaFileExtensions = catMEDIA_EXTENSIONS - setOf("jpg", "png", "gif", "webp") + "m3u8"
private val executableFileExtensions = catPROGRAM_EXTENSIONS
private val shellLaunchExtensions = executableFileExtensions + setOf("hta", "inf", "js", "jse", "lnk", "reg", "scr", "url", "vbe", "vbs", "wsf", "wsh")
private fun taskFileExtension(task: TaskDto): String = task.filename
    .substringBefore('?')
    .substringAfterLast('/')
    .substringAfterLast('\\')
    .substringAfterLast('.', "")
    .lowercase()
internal fun taskSupportsMediaActions(task: TaskDto): Boolean {
    if (task.resourceKind.lowercase() in setOf("hls", "dash", "live", "media", "video", "audio")) return true
    return taskFileExtension(task) in mediaFileExtensions
}
internal fun taskActivationAction(task: TaskDto): String? {
    if (task.status.lowercase() !in setOf("completed", "done") || task.outputMissing) return null
    if (taskSupportsMediaActions(task) && "play" in task.availableActions) return "play"
    return "open".takeIf { it in task.availableActions && taskFileExtension(task) !in shellLaunchExtensions }
}

internal fun toolbarTaskTargets(all: List<DownloadTask>, visible: List<DownloadTask>, selected: Set<String>): List<DownloadTask> =
    if (selected.isEmpty()) all else visible.filter { it.id in selected }
internal fun taskMenuActions(task: TaskDto): List<String> {
    val mediaCapable = taskSupportsMediaActions(task)
    val base = task.availableActions.ifEmpty { listOf("start", "pause", "resume", "retry", "open", "delete") }
        .filterNot { it == "details" || (!mediaCapable && it in setOf("play", "cast", "push_tvbox")) }
        .filterNot { it in setOf("launch", "run") && taskFileExtension(task) !in executableFileExtensions }
    val media = if (mediaCapable && task.playbackReady) listOf("play", "cast", "push_tvbox") else emptyList()
    val queue = if (task.status.lowercase() in setOf("queued", "paused")) listOf("move_queue") else emptyList()
    val fileDelete = if ("delete" in base) listOf("delete_files") else emptyList()
    return (base + queue + fileDelete + media).distinct()
}
internal fun batchTaskMenuActions(tasks: List<TaskDto>): List<String> {
    if (tasks.isEmpty()) return emptyList()
    if (tasks.size == 1) return taskMenuActions(tasks.first())
    val allowedBatch = setOf("start", "pause", "resume", "retry", "cancel", "delete", "delete_files", "move_queue")
    val common = taskMenuActions(tasks.first()).filter { it in allowedBatch }.toMutableList()
    tasks.drop(1).forEach { task -> common.retainAll(taskMenuActions(task).toSet()) }
    return common.distinct()
}
internal fun taskStartAction(task: TaskDto): String? =
    if (task.status.lowercase() in setOf("completed", "done")) null
    else task.availableActions.firstOrNull { it in setOf("start", "resume", "retry") }
internal fun selectedTaskActionTargets(tasks: List<TaskDto>, action: String): List<Pair<String, String>> {
    val targets = if (action in setOf("play", "cast", "push_tvbox")) listOfNotNull(tasks.singleOrNull()) else tasks
    return targets.mapNotNull { task ->
        val resolved = if (action == "start") taskStartAction(task) else action.takeIf { it in taskMenuActions(task) }
        resolved?.let { task.id to it }
    }
}
internal fun workbenchShortcut(ctrl: Boolean, shift: Boolean, keyCode: Int): String? = when {
    ctrl && shift && keyCode == AwtKeyEvent.VK_N -> "batch"
    ctrl && keyCode == AwtKeyEvent.VK_N -> "new"
    ctrl && keyCode == AwtKeyEvent.VK_COMMA -> "settings"
    keyCode == AwtKeyEvent.VK_F5 -> "refresh"
    keyCode == AwtKeyEvent.VK_ESCAPE -> "escape"
    else -> null
}
internal fun sortTasks(tasks: List<DownloadTask>, raw: String): List<DownloadTask> {
    val (field, direction) = raw.split(':', limit = 2).let {
        (it.getOrNull(0)?.takeIf { value -> value in setOf("queue", "name", "progress", "status", "speed", "size") } ?: "queue") to
            (it.getOrNull(1)?.takeIf { value -> value in setOf("asc", "desc") } ?: "asc")
    }
    val comparator = when (field) {
        "name" -> compareBy<DownloadTask> { it.filename.lowercase() }.thenBy { it.source.queueIndex }
        "progress" -> compareBy<DownloadTask> { it.progress }.thenBy { it.source.queueIndex }
        "status" -> compareBy<DownloadTask> { it.status }.thenBy { it.source.queueIndex }
        "speed" -> compareBy<DownloadTask> { it.speedBytes }.thenBy { it.source.queueIndex }
        "size" -> compareBy<DownloadTask> { it.source.totalBytes ?: -1 }.thenBy { it.source.queueIndex }
        else -> compareBy<DownloadTask> { it.source.queueIndex }.thenBy { it.id }
    }
    return tasks.sortedWith(if (direction == "desc") comparator.reversed() else comparator)
}

internal fun upsertDownloadTask(tasks: MutableList<DownloadTask>, positions: MutableMap<String, Int>, task: DownloadTask) {
    val index = positions[task.id]
    if (index != null) tasks[index] = task else {
        positions[task.id] = tasks.size
        tasks += task
    }
}

internal fun removeDownloadTask(tasks: MutableList<DownloadTask>, positions: MutableMap<String, Int>, id: String) {
    val index = positions.remove(id) ?: return
    tasks.removeAt(index)
    for (shifted in index until tasks.size) positions[tasks[shifted].id] = shifted
}

internal fun nextTaskSort(current: String, field: String): String {
    val active = current.substringBefore(':').ifBlank { "queue" }
    val direction = current.substringAfter(':', "asc")
    return if (active == field) "$field:${if (direction == "asc") "desc" else "asc"}" else "$field:asc"
}

internal fun downloadTask(task: TaskDto): DownloadTask {
    val progress = when {
        task.status in setOf("completed", "done") -> 1f
        !task.isLive && task.resourceKind in setOf("hls", "dash") && task.totalRanges > 0 -> task.completedRanges.toFloat() / task.totalRanges
        else -> task.totalBytes?.takeIf { it > 0 }?.let { task.downloadedBytes.toFloat() / it } ?: 0f
    }.coerceIn(0f, 1f)
    val status = displayStatus(task.status)
    val completedRanges = if (status == "已完成") task.totalRanges else task.completedRanges
    val segments = if (task.totalRanges > 0) {
        "${completedRanges}/${task.totalRanges} ${if (task.resourceKind.equals("torrent", true)) "Piece" else "段"}"
    } else "—"
    val processing = task.status in setOf("merging", "checking")
    return DownloadTask(task.id, task.filename.ifBlank { task.title }, status, progress,
        if (processing) "本地处理" else formatRate(task.speedBytesPerSecond), task.speedBytesPerSecond,
        if (processing) "请稍候" else task.etaSeconds?.let(::formatEta) ?: "—", segments, "刚刚", task)
}
internal fun displayStatus(status: String) = when (status.lowercase()) {
    "completed", "done" -> "已完成"
    "running", "downloading" -> "进行中"
    "recording" -> "录制中"
    "merging" -> "合并中"
    "checking" -> "校验中"
    "parsing" -> "解析中"
    "probing" -> "探测中"
    "queued", "" -> "排队中"
    "paused" -> "已暂停"
    "canceled", "cancelled" -> "已取消"
    "failed", "error" -> "失败"
    // 未知状态显式展示为"其他"，避免被静默归入"排队中"导致过滤计数失真
    else -> "其他"
}
/**
 * 任务列表与接管弹窗共用分类规则，与 Rust category/media_ext 保持一致。
 * CategoryParityTest 检查跨语言扩展名表和媒体类型，修改时需同步两端。
 */
internal fun downloadCategory(filename: String, resourceKind: String): TaskCategory {
    // Core：ResourceKind::Hls | Dash | Live 一律算媒体，不看扩展名。
    if (resourceKind.lowercase() in mediaResourceKinds) return TaskCategory.MEDIA
    val extension = filename.substringAfterLast('.', "").lowercase()
    return when (extension) {
        in catMEDIA_EXTENSIONS -> TaskCategory.MEDIA
        in catPROGRAM_EXTENSIONS -> TaskCategory.PROGRAM
        in catARCHIVE_EXTENSIONS -> TaskCategory.ARCHIVE
        else -> TaskCategory.OTHER
    }
}

internal fun taskCategory(task: DownloadTask) = downloadCategory(task.filename, task.source.resourceKind)

internal data class TaskNavigationCounts(
    val total: Int,
    val active: Int,
    val statuses: Map<String, Int>,
    val queues: Map<String, Int>,
    val categories: Map<TaskCategory, Int>,
) {
    fun count(filter: TaskFilter): Int = when (filter) {
        TaskFilter.ALL -> total
        TaskFilter.RUNNING -> active
        else -> statuses[filter.label] ?: 0
    }
}

internal fun taskNavigationCounts(tasks: List<DownloadTask>): State<TaskNavigationCounts> = derivedStateOf(structuralEqualityPolicy()) {
    val statuses = mutableMapOf<String, Int>()
    val queues = mutableMapOf<String, Int>()
    val categories = mutableMapOf<TaskCategory, Int>()
    var active = 0
    for (task in tasks) {
        statuses[task.status] = (statuses[task.status] ?: 0) + 1
        val queue = task.source.queueId
        queues[queue] = (queues[queue] ?: 0) + 1
        val category = taskCategory(task)
        categories[category] = (categories[category] ?: 0) + 1
        if (isActiveTaskStatus(task.status)) active++
    }
    TaskNavigationCounts(tasks.size, active, statuses, queues, categories)
}

internal fun visibleTasks(tasks: List<DownloadTask>, filter: TaskFilter, category: TaskCategory?, query: String, queueId: String? = null): List<DownloadTask> =
    tasks.filter { (filter == TaskFilter.ALL || (filter == TaskFilter.RUNNING && isActiveTaskStatus(it.status)) || it.status == filter.label) && (category == null || taskCategory(it) == category) && (queueId == null || it.source.queueId == queueId) && it.filename.contains(query, true) }
internal fun taskProgressKnown(task: TaskDto): Boolean = task.status in setOf("completed", "done") || (task.totalBytes ?: 0) > 0 || (!task.isLive && task.resourceKind in setOf("hls", "dash") && task.totalRanges > 0)
internal fun taskProgressLabel(task: DownloadTask): String = if (taskProgressKnown(task.source)) "${(task.progress * 100).toInt()}%" else if (task.source.isLive) "录制中" else "大小待定"
internal fun formatRate(bytes: Long): String = when { bytes <= 0 -> "0 KB/s"; bytes >= 1024L * 1024L -> "%.1f MB/s".format(java.util.Locale.ROOT, bytes / 1024.0 / 1024.0); else -> "%.0f KB/s".format(java.util.Locale.ROOT, bytes / 1024.0) }
private fun formatEta(seconds: Long): String { val safe = seconds.coerceAtLeast(0); return when { safe < 60 -> "${safe}s"; safe < 3600 -> "${safe / 60} 分钟"; else -> "${safe / 3600} 小时" } }
