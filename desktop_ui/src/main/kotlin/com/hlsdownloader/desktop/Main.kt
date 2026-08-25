package com.hlsdownloader.desktop

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.ui.tooling.preview.Preview
import androidx.compose.foundation.background
import androidx.compose.foundation.Image
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.focusable
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.gestures.scrollBy
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.HorizontalScrollbar
import androidx.compose.foundation.VerticalScrollbar
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsHoveredAsState
import androidx.compose.foundation.hoverable
import androidx.compose.foundation.rememberScrollbarAdapter
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.window.WindowDraggableArea
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.*
import androidx.compose.material.icons.outlined.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.ExperimentalComposeUiApi
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.painter.BitmapPainter
import androidx.compose.ui.graphics.toComposeImageBitmap
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.isCtrlPressed
import androidx.compose.ui.input.key.isShiftPressed
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.input.pointer.PointerEventType
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.PointerButton
import androidx.compose.ui.input.pointer.isPrimaryPressed
import androidx.compose.ui.input.pointer.isSecondaryPressed
import androidx.compose.ui.input.pointer.isCtrlPressed as isPointerCtrlPressed
import androidx.compose.ui.input.pointer.isShiftPressed as isPointerShiftPressed
import androidx.compose.ui.input.pointer.onPointerEvent
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.paneTitle
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInWindow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.IntRect
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.sp
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.window.Window
import androidx.compose.ui.window.WindowPlacement
import androidx.compose.ui.window.WindowScope
import androidx.compose.ui.window.Popup
import androidx.compose.ui.window.PopupPositionProvider
import androidx.compose.ui.window.PopupProperties
import androidx.compose.ui.window.application
import androidx.compose.ui.window.rememberWindowState
import kotlinx.coroutines.delay
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.coroutines.launch
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import java.awt.Dimension
import java.awt.EventQueue
import java.awt.KeyEventDispatcher
import java.awt.KeyboardFocusManager
import java.awt.Toolkit
import java.awt.datatransfer.DataFlavor
import java.awt.datatransfer.StringSelection
import java.awt.dnd.DnDConstants
import java.awt.event.KeyEvent as AwtKeyEvent
import java.awt.dnd.DropTarget
import java.awt.dnd.DropTargetAdapter
import java.awt.dnd.DropTargetDragEvent
import java.awt.dnd.DropTargetDropEvent
import java.awt.dnd.DropTargetEvent
import java.io.File
import java.net.URI
import java.nio.file.Files
import java.util.UUID
import javax.swing.JFileChooser
import javax.swing.filechooser.FileNameExtensionFilter
import kotlin.math.roundToInt
import org.jetbrains.skia.Image as SkiaImage

enum class TaskFilter(val label: String) { ALL("全部"), RUNNING("进行中"), QUEUED("排队中"), PAUSED("已暂停"), COMPLETED("已完成"), FAILED("失败") }
enum class TaskCategory(val label: String) { MEDIA("媒体"), PROGRAM("程序"), ARCHIVE("压缩包"), OTHER("其他") }
data class DownloadTask(val id: String, val filename: String, val status: String, val progress: Float, val speed: String, val speedBytes: Long, val remaining: String, val segments: String, val updated: String, val source: TaskDto)
data class HarvestCandidateUi(
    val url: String,
    val filename: String,
    val category: String,
    val size: Long,
    val extension: String = "",
)
private data class HarvestCreateRequest(
    val urls: List<String>,
    val referer: String,
    val concurrency: Long,
)
private data class HandoffDecision(
    val filename: String,
    val directory: String,
    val category: TaskCategory,
    val rememberDirectory: Boolean,
)
internal fun droppedFilePaths(payload: Any?): List<String> = (payload as? List<*>)
    .orEmpty().filterIsInstance<File>().map { it.absolutePath }.distinct()
internal fun selectionAfterClick(taskIds: List<String>, selected: Set<String>, anchorId: String?, targetId: String, shift: Boolean, toggle: Boolean): Set<String> {
    val anchor = taskIds.indexOf(anchorId)
    val target = taskIds.indexOf(targetId)
    return when {
        target < 0 -> selected
        shift && anchor >= 0 -> (if (toggle) selected.toMutableSet() else mutableSetOf()).apply {
            taskIds.subList(minOf(anchor, target), maxOf(anchor, target) + 1).forEach(::add)
        }
        toggle -> selected.toMutableSet().apply { if (!add(targetId)) remove(targetId) }
        else -> setOf(targetId)
    }
}
internal fun selectionAfterDrag(taskIds: List<String>, firstIndex: Int, lastIndex: Int, selected: Set<String>, additive: Boolean): Set<String> {
    if (taskIds.isEmpty()) return if (additive) selected else emptySet()
    val start = minOf(firstIndex, lastIndex).coerceIn(0, taskIds.lastIndex)
    val end = maxOf(firstIndex, lastIndex).coerceIn(0, taskIds.lastIndex)
    return (if (additive) selected.toMutableSet() else mutableSetOf()).apply {
        taskIds.subList(start, end + 1).forEach(::add)
    }
}
internal fun harvestFilterCounts(links: List<HarvestCandidateUi>): Map<String, Int> = buildMap {
    put("all", links.size)
    links.forEach { item -> put(item.category, getOrDefault(item.category, 0) + 1) }
}
internal fun visibleHarvestLinks(
    links: List<HarvestCandidateUi>,
    category: String,
    minimumBytes: Long,
): List<HarvestCandidateUi> = links.filter { item ->
    (category == "all" || item.category == category) &&
        (minimumBytes <= 0 || item.size >= minimumBytes)
}
internal fun mergeHarvestSizes(
    links: List<HarvestCandidateUi>,
    sizes: Map<String, Long>,
): List<HarvestCandidateUi> = links.map { item ->
    sizes[item.url]?.takeIf { it > 0 }?.let { item.copy(size = it) } ?: item
}
private val mediaFileExtensions = setOf(
    "3gp", "aac", "ac3", "avi", "flac", "flv", "m2ts", "m3u8", "m4a", "m4v", "mka",
    "mkv", "mov", "mp3", "mp4", "mpd", "mpeg", "mpg", "ogg", "opus", "ts", "wav", "webm", "wma", "wmv",
)
private val executableFileExtensions = setOf("appx", "bat", "cmd", "com", "exe", "msi", "msix", "ps1")
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
internal fun taskMenuActions(task: TaskDto): List<String> {
    val mediaCapable = taskSupportsMediaActions(task)
    val base = task.availableActions.ifEmpty { listOf("start", "pause", "resume", "retry", "open", "delete") }
        .filterNot { it in setOf("details", "play", "cast", "push_tvbox") && !mediaCapable }
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
internal fun workbenchShortcut(ctrl: Boolean, shift: Boolean, keyCode: Int): String? = when {
    ctrl && shift && keyCode == AwtKeyEvent.VK_N -> "batch"
    ctrl && keyCode == AwtKeyEvent.VK_N -> "new"
    ctrl && keyCode == AwtKeyEvent.VK_COMMA -> "settings"
    keyCode == AwtKeyEvent.VK_F5 -> "refresh"
    keyCode == AwtKeyEvent.VK_ESCAPE -> "escape"
    else -> null
}
private sealed interface UiSignal {
    data class Notice(val level: String, val message: String) : UiSignal
    data class Clipboard(val urls: List<String>) : UiSignal
    data class Probe(val url: String, val variants: List<StreamVariantDto>) : UiSignal
    data class TorrentProbe(val data: TorrentProbeDto) : UiSignal
    data class TorrentSelection(val source: String, val files: List<TorrentFileDto>, val totalSize: Long) : UiSignal
    data class Devices(val devices: List<CastDeviceDto>) : UiSignal
    data class MediaPush(val request: MediaPushRequestDto) : UiSignal
    data class MediaPushResolved(val request: MediaPushRequestDto) : UiSignal
    data class Harvest(val url: String, val links: List<HarvestCandidateUi>) : UiSignal
    data class Log(val taskId: String, val lines: List<String>) : UiSignal
    data class Duplicate(val taskId: String, val action: String, val message: String) : UiSignal
    data class Update(
        val current: String,
        val latest: String,
        val notes: String,
        val releaseUrl: String,
        val installerName: String,
        val installerSize: Long,
        val sha256Verified: Boolean,
    ) : UiSignal
    data class UpdatePrepared(
        val latest: String,
        val installerPath: String,
        val sha256: String,
        val productName: String,
        val productVersion: String,
        val upgradeCode: String,
    ) : UiSignal
    data class PowerPending(val action: String, val title: String, val delaySeconds: Long) : UiSignal
    data class Cast(
        val active: Boolean,
        val title: String,
        val device: String,
        val status: String,
        val taskId: String = "",
        val mediaUrl: String = "",
        val deviceKind: String = "",
        val supportedActions: List<String> = emptyList(),
        val playing: Boolean = false,
        val paused: Boolean = false,
        val positionSeconds: Long = 0,
        val durationSeconds: Long = 0,
        val positionAvailable: Boolean = false,
    ) : UiSignal
    data class Player(
        val active: Boolean,
        val title: String,
        val taskId: String = "",
        val status: String,
        val paused: Boolean = false,
        val speed: Double = 1.0,
        val positionSeconds: Double = 0.0,
        val durationSeconds: Double = 0.0,
        val positionAvailable: Boolean = false,
        val audioTracks: Int = 0,
        val subtitleTracks: Int = 0,
    ) : UiSignal
}
private data class DestructiveRequest(val action: String, val taskIds: Set<String>)
private data class MediaSourceSelection(val path: String = "", val url: String = "", val title: String = "")
internal data class TaskColumns(val name: Dp, val progress: Dp, val status: Dp, val speed: Dp, val size: Dp, val actions: Dp, val compact: Boolean) {
    val requiredWidth: Dp get() = name + progress + status + speed + size + actions + 30.dp
}
internal fun taskColumnsForWidth(width: Dp) = if (width < 930.dp) TaskColumns(225.dp, 185.dp, 100.dp, 75.dp, 70.dp, 55.dp, true)
    else TaskColumns(280.dp, 220.dp, 120.dp, 100.dp, 90.dp, 70.dp, false)

internal data class ResolvedTaskColumn(val id: String, val label: String, val width: Dp)
internal data class ResolvedTaskColumns(val items: List<ResolvedTaskColumn>, val compact: Boolean) {
    val requiredWidth: Dp get() = items.fold(30.dp) { total, item -> total + item.width }
}

internal fun resolveTaskColumns(width: Dp): ResolvedTaskColumns {
    val compact = width < 930.dp
    val responsive = taskColumnsForWidth(width)
    val items = listOf(
        ResolvedTaskColumn("name", "名称", responsive.name),
        ResolvedTaskColumn("progress", "进度 / 分段", responsive.progress),
        ResolvedTaskColumn("status", "状态", responsive.status),
        ResolvedTaskColumn("speed", "速度", responsive.speed),
        ResolvedTaskColumn("size", "大小", responsive.size),
        ResolvedTaskColumn("actions", "操作", responsive.actions),
    )
    return ResolvedTaskColumns(items, compact)
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

internal fun nextTaskSort(current: String, field: String): String {
    val active = current.substringBefore(':').ifBlank { "queue" }
    val direction = current.substringAfter(':', "asc")
    return if (active == field) "$field:${if (direction == "asc") "desc" else "asc"}" else "$field:asc"
}

private data class WorkbenchPalette(
    val canvas: Color, val rail: Color, val ink: Color, val muted: Color, val faint: Color,
    val blue: Color, val border: Color, val surface2: Color, val surface3: Color,
    val selected: Color, val dialog: Color, val success: Color, val warning: Color,
)

private val lightPalette = WorkbenchPalette(
    canvas = Color(0xFFEEF2F6), rail = Color.White, ink = Color(0xFF0F172A), muted = Color(0xFF475569), faint = Color(0xFF64748B),
    blue = Color(0xFF2563EB), border = Color(0xFFD8E0EA), surface2 = Color(0xFFF5F7FA), surface3 = Color(0xFFE8EDF3),
    selected = Color(0xFFE7F0FF), dialog = Color(0xFFFCFDFE), success = Color(0xFF16794B), warning = Color(0xFF9A5B00),
)
private val darkPalette = WorkbenchPalette(
    canvas = Color(0xFF151719), rail = Color(0xFF1C1F23), ink = Color(0xFFF4F5F6), muted = Color(0xFFC5C9CF), faint = Color(0xFF969CA4),
    blue = Color(0xFF5EA2F3), border = Color(0xFF383D43), surface2 = Color(0xFF23272B), surface3 = Color(0xFF2B3035),
    selected = Color(0xFF263A51), dialog = Color(0xFF22262A), success = Color(0xFF72D6A5), warning = Color(0xFFFFC46B),
)
private val LocalWorkbenchPalette = staticCompositionLocalOf { lightPalette }
private const val ENGINE_RECONNECTING_NOTICE = "下载引擎暂时无法连接，正在自动重试"
internal val canvas: Color @Composable @ReadOnlyComposable get() = LocalWorkbenchPalette.current.canvas
internal val rail: Color @Composable @ReadOnlyComposable get() = LocalWorkbenchPalette.current.rail
internal val ink: Color @Composable @ReadOnlyComposable get() = LocalWorkbenchPalette.current.ink
internal val muted: Color @Composable @ReadOnlyComposable get() = LocalWorkbenchPalette.current.muted
internal val faint: Color @Composable @ReadOnlyComposable get() = LocalWorkbenchPalette.current.faint
internal val blue: Color @Composable @ReadOnlyComposable get() = LocalWorkbenchPalette.current.blue
internal val border: Color @Composable @ReadOnlyComposable get() = LocalWorkbenchPalette.current.border
internal val surface2: Color @Composable @ReadOnlyComposable get() = LocalWorkbenchPalette.current.surface2
internal val surface3: Color @Composable @ReadOnlyComposable get() = LocalWorkbenchPalette.current.surface3
internal val selectedSurface: Color @Composable @ReadOnlyComposable get() = LocalWorkbenchPalette.current.selected
internal val dialogSurface: Color @Composable @ReadOnlyComposable get() = LocalWorkbenchPalette.current.dialog
internal val successColor: Color @Composable @ReadOnlyComposable get() = LocalWorkbenchPalette.current.success
internal val warningColor: Color @Composable @ReadOnlyComposable get() = LocalWorkbenchPalette.current.warning

fun main() {
    System.setProperty("compose.accessibility.enable", "true")
    System.setProperty("javax.accessibility.assistive_technologies", "com.sun.java.accessibility.AccessBridge")
    application {
    val auditSurface = System.getenv("HLS_UI_AUDIT_SURFACE").orEmpty().lowercase()
    val auditWidth = System.getenv("HLS_UI_AUDIT_WIDTH")?.toIntOrNull()?.coerceAtLeast(1024) ?: 1400
    val auditHeight = System.getenv("HLS_UI_AUDIT_HEIGHT")?.toIntOrNull()?.coerceAtLeast(600) ?: 820
    val state = rememberWindowState(width = auditWidth.dp, height = auditHeight.dp)
    val appIcon = remember { loadDesktopIcon() }
    var droppedPaths by remember { mutableStateOf<List<String>>(emptyList()) }
    var dropActive by remember { mutableStateOf(false) }
    Window(
        onCloseRequest = ::exitApplication,
        title = "HLS Downloader",
        icon = appIcon?.let(::BitmapPainter),
        state = state,
        undecorated = true,
        transparent = false,
    ) {
        LaunchedEffect(Unit) {
            window.minimumSize = Dimension(1024, 600)
            while (true) {
                withContext(Dispatchers.IO) { EnginePipeClient.ensurePresenterStarted() }
                delay(5_000)
            }
        }
        LaunchedEffect(auditSurface) {
            if (auditSurface == "tasks_1000") {
                window.isAlwaysOnTop = true
                while (true) {
                    System.setProperty("hls.audit.window.active", window.isActive.toString())
                    if (!window.isActive) {
                        window.toFront()
                        window.requestFocus()
                    }
                    delay(100)
                }
            }
        }
        DisposableEffect(window) {
            val uiTestApi = UiTestApi.startIfEnabled(window)
            val previous = window.dropTarget
            val target = DropTarget(window, object : DropTargetAdapter() {
                override fun dragEnter(event: DropTargetDragEvent) {
                    val accepted = event.isDataFlavorSupported(DataFlavor.javaFileListFlavor)
                    if (accepted) event.acceptDrag(DnDConstants.ACTION_COPY) else event.rejectDrag()
                    EventQueue.invokeLater { dropActive = accepted }
                }
                override fun dragExit(event: DropTargetEvent) { EventQueue.invokeLater { dropActive = false } }
                override fun drop(event: DropTargetDropEvent) {
                    try {
                        if (!event.isDataFlavorSupported(DataFlavor.javaFileListFlavor)) {
                            event.rejectDrop(); return
                        }
                        event.acceptDrop(DnDConstants.ACTION_COPY)
                        val paths = droppedFilePaths(event.transferable.getTransferData(DataFlavor.javaFileListFlavor))
                        event.dropComplete(paths.isNotEmpty())
                        EventQueue.invokeLater { droppedPaths = paths; dropActive = false }
                    } catch (_: Exception) {
                        event.dropComplete(false)
                        EventQueue.invokeLater { dropActive = false }
                    }
                }
            })
            onDispose {
                uiTestApi?.close()
                target.component = null
                window.dropTarget = previous
            }
        }
        AppShell(
            maximized = state.placement == WindowPlacement.Maximized,
            appIcon = appIcon,
            externalDropPaths = droppedPaths,
            externalDropActive = dropActive,
            onExternalDropConsumed = { droppedPaths = emptyList() },
            onAttention = {
                EventQueue.invokeLater {
                    window.isMinimized = false
                    val previousAlwaysOnTop = window.isAlwaysOnTop
                    window.isAlwaysOnTop = true
                    window.toFront()
                    window.requestFocus()
                    javax.swing.Timer(650) { window.isAlwaysOnTop = previousAlwaysOnTop }.apply {
                        isRepeats = false
                        start()
                    }
                }
            },
            onExit = ::exitApplication,
            titleBar = { WindowTitleBar(appIcon, state.placement == WindowPlacement.Maximized, { window.isMinimized = true }, { state.placement = if (state.placement == WindowPlacement.Maximized) WindowPlacement.Floating else WindowPlacement.Maximized }, ::exitApplication) },
        )
    }
    }
}

internal fun auditSettingsTab(surface: String): String? = when (surface) {
    "settings" -> "通用"
    "settings_download" -> "下载"
    "settings_network" -> "网络"
    "settings_devices" -> "投屏与推送"
    "settings_appearance" -> "外观"
    else -> null
}

@Composable @Preview
fun AppShell(maximized: Boolean = false, appIcon: ImageBitmap? = null, externalDropPaths: List<String> = emptyList(), externalDropActive: Boolean = false, onExternalDropConsumed: () -> Unit = {}, onAttention: () -> Unit = {}, onExit: () -> Unit = {}, titleBar: @Composable () -> Unit = {}) {
    val visualFixture = remember { System.getenv("HLS_UI_AUDIT_SURFACE").orEmpty().lowercase() }
    val visualTheme = remember { System.getenv("HLS_UI_AUDIT_THEME").orEmpty().lowercase() }
    var filter by remember { mutableStateOf(TaskFilter.ALL) }
    var category by remember { mutableStateOf<TaskCategory?>(null) }
    var selectedQueueId by remember { mutableStateOf<String?>(null) }
    var query by remember { mutableStateOf("") }
    var engineText by remember { mutableStateOf(Product.engineConnected) }
    var extensionText by remember { mutableStateOf(Product.extensionDisconnected) }
    var selected by remember { mutableStateOf<Set<String>>(emptySet()) }
    var darkMode by remember { mutableStateOf(visualTheme == "dark") }
    var newTaskDialog by remember { mutableStateOf(visualFixture == "new_task") }
    var batchDialog by remember { mutableStateOf(visualFixture == "batch") }
    var harvestDialog by remember { mutableStateOf(visualFixture == "harvest") }
    var settingsDialog by remember { mutableStateOf(auditSettingsTab(visualFixture) != null) }
    var settingsDeviceScanActive by remember { mutableStateOf(visualFixture == "settings_devices") }
    var queueManagerDialog by remember { mutableStateOf(visualFixture == "queues") }
    var queueAssignTaskIds by remember { mutableStateOf<Set<String>>(emptySet()) }
    var extensionDialog by remember { mutableStateOf(visualFixture == "extension") }
    var newTaskUrl by remember { mutableStateOf("") }
    var detailTaskId by remember { mutableStateOf<String?>(null) }
    var settings by remember { mutableStateOf(EngineSettingsDto()) }
    var handoffBusy by remember { mutableStateOf(false) }
    var refreshKey by remember { mutableIntStateOf(0) }
    var snapshotReady by remember { mutableStateOf(visualFixture == "tasks_1000") }
    var eventSequence by remember { mutableLongStateOf(0) }
    var notice by remember { mutableStateOf<UiSignal.Notice?>(null) }
    val shellFocus = remember { FocusRequester() }
    var probeResult by remember { mutableStateOf<UiSignal.Probe?>(null) }
    var probeDraft by remember { mutableStateOf<TaskDraft?>(null) }
    var torrentProbe by remember { mutableStateOf<UiSignal.TorrentProbe?>(null) }
    var torrentDraft by remember { mutableStateOf<TaskDraft?>(null) }
    var deviceResult by remember { mutableStateOf<UiSignal.Devices?>(when (visualFixture) {
        "devices", "devices_tvbox", "media_push_pending", "settings_devices" -> UiSignal.Devices(listOf(
            CastDeviceDto("dlna:living-room", "客厅电视", "192.168.1.21", "http://192.168.1.21/avtransport", "dlna"),
            CastDeviceDto("chromecast:bedroom", "Chromecast · 卧室显示器", "192.168.1.32:8009", "192.168.1.32:8009", "chromecast"),
            CastDeviceDto("tvbox:http://192.168.1.45:9978", "TVBox / 影视盒子", "http://192.168.1.45:9978", "http://192.168.1.45:9978", "tvbox"),
        ))
        "devices_loading" -> UiSignal.Devices(emptyList())
        else -> null
    }) }
    var harvestResult by remember { mutableStateOf<UiSignal.Harvest?>(null) }
    var harvestReferer by remember { mutableStateOf("") }
    var harvestProbeBusy by remember { mutableStateOf(false) }
    var castSession by remember { mutableStateOf<UiSignal.Cast?>(when (visualFixture) {
        "cast" -> UiSignal.Cast(true, "示例影片 1080p", "客厅电视", "PLAYING", deviceKind = "dlna", supportedActions = listOf("status", "play", "pause", "seek_to", "stop"), playing = true, positionSeconds = 754, durationSeconds = 5420, positionAvailable = true)
        "cast_tvbox" -> UiSignal.Cast(true, "示例影片 1080p", "TVBox / 影视盒子", "PUBLISHED", mediaUrl = "http://192.168.1.8:49152/media/tvbox-demo", deviceKind = "tvbox", supportedActions = listOf("stop"))
        "cast_lan" -> UiSignal.Cast(true, "示例影片 1080p", "局域网", "PUBLISHED", mediaUrl = "http://192.168.1.8:49152/media/lan-demo/video.mp4", deviceKind = "lan", supportedActions = listOf("stop"))
        "media_stack" -> UiSignal.Cast(true, "客厅纪录片", "客厅电视", "PLAYING", deviceKind = "dlna", supportedActions = listOf("status", "play", "pause", "seek_to", "stop"), playing = true, positionSeconds = 132, durationSeconds = 1860, positionAvailable = true)
        "cast_offline" -> UiSignal.Cast(true, "示例影片 1080p", "客厅电视", "OFFLINE", deviceKind = "dlna", supportedActions = listOf("status", "play", "pause", "seek_to", "stop"), paused = true, positionSeconds = 754, durationSeconds = 5420, positionAvailable = true)
        else -> null
    }) }
    var playerSession by remember { mutableStateOf<UiSignal.Player?>(if (visualFixture in setOf("player", "media_stack")) UiSignal.Player(true, "示例影片 1080p", "visual-fixture", "PLAYING", speed = 1.25) else null) }
    var duplicateResult by remember { mutableStateOf<UiSignal.Duplicate?>(null) }
    var updateResult by remember { mutableStateOf<UiSignal.Update?>(if (visualFixture == "update") UiSignal.Update(
        current = "7.0.0",
        latest = "7.1.0",
        notes = "优化 HLS 直播恢复、TVBox 推送和高 DPI 布局。",
        releaseUrl = "https://github.com/ciaooo55/hls-downloader/releases/tag/v7.1.0",
        installerName = "HLSDownloader-7.1.0-Windows-x64.msi",
        installerSize = 154_453_055,
        sha256Verified = true,
    ) else null) }
    var preparedUpdate by remember { mutableStateOf<UiSignal.UpdatePrepared?>(if (visualFixture == "update_prepared") UiSignal.UpdatePrepared(
        latest = "7.1.0",
        installerPath = "C:/Users/lee/AppData/Local/Temp/HLSDownloader/updates/7.1.0/HLSDownloader-7.1.0-Windows-x64.msi",
        sha256 = "a761c958bffea479f736c987be5e335bce6914ddb0267ef6fdd1a3673ac661b0",
        productName = "HLSDownloader",
        productVersion = "7.1.0",
        upgradeCode = "{1C80D5F7-A1EC-4BAE-A4A6-E010C5A3EE6B}",
    ) else null) }
    var powerPending by remember { mutableStateOf<UiSignal.PowerPending?>(null) }
    var pendingCastTask by remember { mutableStateOf<String?>(if (visualFixture in setOf("devices", "devices_tvbox")) "visual-fixture" else null) }
    var pendingCastMode by remember { mutableStateOf(when (visualFixture) { "devices" -> "cast"; "devices_tvbox", "media_push_pending" -> "tvbox"; else -> "" }) }
    var mediaSourceDialog by remember { mutableStateOf(when (visualFixture) { "media_source" -> "cast"; "media_source_tvbox" -> "tvbox"; else -> "" }) }
    var pendingMediaSource by remember { mutableStateOf<MediaSourceSelection?>(if (visualFixture == "media_push_pending") MediaSourceSelection(url = "https://cdn.example.test/客厅纪录片.m3u8", title = "浏览器请求：客厅纪录片") else null) }
    var pendingPushRequestId by remember { mutableStateOf<String?>(if (visualFixture == "media_push_pending") "media-push-visual-fixture" else null) }
    var castDiscovering by remember { mutableStateOf(visualFixture == "devices_loading") }
    var castConnecting by remember { mutableStateOf(false) }
    var castControlBusy by remember { mutableStateOf(false) }
    var playerControlBusy by remember { mutableStateOf(false) }
    var updateDownloadBusy by remember { mutableStateOf(false) }
    var castPollFailures by remember { mutableIntStateOf(0) }
    var destructiveRequest by remember { mutableStateOf<DestructiveRequest?>(null) }
    val scope = rememberCoroutineScope()
    val tasks = remember(visualFixture) {
        mutableStateListOf<DownloadTask>().apply {
            if (visualFixture == "tasks_1000") addAll(auditTasks(1_000))
        }
    }
    val handoffQueue = remember { mutableStateListOf<HandoffOfferDto>() }
    val activeHandoff = handoffQueue.firstOrNull()
    val visible = sortTasks(visibleTasks(tasks, filter, category, query, selectedQueueId), settings.taskSort)
    SideEffect { UiTestState.updateSelection(selected) }
    fun performTaskAction(taskId: String, action: String) {
        if (action in setOf("delete", "delete_files")) {
            destructiveRequest = DestructiveRequest(action, setOf(taskId))
            return
        }
        if (action == "cast" || action == "push_tvbox") {
            settingsDeviceScanActive = false
            pendingCastTask = taskId
            pendingCastMode = if (action == "push_tvbox") "tvbox" else "cast"
            castDiscovering = true
            deviceResult = UiSignal.Devices(emptyList())
        }
        if (action == "move_queue") {
            queueAssignTaskIds = setOf(taskId)
            return
        }
        val requestId = UUID.randomUUID().toString()
        scope.launch {
            runCatching { withContext(Dispatchers.IO) {
                when (action) {
                    "play" -> EnginePipeClient().playTask(taskId)
                    "cast" -> EnginePipeClient().discoverCastDevices("cast")
                    "push_tvbox" -> EnginePipeClient().discoverCastDevices("tvbox")
                    "open" -> EnginePipeClient().openCompleted(taskId, false)
                    "open_folder" -> EnginePipeClient().openCompleted(taskId, true)
                    "log" -> EnginePipeClient().getTaskLog(taskId)
                    "save_site_profile" -> EnginePipeClient().saveSiteProfile(taskId)
                    else -> EnginePipeClient().taskAction(taskId, action)
                }
            } }.onSuccess { refreshKey++ }.onFailure { error ->
                UiDiagnostics.error("task_action.$action", error, taskId, requestId)
                notice = UiSignal.Notice("error", error.message ?: "任务操作失败")
            }
        }
    }
    fun performTaskActions(taskIds: Set<String>, action: String) {
        if (taskIds.isEmpty()) return
        if (taskIds.size == 1) {
            performTaskAction(taskIds.first(), action)
            return
        }
        when (action) {
            "delete", "delete_files" -> destructiveRequest = DestructiveRequest(action, taskIds)
            "move_queue" -> queueAssignTaskIds = taskIds
            else -> scope.launch {
                runCatching { withContext(Dispatchers.IO) { taskIds.forEach { EnginePipeClient().taskAction(it, action) } } }
                    .onSuccess { refreshKey++ }
                    .onFailure { notice = UiSignal.Notice("error", it.message ?: "批量操作失败") }
            }
        }
    }
    fun applyWorkbenchShortcut(action: String?): Boolean = when (action) {
        "new" -> { newTaskUrl = ""; newTaskDialog = true; true }
        "batch" -> { batchDialog = true; true }
        "settings" -> { settingsDialog = true; true }
        "refresh" -> { refreshKey++; true }
        "escape" -> when {
            newTaskDialog -> { newTaskDialog = false; true }
            batchDialog -> { batchDialog = false; true }
            harvestDialog -> { harvestDialog = false; true }
            settingsDialog -> { settingsDialog = false; true }
            extensionDialog -> { extensionDialog = false; true }
            detailTaskId != null -> { detailTaskId = null; true }
            selected.isNotEmpty() -> { selected = emptySet(); true }
            else -> false
        }
        else -> false
    }
    val currentShortcutHandler = rememberUpdatedState<(String?) -> Boolean> { action ->
        applyWorkbenchShortcut(action)
    }
    DisposableEffect(Unit) {
        val manager = KeyboardFocusManager.getCurrentKeyboardFocusManager()
        val dispatcher = KeyEventDispatcher { event ->
            event.id == AwtKeyEvent.KEY_PRESSED && currentShortcutHandler.value(
                workbenchShortcut(event.isControlDown, event.isShiftDown, event.keyCode),
            )
        }
        manager.addKeyEventDispatcher(dispatcher)
        onDispose { manager.removeKeyEventDispatcher(dispatcher) }
    }
    LaunchedEffect(externalDropPaths) {
        if (externalDropPaths.isNotEmpty()) {
            val paths = externalDropPaths
            runCatching { withContext(Dispatchers.IO) { EnginePipeClient().importPaths(paths) } }
                .onSuccess { result ->
                    val count = result.events.count { it.event["kind"]?.jsonPrimitive?.content == "task_created" }
                    notice = UiSignal.Notice("success", "已导入 $count 项任务")
                    refreshKey++
                }
                .onFailure { notice = UiSignal.Notice("error", it.message ?: "拖入文件失败") }
            onExternalDropConsumed()
        }
    }
    LaunchedEffect(refreshKey) {
        if (visualFixture == "tasks_1000") {
            engineText = Product.engineConnected
            return@LaunchedEffect
        }
        engineText = Product.engineReconnecting
        var attempts = 0
        while (!snapshotReady) {
            val snapshot = runCatching { withContext(Dispatchers.IO) { EnginePipeClient().snapshotState() } }
            val state = snapshot.getOrNull()
            if (state != null) {
                tasks.clear()
                tasks += state.tasks.map(::downloadTask)
                eventSequence = state.latestSequence
                snapshotReady = true
                engineText = Product.engineConnected
                if (notice?.message == ENGINE_RECONNECTING_NOTICE) notice = null
                break
            }
            val error = snapshot.exceptionOrNull() ?: IllegalStateException("下载引擎连接失败")
            if (attempts == 0) UiDiagnostics.error("engine.snapshot", error)
            if (attempts % 4 == 0) {
                withContext(Dispatchers.IO) { EnginePipeClient.ensureStarted() }
            }
            attempts++
            if (attempts == 12) notice = UiSignal.Notice("error", ENGINE_RECONNECTING_NOTICE)
            delay((100L + attempts * 40L).coerceAtMost(500L))
        }
        runCatching { withContext(Dispatchers.IO) { EnginePipeClient().loadSettings() } }
            .onSuccess { loaded ->
                settings = loaded
                darkMode = when (visualTheme) { "light" -> false; "dark" -> true; else -> loaded.darkMode }
            }
        if (visualFixture.isBlank()) {
            runCatching { withContext(Dispatchers.IO) { EnginePipeClient().loadMediaPushRequests().firstOrNull() } }
                .onSuccess { request ->
                    if (request != null && pendingPushRequestId == null) {
                        pendingPushRequestId = request.id
                        pendingCastTask = null
                        pendingCastMode = request.pushKind
                        pendingMediaSource = MediaSourceSelection(url = request.url, title = request.title)
                        castDiscovering = true
                        deviceResult = UiSignal.Devices(emptyList())
                        scope.launch {
                            runCatching { withContext(Dispatchers.IO) { EnginePipeClient().discoverCastDevices(request.pushKind) } }
                                .onFailure { castDiscovering = false; notice = UiSignal.Notice("error", it.message ?: "设备搜索失败") }
                        }
                    }
                }
        }
    }
    LaunchedEffect(Unit) {
        if (visualFixture == "tasks_1000") return@LaunchedEffect
        while (true) {
            if (!snapshotReady) { delay(80); continue }
            val received = runCatching { withContext(Dispatchers.IO) { EnginePipeClient().waitEvents(eventSequence) } }
            received.onSuccess { events ->
                events.forEach { envelope ->
                    if (envelope.sequence > eventSequence + 1 && eventSequence > 0) refreshKey++
                    eventSequence = maxOf(eventSequence, envelope.sequence)
                    val eventKind = envelope.event["kind"]?.jsonPrimitive?.content.orEmpty()
                    if (eventKind == "settings_changed") refreshKey++
                    val attentionRequired = when (eventKind) {
                        "handoff_offered", "error" -> true
                        "update_install_result" -> envelope.event["status"]?.jsonPrimitive?.content != "success"
                        "task_created", "task_updated", "task_progress" -> {
                            val snapshot = envelope.event["snapshot"]?.jsonObject
                            val taskId = snapshot?.get("task_id")?.jsonPrimitive?.content.orEmpty()
                            val previousStatus = tasks.firstOrNull { it.id == taskId }?.source?.status.orEmpty().lowercase()
                            val nextStatus = snapshot?.get("status")?.jsonPrimitive?.content.orEmpty().lowercase()
                            previousStatus != nextStatus && nextStatus in setOf("completed", "done", "failed", "error")
                        }
                        else -> false
                    }
                    if (eventKind == "error") {
                        UiDiagnostics.warning(
                            "engine.event.error",
                            envelope.event["message"]?.jsonPrimitive?.content ?: "下载引擎报告错误",
                            envelope.event["task_id"]?.jsonPrimitive?.content.orEmpty(),
                            envelope.event["request_id"]?.jsonPrimitive?.content.orEmpty(),
                        )
                    }
                    handleSignal(applyEngineEvent(
                        tasks,
                        envelope.event,
                        setExtension = { connected -> extensionText = connected },
                        offerHandoff = { offer -> if (handoffQueue.none { it.handoffId == offer.handoffId }) handoffQueue += offer },
                        resolveHandoff = { handoffId -> handoffQueue.removeAll { it.handoffId == handoffId } },
                    ), { signal ->
                        when (signal) {
                            is UiSignal.Notice -> notice = signal
                            is UiSignal.Clipboard -> {
                                if (signal.urls.size == 1) {
                                    newTaskUrl = signal.urls.first()
                                    newTaskDialog = true
                                } else if (signal.urls.isNotEmpty()) {
                                    notice = UiSignal.Notice("info", "检测到 ${signal.urls.size} 条可下载链接，请使用批量添加")
                                }
                            }
                            is UiSignal.Probe -> {
                                newTaskDialog = false
                                probeResult = signal
                            }
                            is UiSignal.TorrentProbe -> torrentProbe = signal
                            is UiSignal.TorrentSelection -> notice = UiSignal.Notice("success", "已选择 ${signal.files.count { it.selected }} 个文件，共 ${formatBytes(signal.totalSize)}")
                            is UiSignal.Devices -> { deviceResult = signal; castDiscovering = false }
                            is UiSignal.MediaPush -> {
                                settingsDeviceScanActive = false
                                pendingPushRequestId = signal.request.id
                                pendingCastTask = null
                                pendingCastMode = signal.request.pushKind
                                pendingMediaSource = MediaSourceSelection(url = signal.request.url, title = signal.request.title)
                                castDiscovering = true
                                deviceResult = UiSignal.Devices(emptyList())
                                scope.launch {
                                    runCatching { withContext(Dispatchers.IO) { EnginePipeClient().discoverCastDevices(signal.request.pushKind) } }
                                        .onFailure { castDiscovering = false; notice = UiSignal.Notice("error", it.message ?: "设备搜索失败") }
                                }
                            }
                            is UiSignal.MediaPushResolved -> {
                                val request = signal.request
                                val level = when (request.status.lowercase()) {
                                    "done" -> "success"
                                    "canceled" -> "info"
                                    else -> "error"
                                }
                                notice = UiSignal.Notice(level, request.message.ifBlank {
                                    when (level) {
                                        "success" -> if (request.pushKind == "tvbox") "TVBox 推送完成" else "投屏完成"
                                        "info" -> "已取消设备选择"
                                        else -> if (request.pushKind == "tvbox") "TVBox 推送失败" else "投屏失败"
                                    }
                                })
                            }
                            is UiSignal.Harvest -> harvestResult = signal
                            is UiSignal.Log -> {
                                val index = tasks.indexOfFirst { it.id == signal.taskId }
                                if (index >= 0) tasks[index] = tasks[index].copy(source = tasks[index].source.copy(logTail = signal.lines))
                            }
                            is UiSignal.Duplicate -> {
                                if (signal.action == "focus" && tasks.any { it.id == signal.taskId }) {
                                    selected = setOf(signal.taskId)
                                    detailTaskId = signal.taskId
                                    duplicateResult = null
                                } else {
                                    duplicateResult = signal
                                }
                            }
                            is UiSignal.Update -> updateResult = signal
                            is UiSignal.UpdatePrepared -> { updateResult = null; preparedUpdate = signal }
                            is UiSignal.PowerPending -> powerPending = signal
                            is UiSignal.Cast -> {
                                castPollFailures = 0
                                castSession = signal.copy(
                                    title = signal.title.ifBlank { castSession?.title.orEmpty() },
                                    taskId = signal.taskId.ifBlank { castSession?.taskId.orEmpty() },
                                    mediaUrl = signal.mediaUrl.ifBlank { castSession?.mediaUrl.orEmpty() },
                                )
                            }
                            is UiSignal.Player -> playerSession = signal
                        }
                    })
                    if (attentionRequired) onAttention()
                }
                engineText = Product.engineConnected
            }.onFailure { error ->
                UiDiagnostics.error("engine.wait_events", error)
                engineText = Product.engineReconnecting
                snapshotReady = false
                delay(500)
                refreshKey++
            }
        }
    }
    LaunchedEffect(snapshotReady, visualFixture) {
        if (!snapshotReady || visualFixture.isNotBlank()) return@LaunchedEffect
        delay(5_000)
        while (true) {
            runCatching { withContext(Dispatchers.IO) { EnginePipeClient().checkUpdate(silent = true) } }
                .onFailure { UiDiagnostics.warning("update.check.silent", it.message ?: "静默检查更新失败") }
            delay(24 * 60 * 60 * 1_000L)
        }
    }
    LaunchedEffect(castSession?.active, castSession?.supportedActions) {
        while (visualFixture !in setOf("cast", "cast_tvbox", "cast_lan", "cast_offline", "media_stack") && castSession?.active == true && castSession?.supportedActions?.contains("status") == true) {
            delay(1_000)
            runCatching { withContext(Dispatchers.IO) { EnginePipeClient().controlCast("status") } }
                .onFailure { error ->
                    castPollFailures++
                    if (castPollFailures >= 2) castSession = castSession?.copy(status = "OFFLINE", playing = false, paused = true)
                    notice = UiSignal.Notice("error", error.message ?: "无法读取投屏设备状态")
                    delay(2_000)
                }
        }
    }
    LaunchedEffect(Unit) { shellFocus.requestFocus() }
    val modalVisible = newTaskDialog || batchDialog || harvestDialog || settingsDialog || queueManagerDialog ||
        queueAssignTaskIds.isNotEmpty() || detailTaskId != null || extensionDialog || activeHandoff != null ||
        probeResult != null || torrentProbe != null || harvestResult != null || mediaSourceDialog.isNotEmpty() ||
        (!settingsDeviceScanActive && deviceResult != null) || duplicateResult != null || updateResult != null ||
        preparedUpdate != null || powerPending != null || destructiveRequest != null
    CompositionLocalProvider(LocalWorkbenchPalette provides if (darkMode) darkPalette else lightPalette) {
    Column(Modifier.fillMaxSize().focusRequester(shellFocus).focusable().clip(RoundedCornerShape(if (maximized) 0.dp else 9.dp)).background(canvas).border(1.dp, border, RoundedCornerShape(if (maximized) 0.dp else 9.dp)).then(if (modalVisible) Modifier.clearAndSetSemantics { } else Modifier)) {
        titleBar()
        DesktopToolbar(query, { query = it }, { newTaskUrl = ""; newTaskDialog = true }, {
            scope.launch {
                val clipboard = runCatching { withContext(Dispatchers.IO) { Toolkit.getDefaultToolkit().systemClipboard.getData(DataFlavor.stringFlavor) as? String } }.getOrNull().orEmpty()
                newTaskUrl = clipboard.trim(); newTaskDialog = true
            }
        }, { batchDialog = true }, { harvestDialog = true }, { tasks.forEach { task -> val action = task.source.availableActions.firstOrNull { it in setOf("start", "resume", "retry") }; if (action != null) scope.launch { runCatching { withContext(Dispatchers.IO) { EnginePipeClient().taskAction(task.id, action) } }.onSuccess { refreshKey++ } } } }, { tasks.filter { "pause" in it.source.availableActions }.forEach { task -> scope.launch { runCatching { withContext(Dispatchers.IO) { EnginePipeClient().taskAction(task.id, "pause") } }.onSuccess { refreshKey++ } } } }, { mediaSourceDialog = "cast" }, { mediaSourceDialog = "tvbox" }, { extensionDialog = true }, { settingsDialog = true }, darkMode, {
            darkMode = !darkMode
            scope.launch { runCatching { withContext(Dispatchers.IO) { EnginePipeClient().storeSetting("dark_mode", darkMode) } } }
        })
        Row(Modifier.weight(1f).fillMaxWidth()) {
                Sidebar(
                    filter, category, selectedQueueId, settings.queueProfiles, tasks, engineText, extensionText,
                    { filter = it; category = null },
                    { category = if (category == it) null else it },
                    { selectedQueueId = if (selectedQueueId == it) null else it },
                    { queueManagerDialog = true },
                )
                Column(Modifier.weight(1f).fillMaxHeight().background(canvas)) {
                ContentHeader(filter, visible.size, selected.isNotEmpty(), tasks.any { it.status == "已完成" }, { refreshKey++ }, {
                    scope.launch {
                        runCatching { withContext(Dispatchers.IO) { EnginePipeClient().clearCompleted() } }
                            .onSuccess { refreshKey++ }.onFailure { engineText = Product.engineReconnecting }
                    }
                }, onMore = { action ->
                    when (action) {
                        "import" -> chooseImportPaths()?.let { paths -> scope.launch { runCatching { withContext(Dispatchers.IO) { EnginePipeClient().importPaths(paths) } }.onSuccess { result -> notice = UiSignal.Notice("success", "已导入 ${result.events.count { it.event["kind"]?.jsonPrimitive?.content == "task_created" }} 项任务"); refreshKey++ }.onFailure { notice = UiSignal.Notice("error", it.message ?: "导入失败") } } }
                        "export" -> chooseExportPath()?.let { path -> scope.launch { runCatching { withContext(Dispatchers.IO) { exportTaskList(path, selected.toList()) } }.onSuccess { result -> notice = UiSignal.Notice("success", "已导出 ${result.taskCount} 项任务") }.onFailure { notice = UiSignal.Notice("error", it.message ?: "导出失败") } } }
                        "update" -> scope.launch { runCatching { withContext(Dispatchers.IO) { EnginePipeClient().checkUpdate(silent = false) } }.onFailure { notice = UiSignal.Notice("error", it.message ?: "检查更新失败") } }
                        "cancel_power" -> scope.launch { runCatching { withContext(Dispatchers.IO) { EnginePipeClient().cancelPowerAction() } }.onFailure { notice = UiSignal.Notice("error", it.message ?: "取消电源动作失败") } }
                    }
                }) { action ->
                    if (action in setOf("delete", "delete_files")) destructiveRequest = DestructiveRequest(action, selected)
                    else if (action == "move_queue") queueAssignTaskIds = selected
                    else if (action in setOf("play", "cast", "push_tvbox")) selected.firstOrNull()?.let { performTaskAction(it, action) }
                    else scope.launch {
                        runCatching { withContext(Dispatchers.IO) { selected.forEach { EnginePipeClient().taskAction(it, action) } } }
                            .onSuccess { refreshKey++ }.onFailure { notice = UiSignal.Notice("error", it.message ?: "批量操作失败") }
                    }
                }
                TaskTable(
                    visible,
                    selected,
                    appIcon,
                    settings.taskSort,
                    { selected = it },
                    { detailTaskId = it.id },
                    onDeleteSelection = { if (selected.isNotEmpty()) destructiveRequest = DestructiveRequest("delete", selected) },
                    onQueueMove = { taskId, delta -> scope.launch {
                        runCatching { withContext(Dispatchers.IO) { EnginePipeClient().reorderQueue(taskId, delta) } }
                            .onSuccess { refreshKey++ }.onFailure { notice = UiSignal.Notice("error", it.message ?: "队列排序失败") }
                    } },
                    modifier = Modifier.weight(1f),
                    onBatchAction = ::performTaskActions,
                    onSort = { field ->
                        val next = nextTaskSort(settings.taskSort, field)
                        settings = settings.copy(taskSort = next)
                        scope.launch { runCatching { withContext(Dispatchers.IO) { EnginePipeClient().storeSetting("task_sort", next) } } }
                    },
                ) { taskId, action -> performTaskAction(taskId, action) }
                }
        }
        ConnectionStatus(tasks, engineText, extensionText)
    }
    if (externalDropActive || visualFixture == "drop") {
        Popup(alignment = Alignment.Center, properties = PopupProperties(focusable = false)) {
            Surface(color = dialogSurface, shape = RoundedCornerShape(9.dp), shadowElevation = 14.dp, border = BorderStroke(2.dp, blue), modifier = Modifier.width(390.dp)) {
                Row(Modifier.padding(horizontal = 20.dp, vertical = 18.dp), verticalAlignment = Alignment.CenterVertically) {
                    Surface(Modifier.size(42.dp), color = selectedSurface, shape = RoundedCornerShape(8.dp)) { Box(contentAlignment = Alignment.Center) { Icon(Icons.Outlined.FileDownload, null, tint = blue, modifier = Modifier.size(23.dp)) } }
                    Spacer(Modifier.width(13.dp)); Column { Text("松开以导入", color = ink, fontSize = 14.sp, fontWeight = FontWeight.SemiBold); Text("支持任务 JSON、种子、Metalink 和 URL 列表", color = muted, fontSize = 10.sp, modifier = Modifier.padding(top = 4.dp)) }
                }
            }
        }
    }
    if (newTaskDialog) NewTaskDialog({ newTaskDialog = false }, newTaskUrl, settings, onProbe = { draft ->
        if (taskProbeTarget(draft) == ResourceProbeTarget.Torrent) {
            torrentDraft = draft.copy(queueId = selectedQueueId ?: "default")
            scope.launch {
                runCatching { withContext(Dispatchers.IO) { EnginePipeClient().probeTorrent(draft.url) } }
                    .onSuccess { newTaskDialog = false }
                    .onFailure { notice = UiSignal.Notice("error", it.message ?: "种子分析失败") }
            }
        } else {
            scope.launch {
                probeDraft = draft
                runCatching { withContext(Dispatchers.IO) { EnginePipeClient().probeUrl(draft) } }
                    .onSuccess { newTaskDialog = false }
                    .onFailure { notice = UiSignal.Notice("error", it.message ?: "资源分析失败") }
            }
        }
    }) { draft ->
        if (draft.kind.equals("torrent", true)) {
            torrentDraft = draft.copy(queueId = selectedQueueId ?: "default")
            scope.launch { runCatching { withContext(Dispatchers.IO) { EnginePipeClient().probeTorrent(draft.url) } }.onFailure { notice = UiSignal.Notice("error", it.message ?: "种子分析失败") } }
            newTaskDialog = false
            return@NewTaskDialog
        }
        scope.launch {
            runCatching { withContext(Dispatchers.IO) {
                val queued = draft.copy(queueId = selectedQueueId ?: "default")
                if (queued.curlCommand.isNotBlank()) EnginePipeClient().importCurl(queued) else EnginePipeClient().createTask(queued)
            } }
                .onSuccess { refreshKey++ }
                .onFailure { notice = UiSignal.Notice("error", it.message ?: "创建下载失败") }
        }
        newTaskDialog = false
    }
    torrentProbe?.let { signal -> TorrentSelectionDialog(signal.data, { torrentProbe = null; torrentDraft = null }) { files ->
        val draft = torrentDraft ?: return@TorrentSelectionDialog
        scope.launch {
            runCatching {
                withContext(Dispatchers.IO) {
                    EnginePipeClient().selectTorrentFiles(draft.url, files)
                    EnginePipeClient().createTask(draft.copy(torrentSelection = files))
                }
            }.onSuccess { torrentProbe = null; torrentDraft = null; refreshKey++ }
                .onFailure { notice = UiSignal.Notice("error", it.message ?: "创建种子任务失败") }
        }
    } }
    if (batchDialog) BatchAddDialog({ batchDialog = false }) { urls ->
        scope.launch {
            runCatching { withContext(Dispatchers.IO) { urls.forEach { EnginePipeClient().createTask(TaskDraft(url = it, queueId = selectedQueueId ?: "default")) } } }
                .onSuccess { refreshKey++ }
                .onFailure { engineText = Product.engineReconnecting }
        }
        batchDialog = false
    }
    if (harvestDialog) HarvestDialog({ harvestDialog = false }, settings.defaultReferer) { url, referer ->
        val effectiveReferer = referer.ifBlank { url }
        harvestReferer = effectiveReferer
        scope.launch {
            runCatching { withContext(Dispatchers.IO) { EnginePipeClient().harvestPage(url, effectiveReferer) } }
                .onSuccess { refreshKey++ }
                .onFailure { notice = UiSignal.Notice("error", it.message ?: "页面抓取失败") }
        }
        harvestDialog = false
    }
    if (settingsDialog) FullSettingsDialog(
        onDismiss = {
            settingsDialog = false
            settingsDeviceScanActive = false
            if (pendingCastTask == null && pendingMediaSource == null) deviceResult = null
        },
        current = settings,
        discoveredDevices = if (settingsDeviceScanActive) deviceResult?.devices.orEmpty() else emptyList(),
        discoveringDevices = settingsDeviceScanActive && castDiscovering,
        onDiscoverDevices = { mode ->
            settingsDeviceScanActive = true
            castDiscovering = true
            deviceResult = UiSignal.Devices(emptyList())
            scope.launch {
                runCatching { withContext(Dispatchers.IO) { EnginePipeClient().discoverCastDevices(mode) } }
                    .onFailure { error ->
                        castDiscovering = false
                        notice = UiSignal.Notice("error", error.message ?: "设备扫描失败")
                    }
            }
        },
        initialTab = auditSettingsTab(visualFixture) ?: "通用",
    ) { updated, defaultCookie, siteRuleCredentialEdits ->
        settings = updated; darkMode = updated.darkMode
        scope.launch {
            runCatching { withContext(Dispatchers.IO) {
                val client = EnginePipeClient()
                var saved = client.saveSettings(updated)
                if (defaultCookie != null) saved = client.saveDefaultCookie(defaultCookie)
                siteRuleCredentialEdits.forEach { edit -> saved = client.saveSiteRuleCredential(edit) }
                saved
            } }
                .onSuccess { saved -> settings = saved; notice = UiSignal.Notice("success", "设置已保存") }
                .onFailure { error -> notice = UiSignal.Notice("error", error.message ?: "设置保存失败") }
        }
    }
    if (queueManagerDialog) QueueManagerDialog(settings.queueProfiles, { queueManagerDialog = false }) { profiles ->
        scope.launch {
            runCatching { withContext(Dispatchers.IO) {
                val client = EnginePipeClient()
                val validIds = profiles.mapTo(mutableSetOf()) { it.id }
                val orphaned = tasks.filter { it.source.queueId !in validIds }.map { it.id }
                if (orphaned.isNotEmpty()) client.assignQueue(orphaned, "default")
                client.saveSettings(settings.copy(queueProfiles = profiles))
            } }.onSuccess { saved ->
                settings = saved
                if (selectedQueueId !in profiles.map { it.id }) selectedQueueId = null
                queueManagerDialog = false
                refreshKey++
                notice = UiSignal.Notice("success", "队列设置已保存")
            }.onFailure { notice = UiSignal.Notice("error", it.message ?: "队列设置保存失败") }
        }
    }
    if (queueAssignTaskIds.isNotEmpty()) QueueAssignDialog(
        queueAssignTaskIds.size,
        settings.queueProfiles,
        { queueAssignTaskIds = emptySet() },
    ) { queueId ->
        val taskIds = queueAssignTaskIds
        queueAssignTaskIds = emptySet()
        scope.launch {
            runCatching { withContext(Dispatchers.IO) { EnginePipeClient().assignQueue(taskIds, queueId) } }
                .onSuccess { refreshKey++; notice = UiSignal.Notice("success", "已移动 ${taskIds.size} 个任务") }
                .onFailure { notice = UiSignal.Notice("error", it.message ?: "移动任务失败") }
        }
    }
    detailTaskId?.let { taskId -> tasks.firstOrNull { it.id == taskId } }?.let { task ->
        TaskDetailsDialog(
            task = task,
            onDismiss = { detailTaskId = null },
            onRefreshRequest = { url, cookie ->
                val requestId = UUID.randomUUID().toString()
                scope.launch {
                    runCatching { withContext(Dispatchers.IO) { EnginePipeClient().refreshTaskRequest(task.id, url, cookie) } }
                        .onSuccess { refreshKey++; notice = UiSignal.Notice("success", if (cookie.isBlank()) "下载地址已更新" else "下载地址和凭据已更新") }
                        .onFailure { error ->
                            UiDiagnostics.error("task_refresh_request", error, task.id, requestId)
                            notice = UiSignal.Notice("error", error.message ?: "更新下载请求失败")
                        }
                }
            },
            onAction = { action -> performTaskAction(task.id, action) },
        )
    }
    if (extensionDialog) ExtensionDialog(extensionText) { extensionDialog = false }
    activeHandoff?.let { offer ->
        LaunchedEffect(offer.handoffId) {
            runCatching { withContext(Dispatchers.IO) { EnginePipeClient().presentHandoff(offer.handoffId) } }
                .onFailure { engineText = Product.engineReconnecting }
        }
        LaunchedEffect(offer.handoffId, handoffBusy) {
            if (handoffBusy) return@LaunchedEffect
            while (handoffQueue.any { it.handoffId == offer.handoffId }) {
                delay(1_500)
                val status = runCatching { withContext(Dispatchers.IO) { EnginePipeClient().loadHandoffStatuses() } }
                    .getOrNull()
                    ?.firstOrNull { it.id == offer.handoffId }
                    ?.status
                if (status != null && status != "pending") {
                    handoffQueue.removeAll { it.handoffId == offer.handoffId }
                }
            }
        }
        BrowserHandoffDialog(
            offer = offer,
            settings = settings,
            duplicate = tasks.firstOrNull { canonicalHandoffUrl(it.source.url) == canonicalHandoffUrl(offer.url) },
            pendingCount = handoffQueue.size,
            busy = handoffBusy,
            onAccept = { decision ->
                handoffBusy = true
                scope.launch {
                    runCatching {
                        withContext(Dispatchers.IO) {
                            val client = EnginePipeClient()
                            client.acceptHandoff(offer.handoffId, decision.filename, decision.directory)
                            if (decision.rememberDirectory && decision.directory.isNotBlank()) {
                                val dirs = buildJsonObject {
                                    put("media", if (decision.category == TaskCategory.MEDIA) decision.directory else settings.categoryDirMedia)
                                    put("program", if (decision.category == TaskCategory.PROGRAM) decision.directory else settings.categoryDirProgram)
                                    put("archive", if (decision.category == TaskCategory.ARCHIVE) decision.directory else settings.categoryDirArchive)
                                    put("other", if (decision.category == TaskCategory.OTHER) decision.directory else settings.categoryDirOther)
                                }
                                client.storeSetting("browser_category_dirs", dirs.toString())
                            }
                        }
                    }
                        .onSuccess { handoffQueue.removeAll { it.handoffId == offer.handoffId }; refreshKey++ }
                        .onFailure { engineText = Product.engineReconnecting }
                    handoffBusy = false
                }
            },
            onReject = { suppressSiteKind ->
                handoffBusy = true
                scope.launch {
                    runCatching { withContext(Dispatchers.IO) { EnginePipeClient().rejectHandoff(offer.handoffId, suppressSiteKind) } }
                        .onSuccess { handoffQueue.removeAll { it.handoffId == offer.handoffId } }
                        .onFailure { engineText = Product.engineReconnecting }
                    handoffBusy = false
                }
            },
        )
    }
    notice?.let { signal -> NoticeToast(signal) { notice = null } }
    probeResult?.let { signal -> ProbeResultDialog(signal, { probeResult = null; probeDraft = null }) { variant ->
        scope.launch {
            runCatching { withContext(Dispatchers.IO) {
                EnginePipeClient().createTask((probeDraft ?: TaskDraft(url = signal.url)).copy(
                    preferredBandwidth = variant?.bandwidth ?: 0,
                    preferredHeight = variant?.height ?: 0,
                    preferredAudio = variant?.name.orEmpty(),
                    queueId = selectedQueueId ?: "default",
                ))
            } }
                .onSuccess { probeResult = null; probeDraft = null; refreshKey++ }
                .onFailure { notice = UiSignal.Notice("error", it.message ?: "创建媒体任务失败") }
        }
    } }
    harvestResult?.let { signal -> HarvestResultDialog(
        signal = signal,
        initialReferer = harvestReferer.ifBlank { signal.url },
        defaultConcurrency = settings.defaultConcurrency,
        probing = harvestProbeBusy,
        onDismiss = { harvestResult = null },
        onProbe = { urls, referer ->
            harvestProbeBusy = true
            scope.launch {
                runCatching {
                    withContext(Dispatchers.IO) { EnginePipeClient().probeHarvestSizes(signal.url, referer, urls) }
                }.onSuccess { sizes ->
                    harvestResult = harvestResult?.let { current ->
                        current.copy(links = mergeHarvestSizes(current.links, sizes))
                    }
                }.onFailure { notice = UiSignal.Notice("error", it.message ?: "读取文件大小失败") }
                harvestProbeBusy = false
            }
        },
    ) { request ->
        scope.launch {
            runCatching {
                withContext(Dispatchers.IO) {
                    request.urls.forEach { url ->
                        EnginePipeClient().createTask(TaskDraft(
                            url = url,
                            referer = request.referer,
                            concurrency = request.concurrency,
                            allowDuplicate = true,
                            queueId = selectedQueueId ?: "default",
                        ))
                    }
                }
            }
                .onSuccess { harvestResult = null; refreshKey++ }
                .onFailure { notice = UiSignal.Notice("error", it.message ?: "创建抓取任务失败") }
        }
    } }
    if (mediaSourceDialog.isNotEmpty()) MediaSourcePickerDialog(mediaSourceDialog, { mediaSourceDialog = "" }) { source ->
        settingsDeviceScanActive = false
        pendingMediaSource = source
        pendingCastTask = null
        pendingCastMode = mediaSourceDialog
        mediaSourceDialog = ""
        castDiscovering = true
        deviceResult = UiSignal.Devices(emptyList())
        scope.launch {
            runCatching { withContext(Dispatchers.IO) { EnginePipeClient().discoverCastDevices(pendingCastMode) } }
                .onFailure { castDiscovering = false; notice = UiSignal.Notice("error", it.message ?: "设备搜索失败") }
        }
    }
    if (!settingsDeviceScanActive) deviceResult?.let { signal -> DevicePickerDialog(signal, pendingCastMode, pendingMediaSource, castDiscovering, castConnecting, settings.preferredCastDeviceId, {
        val requestId = pendingPushRequestId
        deviceResult = null; pendingCastTask = null; pendingMediaSource = null; pendingCastMode = ""; pendingPushRequestId = null
        if (requestId != null) scope.launch { runCatching { withContext(Dispatchers.IO) { EnginePipeClient().resolveMediaPush(requestId, "canceled", "已取消设备选择") } } }
    }, onRescan = {
        castDiscovering = true
        scope.launch { runCatching { withContext(Dispatchers.IO) { EnginePipeClient().discoverCastDevices(pendingCastMode) } }.onFailure { castDiscovering = false; notice = UiSignal.Notice("error", it.message ?: "设备搜索失败") } }
    }, onPublish = {
        val taskId = pendingCastTask
        val media = pendingMediaSource
        if (taskId != null || media != null) scope.launch {
            castConnecting = true
            runCatching { withContext(Dispatchers.IO) {
                if (taskId != null) EnginePipeClient().castTask(taskId)
                else EnginePipeClient().shareMedia(media!!.path, media.url, media.title, "")
            } }
                .onSuccess { result ->
                    val requestId = pendingPushRequestId
                    if (requestId != null) scope.launch { runCatching { withContext(Dispatchers.IO) { EnginePipeClient().resolveMediaPush(requestId, "done", "已发布局域网播放地址") } } }
                    deviceResult = null; pendingCastTask = null; pendingMediaSource = null; pendingCastMode = ""; pendingPushRequestId = null
                }
                .onFailure { error ->
                    val requestId = pendingPushRequestId
                    if (requestId != null) scope.launch { runCatching { withContext(Dispatchers.IO) { EnginePipeClient().resolveMediaPush(requestId, "failed", error.message ?: "局域网发布失败") } } }
                    notice = UiSignal.Notice("error", error.message ?: "局域网发布失败")
                }
            castConnecting = false
        }
    }) { device ->
        val taskId = pendingCastTask
        val media = pendingMediaSource
        if (taskId != null || media != null) scope.launch {
            castConnecting = true
            runCatching { withContext(Dispatchers.IO) {
                if (taskId != null) EnginePipeClient().castToDevice(taskId, device.id)
                else EnginePipeClient().shareMedia(media!!.path, media.url, media.title, device.id)
                EnginePipeClient().storeSetting("preferred_cast_device_id", device.id)
            } }
                .onSuccess {
                    settings = settings.copy(preferredCastDeviceId = device.id)
                    val requestId = pendingPushRequestId
                    if (requestId != null) scope.launch { runCatching { withContext(Dispatchers.IO) { EnginePipeClient().resolveMediaPush(requestId, "done", "已发送到 ${device.label}") } } }
                    deviceResult = null; pendingCastTask = null; pendingMediaSource = null; pendingCastMode = ""; pendingPushRequestId = null
                }
                .onFailure { error ->
                    val requestId = pendingPushRequestId
                    if (requestId != null) scope.launch { runCatching { withContext(Dispatchers.IO) { EnginePipeClient().resolveMediaPush(requestId, "failed", error.message ?: "投屏连接失败") } } }
                    notice = UiSignal.Notice("error", error.message ?: "投屏连接失败")
                }
            castConnecting = false
        }
    } }
    duplicateResult?.let { signal -> DuplicateDialog(signal, { duplicateResult = null }) {
        duplicateResult = null
        if (signal.action == "focus") {
            selected = setOf(signal.taskId)
            detailTaskId = signal.taskId
        } else {
            performTaskAction(signal.taskId, signal.action)
        }
    } }
    updateResult?.let { signal -> UpdateDialog(signal, updateDownloadBusy, { if (!updateDownloadBusy) updateResult = null }, onRelease = {
        runCatching { java.awt.Desktop.getDesktop().browse(URI(signal.releaseUrl)) }
            .onFailure { notice = UiSignal.Notice("error", "无法打开发布页") }
    }) {
        if (!updateDownloadBusy) scope.launch {
            updateDownloadBusy = true
            runCatching { withContext(Dispatchers.IO) { EnginePipeClient().downloadUpdate() } }
                .onSuccess { notice = UiSignal.Notice("success", "安装包已下载并完成身份校验") }
                .onFailure { notice = UiSignal.Notice("error", it.message ?: "下载或校验安装包失败") }
            updateDownloadBusy = false
        }
    } }
    preparedUpdate?.let { signal -> UpdatePreparedDialog(signal, updateDownloadBusy, { if (!updateDownloadBusy) preparedUpdate = null }) {
        if (!updateDownloadBusy) scope.launch {
            updateDownloadBusy = true
            runCatching { withContext(Dispatchers.IO) { EnginePipeClient().installUpdate(ProcessHandle.current().pid()) } }
                .onSuccess {
                    notice = UiSignal.Notice("info", "任务断点已保存，正在关闭工作台并完成升级")
                    delay(100)
                    onExit()
                }
                .onFailure { notice = UiSignal.Notice("error", it.message ?: "无法开始覆盖升级") }
            updateDownloadBusy = false
        }
    } }
    powerPending?.let { signal -> PowerActionDialog(signal, onCancel = {
        powerPending = null
        scope.launch {
            runCatching { withContext(Dispatchers.IO) { EnginePipeClient().cancelPowerAction() } }
                .onSuccess { notice = UiSignal.Notice("info", "已取消完成后电源动作") }
                .onFailure { notice = UiSignal.Notice("error", it.message ?: "取消电源动作失败") }
        }
    }, onConfirm = {
        powerPending = null
        scope.launch {
            runCatching { withContext(Dispatchers.IO) { EnginePipeClient().confirmPowerAction() } }
                .onFailure { notice = UiSignal.Notice("error", it.message ?: "执行电源动作失败") }
        }
    }) }
    playerSession?.takeIf { it.active }?.let { signal -> PlayerSessionHud(signal, playerControlBusy) { action ->
        if (!playerControlBusy) scope.launch {
            playerControlBusy = true
            runCatching { withContext(Dispatchers.IO) { EnginePipeClient().playerControl(action) } }
                .onFailure { notice = UiSignal.Notice("error", it.message ?: "播放器控制失败") }
            playerControlBusy = false
        }
    } }
    castSession?.takeIf { it.active }?.let { signal -> CastSessionHud(signal, tasks.firstOrNull { it.id == signal.taskId }, castControlBusy, playerSession?.active == true, onCopy = { value ->
        runCatching { Toolkit.getDefaultToolkit().systemClipboard.setContents(StringSelection(value), null) }
            .onSuccess { notice = UiSignal.Notice("success", "播放地址已复制") }
            .onFailure { notice = UiSignal.Notice("error", "复制播放地址失败") }
    }) { action, seconds ->
        if (!castControlBusy) scope.launch {
            castControlBusy = true
            runCatching { withContext(Dispatchers.IO) { EnginePipeClient().controlCast(action, seconds) } }
                .onSuccess { if (action == "stop") castSession = null }
                .onFailure { notice = UiSignal.Notice("error", it.message ?: "投屏控制失败") }
            castControlBusy = false
        }
    } }
    destructiveRequest?.let { request -> DestructiveConfirmDialog(request, { destructiveRequest = null }) {
        destructiveRequest = null
        scope.launch {
            runCatching { withContext(Dispatchers.IO) { request.taskIds.forEach { EnginePipeClient().taskAction(it, request.action) } } }
                .onSuccess { selected = selected - request.taskIds; refreshKey++ }
                .onFailure { notice = UiSignal.Notice("error", it.message ?: "删除失败") }
        }
    } }
    }
}

internal fun downloadTask(task: TaskDto): DownloadTask {
    val progress = task.totalBytes?.takeIf { it > 0 }?.let { task.downloadedBytes.toFloat() / it } ?: 0f
    val status = displayStatus(task.status)
    val completedRanges = if (status == "已完成") task.totalRanges else task.completedRanges
    val segments = if (task.totalRanges > 0) {
        "${completedRanges}/${task.totalRanges} ${if (task.resourceKind.equals("torrent", true)) "Piece" else "段"}"
    } else "—"
    return DownloadTask(task.id, task.filename.ifBlank { task.title }, status, progress, formatRate(task.speedBytesPerSecond), task.speedBytesPerSecond, task.etaSeconds?.let(::formatEta) ?: "—", segments, "刚刚", task)
}
internal fun auditTasks(count: Int): List<DownloadTask> = List(count.coerceAtLeast(0)) { index ->
    val status = listOf("running", "queued", "paused", "completed", "failed")[index % 5]
    val total = 64L * 1024 * 1024 + index * 4096L
    val downloaded = when (status) {
        "completed" -> total
        "running" -> total * ((index % 83) + 10) / 100
        "paused" -> total / 3
        else -> 0
    }
    val extension = listOf("mp4", "zip", "exe", "m3u8", "bin")[index % 5]
    downloadTask(
        TaskDto(
            id = "audit-task-$index",
            filename = if (index % 17 == 0) "超长文件名-用于验证省略与布局稳定性-${index.toString().padStart(4, '0')}.$extension" else "任务-${index.toString().padStart(4, '0')}.$extension",
            status = status,
            downloadedBytes = downloaded,
            totalBytes = total,
            speedBytesPerSecond = if (status == "running") 12L * 1024 * 1024 + index * 1024 else 0,
            etaSeconds = if (status == "running") 42L + index % 300 else null,
            activeWorkers = if (status == "running") 8 else 0,
            completedRanges = if (status == "completed") 32 else (index % 31).toLong(),
            totalRanges = 32,
            playbackReady = extension in setOf("mp4", "m3u8") && downloaded > 0,
            url = "https://audit.invalid/files/task-$index.$extension",
            resourceKind = if (extension == "m3u8") "hls" else "file",
            availableActions = when (status) {
                "running" -> listOf("pause", "cancel", "details")
                "paused" -> listOf("resume", "delete", "details")
                "completed" -> listOf("open", "play", "cast", "delete")
                "failed" -> listOf("retry", "delete", "details")
                else -> listOf("start", "delete", "details")
            },
        ),
    )
}
private fun applyEngineEvent(
    tasks: MutableList<DownloadTask>,
    event: kotlinx.serialization.json.JsonObject,
    setExtension: (String) -> Unit,
    offerHandoff: (HandoffOfferDto) -> Unit,
    resolveHandoff: (String) -> Unit,
): UiSignal? {
    return when (event["kind"]?.toString()?.trim('"')) {
        "task_created", "task_updated", "task_progress" -> event["snapshot"]?.jsonObject?.let { snapshot ->
            val task = downloadTask(protocolJson.decodeFromJsonElement(TaskDto.serializer(), snapshot))
            val index = tasks.indexOfFirst { it.id == task.id }
            if (index >= 0) tasks[index] = task else tasks += task
            null
        }
        "task_deleted" -> event["task_id"]?.toString()?.trim('"')?.let { id -> tasks.removeAll { it.id == id }; null }
        "browser_status" -> { setExtension(if (event["connected"]?.toString() == "true") "浏览器插件 · 已连接" else Product.extensionDisconnected); null }
        "clipboard_offer" -> UiSignal.Clipboard(
            event["urls"]?.jsonArray.orEmpty().mapNotNull { item -> runCatching { item.jsonPrimitive.content }.getOrNull() }.filter(String::isNotBlank),
        )
        "handoff_offered" -> event["offer"]?.jsonObject?.let { offer ->
            runCatching { protocolJson.decodeFromJsonElement(HandoffOfferDto.serializer(), offer) }.getOrNull()?.let(offerHandoff)
            null
        }
        "handoff_resolved" -> event["handoff_id"]?.toString()?.trim('"')?.let { resolveHandoff(it); null }
        "probe_result" -> UiSignal.Probe(
            event["url"]?.toString()?.trim('"').orEmpty(),
            event["variants"]?.jsonArray.orEmpty().mapNotNull { runCatching { protocolJson.decodeFromJsonElement(StreamVariantDto.serializer(), it) }.getOrNull() },
        )
        "torrent_probe_result" -> UiSignal.TorrentProbe(
            TorrentProbeDto(
                source = event["source"]?.jsonPrimitive?.content.orEmpty(),
                name = event["name"]?.jsonPrimitive?.content ?: "torrent",
                totalSize = event["total_size"]?.jsonPrimitive?.content?.toLongOrNull() ?: 0,
                files = event["files"]?.jsonArray.orEmpty().mapNotNull { item -> runCatching { protocolJson.decodeFromJsonElement(TorrentFileDto.serializer(), item) }.getOrNull() },
                magnet = event["magnet"]?.jsonPrimitive?.content == "true",
            ),
        )
        "torrent_selection_result" -> UiSignal.TorrentSelection(
            source = event["source"]?.jsonPrimitive?.content.orEmpty(),
            files = event["selections"]?.jsonArray.orEmpty().mapNotNull { item -> runCatching { protocolJson.decodeFromJsonElement(TorrentFileDto.serializer(), item) }.getOrNull() },
            totalSize = event["total_size"]?.jsonPrimitive?.content?.toLongOrNull() ?: 0,
        )
        "cast_devices" -> UiSignal.Devices(event["devices"]?.jsonArray.orEmpty().mapNotNull { runCatching { protocolJson.decodeFromJsonElement(CastDeviceDto.serializer(), it) }.getOrNull() })
        "media_push_requested" -> event["request"]?.jsonObject?.let { request -> runCatching { UiSignal.MediaPush(protocolJson.decodeFromJsonElement(MediaPushRequestDto.serializer(), request)) }.getOrNull() }
        "media_push_resolved" -> event["request"]?.jsonObject?.let { request -> runCatching { UiSignal.MediaPushResolved(protocolJson.decodeFromJsonElement(MediaPushRequestDto.serializer(), request)) }.getOrNull() }
        "power_action_pending" -> UiSignal.PowerPending(
            event["action"]?.jsonPrimitive?.content.orEmpty(),
            event["title"]?.jsonPrimitive?.content.orEmpty(),
            event["delay_seconds"]?.jsonPrimitive?.content?.toLongOrNull() ?: 30,
        )
        "harvest_result" -> UiSignal.Harvest(
            event["url"]?.toString()?.trim('"').orEmpty(),
            event["links"]?.jsonArray.orEmpty().mapNotNull { item ->
                item.jsonObject.let { value ->
                    HarvestCandidateUi(
                        url = value["url"]?.jsonPrimitive?.content.orEmpty(),
                        filename = value["filename"]?.jsonPrimitive?.content.orEmpty(),
                        category = value["category"]?.jsonPrimitive?.content.orEmpty(),
                        size = value["size"]?.jsonPrimitive?.content?.toLongOrNull() ?: 0,
                        extension = value["extension"]?.jsonPrimitive?.content.orEmpty(),
                    )
                }.takeIf { it.url.isNotBlank() }
            },
        )
        "task_log" -> UiSignal.Log(event["task_id"]?.jsonPrimitive?.content.orEmpty(), event["lines"]?.jsonArray.orEmpty().map { it.jsonPrimitive.content })
        "duplicate_offered" -> UiSignal.Duplicate(event["task_id"]?.jsonPrimitive?.content.orEmpty(), event["action"]?.jsonPrimitive?.content.orEmpty(), event["message"]?.jsonPrimitive?.content ?: "发现重复任务")
        "toast" -> UiSignal.Notice(event["level"]?.jsonPrimitive?.content ?: "info", event["message"]?.jsonPrimitive?.content ?: "操作已完成")
        "update_available" -> UiSignal.Update(
            current = event["current"]?.jsonPrimitive?.content.orEmpty(),
            latest = event["latest"]?.jsonPrimitive?.content.orEmpty(),
            notes = event["notes"]?.jsonPrimitive?.content.orEmpty(),
            releaseUrl = event["release_url"]?.jsonPrimitive?.content.orEmpty(),
            installerName = event["installer_name"]?.jsonPrimitive?.content.orEmpty(),
            installerSize = event["installer_size"]?.jsonPrimitive?.content?.toLongOrNull() ?: 0,
            sha256Verified = event["sha256_verified"]?.jsonPrimitive?.content == "true",
        )
        "update_current" -> UiSignal.Notice("success", "已是最新版本 ${event["current"]?.jsonPrimitive?.content.orEmpty()}")
        "update_ready" -> UiSignal.UpdatePrepared(
            latest = event["latest"]?.jsonPrimitive?.content.orEmpty(),
            installerPath = event["installer_path"]?.jsonPrimitive?.content.orEmpty(),
            sha256 = event["sha256"]?.jsonPrimitive?.content.orEmpty(),
            productName = event["product_name"]?.jsonPrimitive?.content.orEmpty(),
            productVersion = event["product_version"]?.jsonPrimitive?.content.orEmpty(),
            upgradeCode = event["upgrade_code"]?.jsonPrimitive?.content.orEmpty(),
        )
        "update_install_started" -> UiSignal.Notice("info", "任务断点已保存，正在关闭并安装版本 ${event["latest"]?.jsonPrimitive?.content.orEmpty()}")
        "update_install_result" -> UiSignal.Notice(
            if (event["status"]?.jsonPrimitive?.content == "success") "success" else "error",
            event["message"]?.jsonPrimitive?.content ?: "覆盖升级结果未知",
        )
        "error" -> UiSignal.Notice("error", event["message"]?.jsonPrimitive?.content ?: "操作失败")
        "cast_session" -> UiSignal.Cast(
            active = event["active"]?.jsonPrimitive?.content == "true",
            title = event["title"]?.jsonPrimitive?.content.orEmpty(),
            device = event["device"]?.jsonPrimitive?.content.orEmpty(),
            status = event["status"]?.jsonPrimitive?.content.orEmpty(),
            taskId = event["task_id"]?.jsonPrimitive?.content.orEmpty(),
            mediaUrl = event["media_url"]?.jsonPrimitive?.content.orEmpty(),
            deviceKind = event["device_kind"]?.jsonPrimitive?.content.orEmpty(),
            supportedActions = event["supported_actions"]?.jsonArray.orEmpty().map { it.jsonPrimitive.content },
            playing = event["playing"]?.jsonPrimitive?.content == "true",
            paused = event["paused"]?.jsonPrimitive?.content == "true",
            positionSeconds = event["position_seconds"]?.jsonPrimitive?.content?.toLongOrNull() ?: 0,
            durationSeconds = event["duration_seconds"]?.jsonPrimitive?.content?.toLongOrNull() ?: 0,
            positionAvailable = event["position_available"]?.jsonPrimitive?.content == "true",
        )
        "player_session" -> UiSignal.Player(
            active = event["active"]?.jsonPrimitive?.content == "true",
            title = event["title"]?.jsonPrimitive?.content.orEmpty(),
            taskId = event["task_id"]?.jsonPrimitive?.content.orEmpty(),
            status = event["status"]?.jsonPrimitive?.content.orEmpty(),
            paused = event["paused"]?.jsonPrimitive?.content == "true",
            speed = event["speed"]?.jsonPrimitive?.content?.toDoubleOrNull() ?: 1.0,
            positionSeconds = event["position_seconds"]?.jsonPrimitive?.content?.toDoubleOrNull() ?: 0.0,
            durationSeconds = event["duration_seconds"]?.jsonPrimitive?.content?.toDoubleOrNull() ?: 0.0,
            positionAvailable = event["position_available"]?.jsonPrimitive?.content == "true",
            audioTracks = event["audio_tracks"]?.jsonPrimitive?.content?.toIntOrNull() ?: 0,
            subtitleTracks = event["subtitle_tracks"]?.jsonPrimitive?.content?.toIntOrNull() ?: 0,
        )
        else -> null
    }
}

private inline fun handleSignal(signal: UiSignal?, consume: (UiSignal) -> Unit) { if (signal != null) consume(signal) }
private fun displayStatus(status: String) = when (status.lowercase()) {
    "completed", "done" -> "已完成"
    "running", "downloading" -> "进行中"
    "recording" -> "录制中"
    "merging" -> "合并中"
    "checking" -> "校验中"
    "paused" -> "已暂停"
    "canceled", "cancelled" -> "已取消"
    "failed", "error" -> "失败"
    else -> "排队中"
}
internal fun taskCategory(task: DownloadTask) = when (task.filename.substringAfterLast('.', "").lowercase()) { "m3u8", "mpd", "mp4", "mkv", "webm", "mp3", "flac", "wav", "avi", "mov" -> TaskCategory.MEDIA; "exe", "msi", "appx", "apk", "dmg", "pkg" -> TaskCategory.PROGRAM; "zip", "rar", "7z", "tar", "gz", "bz2", "xz" -> TaskCategory.ARCHIVE; else -> TaskCategory.OTHER }
internal fun visibleTasks(tasks: List<DownloadTask>, filter: TaskFilter, category: TaskCategory?, query: String, queueId: String? = null): List<DownloadTask> =
    tasks.filter { (filter == TaskFilter.ALL || it.status == filter.label) && (category == null || taskCategory(it) == category) && (queueId == null || it.source.queueId == queueId) && it.filename.contains(query, true) }
private fun formatRate(bytes: Long): String = when { bytes <= 0 -> "—"; bytes >= 1024L * 1024L -> "%.1f MB/s".format(bytes / 1024.0 / 1024.0); else -> "%.0f KB/s".format(bytes / 1024.0) }
private fun formatEta(seconds: Long): String = when { seconds < 60 -> "${seconds}s"; seconds < 3600 -> "${seconds / 60} 分钟"; else -> "${seconds / 3600} 小时" }
internal fun chooseDirectory(initialPath: String = "", title: String = "选择目录"): String? {
    val chooser = JFileChooser().apply {
        dialogTitle = title
        fileSelectionMode = JFileChooser.DIRECTORIES_ONLY
        isAcceptAllFileFilterUsed = false
        if (initialPath.isNotBlank()) currentDirectory = java.io.File(initialPath).let { if (it.isDirectory) it else it.parentFile }
    }
    return if (chooser.showOpenDialog(null) == JFileChooser.APPROVE_OPTION) chooser.selectedFile.absolutePath else null
}

private fun chooseImportPaths(): List<String>? {
    val chooser = JFileChooser().apply {
        dialogTitle = "导入任务 JSON、种子、Metalink 或 URL 列表"
        isMultiSelectionEnabled = true
        fileFilter = FileNameExtensionFilter("支持的导入文件", "json", "torrent", "metalink", "meta4", "txt", "urls")
    }
    return if (chooser.showOpenDialog(null) == JFileChooser.APPROVE_OPTION) chooser.selectedFiles.map { it.absolutePath } else null
}

private fun chooseExportPath(): java.nio.file.Path? {
    val chooser = JFileChooser().apply {
        dialogTitle = "导出任务列表"
        selectedFile = java.io.File("hls-downloader-tasks.json")
        fileFilter = FileNameExtensionFilter("JSON、CSV 或 URL 列表", "json", "csv", "txt")
    }
    return if (chooser.showSaveDialog(null) == JFileChooser.APPROVE_OPTION) chooser.selectedFile.toPath() else null
}

private fun chooseMediaPath(): String? {
    val chooser = JFileChooser().apply {
        dialogTitle = "选择要投屏或推送的媒体文件"
        isMultiSelectionEnabled = false
        fileFilter = FileNameExtensionFilter("视频和音频文件", "mp4", "mkv", "webm", "mov", "avi", "m4v", "mp3", "flac", "wav", "m4a", "aac", "ts", "m3u8")
    }
    return if (chooser.showOpenDialog(null) == JFileChooser.APPROVE_OPTION) chooser.selectedFile.absolutePath else null
}

private fun exportTaskList(path: java.nio.file.Path, taskIds: List<String>): TaskExportResult {
    val extension = path.fileName.toString().substringAfterLast('.', "json").lowercase()
    val format = when (extension) {
        "csv" -> "csv"
        "txt", "urls" -> "urls"
        else -> "json"
    }
    val result = EnginePipeClient().exportTasks(taskIds, format)
    path.parent?.let(Files::createDirectories)
    Files.writeString(path, result.data, Charsets.UTF_8)
    return result
}
internal fun loadDesktopIcon(): ImageBitmap? = runCatching {
    val bytes = checkNotNull(Thread.currentThread().contextClassLoader.getResourceAsStream("app-icon.png")) { "app-icon.png is missing" }.use { it.readBytes() }
    SkiaImage.makeFromEncoded(bytes).toComposeImageBitmap()
}.getOrNull()

@Composable private fun WindowScope.WindowTitleBar(appIcon: ImageBitmap?, maximized: Boolean, onMinimize: () -> Unit, onToggleMaximize: () -> Unit, onClose: () -> Unit) {
    WindowDraggableArea(Modifier.fillMaxWidth().height(34.dp)) {
        Row(Modifier.fillMaxSize().background(surface2).padding(start = 11.dp), verticalAlignment = Alignment.CenterVertically) {
            if (appIcon != null) Image(appIcon, null, modifier = Modifier.size(17.dp)) else Icon(Icons.Outlined.Downloading, null, tint = blue, modifier = Modifier.size(16.dp))
            Spacer(Modifier.width(7.dp))
            Text("HLS Downloader ${Product.version}", color = ink, fontSize = 11.sp, fontWeight = FontWeight.Medium)
            Spacer(Modifier.weight(1f))
            IconButton(onClick = onMinimize, modifier = Modifier.width(45.dp).fillMaxHeight()) { Icon(Icons.Outlined.Minimize, "最小化", tint = muted, modifier = Modifier.size(16.dp)) }
            IconButton(onClick = onToggleMaximize, modifier = Modifier.width(45.dp).fillMaxHeight()) { Icon(if (maximized) Icons.Outlined.FilterNone else Icons.Outlined.CropSquare, if (maximized) "还原" else "最大化", tint = muted, modifier = Modifier.size(14.dp)) }
            IconButton(onClick = onClose, modifier = Modifier.width(45.dp).fillMaxHeight()) { Icon(Icons.Outlined.Close, "关闭", tint = muted, modifier = Modifier.size(17.dp)) }
        }
    }
}

@Composable private fun DesktopToolbar(query: String, onQuery: (String) -> Unit, onNew: () -> Unit, onPaste: () -> Unit, onBatch: () -> Unit, onHarvest: () -> Unit, onStartAll: () -> Unit, onPauseAll: () -> Unit, onCastMedia: () -> Unit, onPushMedia: () -> Unit, onExtension: () -> Unit, onSettings: () -> Unit, dark: Boolean, onTheme: () -> Unit) {
    BoxWithConstraints(Modifier.height(52.dp).fillMaxWidth().background(rail).border(BorderStroke(1.dp, border))) {
        val compact = maxWidth < 1180.dp
        val narrow = maxWidth < 980.dp
        @Composable fun Action(id: String) {
            val (icon, label, action) = when (id) {
                "new" -> Triple(Icons.Outlined.Add, "新建", onNew)
                "paste" -> Triple(Icons.Outlined.ContentPaste, "粘贴链接", onPaste)
                "batch" -> Triple(Icons.AutoMirrored.Outlined.PlaylistAdd, "批量添加", onBatch)
                "harvest" -> Triple(Icons.Outlined.TravelExplore, "页面抓取", onHarvest)
                "start_all" -> Triple(Icons.Outlined.PlayArrow, "全部开始", onStartAll)
                "pause_all" -> Triple(Icons.Outlined.Pause, "全部暂停", onPauseAll)
                "cast" -> Triple(Icons.Outlined.Cast, "投屏到电视", onCastMedia)
                "tvbox" -> Triple(Icons.Outlined.Tv, "TVBox 推送", onPushMedia)
                "extension" -> Triple(Icons.Outlined.Extension, "插件", onExtension)
                else -> return
            }
            if (id == "new") ToolbarButton(icon, label, action, true)
            else if (compact) ToolbarIcon(icon, label, action)
            else ToolbarButton(icon, label, action)
        }
        Row(Modifier.fillMaxSize().padding(horizontal = if (narrow) 8.dp else 14.dp), verticalAlignment = Alignment.CenterVertically) {
            listOf("new", "paste", "batch", "harvest", "start_all", "pause_all", "cast", "tvbox", "extension").forEach { Action(it) }
            Spacer(Modifier.weight(1f))
            ToolbarSearchField(query, onQuery, narrow)
            Spacer(Modifier.width(5.dp))
            if (compact) ToolbarIcon(Icons.Outlined.Settings, "设置", onSettings) else ToolbarButton(Icons.Outlined.Settings, "设置", onSettings)
            ToolbarIcon(if (dark) Icons.Outlined.LightMode else Icons.Outlined.DarkMode, if (dark) "浅色模式" else "深色模式", onTheme)
        }
    }
}
@Composable private fun ToolbarSearchField(value: String, onValue: (String) -> Unit, narrow: Boolean) {
    var focused by remember { mutableStateOf(false) }
    Row(
        Modifier.width(if (narrow) 145.dp else 190.dp).height(36.dp).clip(RoundedCornerShape(7.dp))
            .background(if (focused) rail else surface2).border(1.dp, if (focused) blue else border, RoundedCornerShape(7.dp)).padding(horizontal = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(Icons.Outlined.Search, "搜索", modifier = Modifier.size(17.dp), tint = muted); Spacer(Modifier.width(8.dp))
        BasicTextField(
            value = value,
            onValueChange = onValue,
            modifier = Modifier.weight(1f).onFocusChanged { focused = it.isFocused },
            singleLine = true,
            textStyle = TextStyle(color = ink, fontSize = 11.sp),
            cursorBrush = SolidColor(blue),
            decorationBox = { field -> if (value.isEmpty()) Text(if (narrow) "搜索" else "搜索任务", color = faint, fontSize = 11.sp, maxLines = 1); field() },
        )
    }
}
@Composable private fun ToolbarButton(icon: ImageVector, label: String, action: () -> Unit, primary: Boolean = false) { Button(onClick = action, colors = ButtonDefaults.buttonColors(containerColor = if (primary) blue else Color.Transparent, contentColor = if (primary) Color.White else ink), border = if (primary) null else BorderStroke(1.dp, Color.Transparent), shape = RoundedCornerShape(7.dp), contentPadding = PaddingValues(horizontal = 8.dp), modifier = Modifier.height(36.dp).padding(horizontal = 1.dp)) { Icon(icon, null, Modifier.size(16.dp)); Spacer(Modifier.width(5.dp)); Text(label, fontSize = 11.sp, fontWeight = if (primary) FontWeight.SemiBold else FontWeight.Medium) } }
@Composable private fun ToolbarIcon(icon: ImageVector, text: String, action: () -> Unit) = WorkbenchTooltip(text) { IconButton(onClick = action, modifier = Modifier.size(36.dp)) { Icon(icon, text, tint = muted, modifier = Modifier.size(18.dp)) } }

@Composable private fun Sidebar(selected: TaskFilter, selectedCategory: TaskCategory?, selectedQueueId: String?, profiles: List<QueueProfileDto>, tasks: List<DownloadTask>, engine: String, extension: String, onSelected: (TaskFilter) -> Unit, onCategory: (TaskCategory) -> Unit, onQueue: (String) -> Unit, onManageQueues: () -> Unit) {
    val scrollState = rememberScrollState()
    Column(Modifier.width(190.dp).fillMaxHeight().background(rail).border(BorderStroke(1.dp, border)).padding(horizontal = 9.dp, vertical = 11.dp)) {
        Text("任务状态", color = muted, fontSize = 11.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(horizontal = 10.dp, vertical = 5.dp))
        Box(Modifier.weight(1f).fillMaxWidth()) {
        Column(Modifier.fillMaxSize().verticalScroll(scrollState).padding(end = 5.dp)) {
        TaskFilter.entries.forEach { item ->
            val active = item == selected
            Row(Modifier.fillMaxWidth().height(36.dp).clip(RoundedCornerShape(7.dp)).background(if (active) selectedSurface else Color.Transparent).selectable(selected = active, role = Role.Tab) { onSelected(item) }.padding(horizontal = 10.dp), verticalAlignment = Alignment.CenterVertically) {
                Icon(categoryIcon(item), null, tint = if (active) blue else muted, modifier = Modifier.size(17.dp)); Spacer(Modifier.width(8.dp)); Text(item.label, color = if (active) ink else muted, fontSize = 13.sp, fontWeight = if (active) FontWeight.SemiBold else FontWeight.Normal); Spacer(Modifier.weight(1f)); Text(taskCount(tasks, item).toString(), color = if (active) blue else muted, fontSize = 11.sp, fontWeight = if (active) FontWeight.SemiBold else FontWeight.Normal)
            }
        }
        Spacer(Modifier.height(14.dp)); Row(Modifier.fillMaxWidth().padding(start = 10.dp, end = 2.dp, bottom = 5.dp), verticalAlignment = Alignment.CenterVertically) {
            Text("下载队列", color = muted, fontSize = 11.sp, fontWeight = FontWeight.SemiBold); Spacer(Modifier.weight(1f)); WorkbenchTooltip("管理队列") { IconButton(onClick = onManageQueues, modifier = Modifier.size(26.dp)) { Icon(Icons.Outlined.SettingsSuggest, "管理队列", tint = muted, modifier = Modifier.size(15.dp)) } }
        }
        profiles.sortedByDescending { it.priority }.forEach { profile ->
            val active = profile.id == selectedQueueId
            Row(Modifier.fillMaxWidth().height(36.dp).clip(RoundedCornerShape(7.dp)).background(if (active) selectedSurface else Color.Transparent).selectable(selected = active, role = Role.Tab) { onQueue(profile.id) }.padding(horizontal = 10.dp), verticalAlignment = Alignment.CenterVertically) {
                Icon(if (profile.enabled) Icons.AutoMirrored.Outlined.PlaylistPlay else Icons.Outlined.PauseCircleOutline, null, tint = if (active) blue else muted, modifier = Modifier.size(17.dp)); Spacer(Modifier.width(8.dp)); Text(profile.name, color = if (active) ink else muted, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f)); Text(tasks.count { it.source.queueId == profile.id }.toString(), color = if (active) blue else faint, fontSize = 11.sp)
            }
        }
        Spacer(Modifier.height(14.dp)); Text("分类", color = muted, fontSize = 11.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(horizontal = 10.dp, vertical = 5.dp))
        TaskCategory.entries.forEach { item ->
            val active = item == selectedCategory
            Row(Modifier.fillMaxWidth().height(36.dp).clip(RoundedCornerShape(7.dp)).background(if (active) selectedSurface else Color.Transparent).selectable(selected = active, role = Role.Tab) { onCategory(item) }.padding(horizontal = 10.dp), verticalAlignment = Alignment.CenterVertically) { Icon(categoryIcon(item), null, tint = if (active) blue else muted, modifier = Modifier.size(17.dp)); Spacer(Modifier.width(8.dp)); Text(item.label, color = if (active) ink else muted, fontSize = 13.sp, fontWeight = if (active) FontWeight.SemiBold else FontWeight.Normal); Spacer(Modifier.weight(1f)); Text(tasks.count { taskCategory(it) == item }.toString(), color = if (active) blue else muted, fontSize = 11.sp, fontWeight = if (active) FontWeight.SemiBold else FontWeight.Normal) }
        }
        }
        VerticalScrollbar(rememberScrollbarAdapter(scrollState), Modifier.align(Alignment.CenterEnd).fillMaxHeight().width(6.dp))
        }
        HorizontalDivider(color = border); Spacer(Modifier.height(10.dp)); Text(engine, color = if (engine == Product.engineConnected) Color(0xFF159447) else Color(0xFFD97706), fontSize = 12.sp, modifier = Modifier.padding(horizontal = 4.dp)); Text(extension, color = if (extension.contains("已连接")) Color(0xFF159447) else muted, fontSize = 12.sp, modifier = Modifier.padding(horizontal = 4.dp, vertical = 3.dp))
    }
}
private fun categoryIcon(filter: TaskFilter): ImageVector = when (filter) { TaskFilter.RUNNING -> Icons.Outlined.Downloading; TaskFilter.QUEUED -> Icons.Outlined.Schedule; TaskFilter.PAUSED -> Icons.Outlined.PauseCircle; TaskFilter.COMPLETED -> Icons.Outlined.CheckCircle; TaskFilter.FAILED -> Icons.Outlined.ErrorOutline; else -> Icons.Outlined.Folder }
private fun categoryIcon(category: TaskCategory): ImageVector = when (category) { TaskCategory.MEDIA -> Icons.Outlined.Image; TaskCategory.PROGRAM -> Icons.Outlined.Apps; TaskCategory.ARCHIVE -> Icons.Outlined.FolderZip; TaskCategory.OTHER -> Icons.AutoMirrored.Outlined.InsertDriveFile }

@Composable private fun ContentHeader(filter: TaskFilter, count: Int, hasSelection: Boolean, hasCompleted: Boolean, onRefresh: () -> Unit, onClearCompleted: () -> Unit, onMore: (String) -> Unit, onSelectedAction: (String) -> Unit) {
    var menuOpen by remember { mutableStateOf(false) }
    BoxWithConstraints(Modifier.height(42.dp).fillMaxWidth().background(surface2).border(BorderStroke(1.dp, border))) {
        val compact = maxWidth < 940.dp
        Row(Modifier.fillMaxSize().padding(horizontal = if (compact) 8.dp else 14.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(filter.label, color = ink, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
            Spacer(Modifier.width(8.dp)); Text("$count 项", color = faint, fontSize = 11.sp)
            if (hasSelection) {
                Spacer(Modifier.width(if (compact) 8.dp else 16.dp))
                if (!compact) Text("已选择", color = blue, fontSize = 11.sp, fontWeight = FontWeight.SemiBold)
                SelectionAction(Icons.Outlined.FileDownload, "开始") { onSelectedAction("start") }; SelectionAction(Icons.Outlined.Pause, "暂停") { onSelectedAction("pause") }; SelectionAction(Icons.AutoMirrored.Outlined.DriveFileMove, "移动队列") { onSelectedAction("move_queue") }; SelectionAction(Icons.Outlined.PlayCircle, "播放") { onSelectedAction("play") }; SelectionAction(Icons.Outlined.Cast, "投屏") { onSelectedAction("cast") }; SelectionAction(Icons.Outlined.Tv, "TVBox 推送") { onSelectedAction("push_tvbox") }; SelectionAction(Icons.Outlined.DeleteOutline, "删除") { onSelectedAction("delete") }
            } else if (!compact) {
                Spacer(Modifier.width(14.dp)); Text("选择任务后可进行批量操作", color = faint, fontSize = 11.sp)
            }
            Spacer(Modifier.weight(1f))
            if (compact) {
                ToolbarIcon(Icons.Outlined.DeleteSweep, "清理已完成") { if (hasCompleted) onClearCompleted() }; ToolbarIcon(Icons.Outlined.Refresh, "刷新", onRefresh)
            } else {
                TextButton(onClick = onClearCompleted, enabled = hasCompleted, contentPadding = PaddingValues(horizontal = 8.dp)) { Icon(Icons.Outlined.DeleteSweep, null, Modifier.size(15.dp)); Spacer(Modifier.width(4.dp)); Text("清理已完成", fontSize = 11.sp) }; TextButton(onClick = onRefresh, contentPadding = PaddingValues(horizontal = 8.dp)) { Icon(Icons.Outlined.Refresh, null, Modifier.size(15.dp)); Spacer(Modifier.width(4.dp)); Text("刷新", fontSize = 11.sp) }
            }
            Box { ToolbarIcon(Icons.Outlined.MoreHoriz, "更多操作") { menuOpen = true }; DropdownMenu(menuOpen, { menuOpen = false }, shape = RoundedCornerShape(7.dp), containerColor = dialogSurface, shadowElevation = 6.dp) { listOf("import" to "导入任务或种子", "export" to "导出任务列表", "update" to "检查更新", "cancel_power" to "取消完成后电源动作").forEach { (action, label) -> DropdownMenuItem(text = { Text(label, fontSize = 12.sp) }, onClick = { menuOpen = false; onMore(action) }) } } }
        }
    }
}
@Composable private fun SelectionAction(icon: ImageVector, label: String, onClick: () -> Unit) = ToolbarIcon(icon, label, onClick)
private data class TaskContextMenuRequest(
    val task: DownloadTask,
    val targets: List<DownloadTask>,
    val position: IntOffset,
)
@OptIn(ExperimentalComposeUiApi::class)
@Composable private fun TaskTable(tasks: List<DownloadTask>, selected: Set<String>, appIcon: ImageBitmap?, taskSort: String, onSelection: (Set<String>) -> Unit, onDetails: (DownloadTask) -> Unit, onDeleteSelection: () -> Unit, onQueueMove: (String, Int) -> Unit, modifier: Modifier = Modifier, onBatchAction: (Set<String>, String) -> Unit, onSort: (String) -> Unit, onAction: (String, String) -> Unit) {
    var anchorId by remember { mutableStateOf<String?>(null) }
    var tableFocused by remember { mutableStateOf(false) }
    var contextMenu by remember { mutableStateOf<TaskContextMenuRequest?>(null) }
    val focusRequester = remember { FocusRequester() }
    fun selectTask(id: String, shift: Boolean, toggle: Boolean): Set<String> {
        val next = selectionAfterClick(tasks.map { it.id }, selected, anchorId, id, shift, toggle)
        anchorId = id
        onSelection(next)
        return next
    }
    SideEffect {
        UiTestState.installTaskSelector { index, shift, toggle ->
            tasks.getOrNull(index)?.let { selectTask(it.id, shift, toggle) } ?: selected
        }
        UiTestState.installTaskContextOpener { index, x, y ->
            val task = tasks.getOrNull(index) ?: return@installTaskContextOpener
            val targets = if (task.id in selected) tasks.filter { it.id in selected } else listOf(task)
            val position = IntOffset(x, y)
            val actions = batchTaskMenuActions(targets.map { it.source })
            contextMenu = TaskContextMenuRequest(task, targets, position)
            UiTestState.updateContextMenu(position, targets.mapTo(mutableSetOf()) { it.id }, actions)
        }
    }
    DisposableEffect(Unit) { onDispose { UiTestState.installTaskSelector(null); UiTestState.installTaskContextOpener(null) } }
    BoxWithConstraints(
        modifier.fillMaxWidth().background(rail)
            .semantics { contentDescription = "下载任务列表，共 ${tasks.size} 项，已选择 ${selected.size} 项" }
            .focusRequester(focusRequester).onFocusChanged { tableFocused = it.isFocused }.focusable()
            .onPreviewKeyEvent { event ->
                if (event.type != KeyEventType.KeyDown) return@onPreviewKeyEvent false
                when {
                    event.isCtrlPressed && event.key == Key.A -> { onSelection(tasks.mapTo(mutableSetOf()) { it.id }); anchorId = tasks.lastOrNull()?.id; true }
                    event.key == Key.Escape && selected.isNotEmpty() -> { onSelection(emptySet()); anchorId = null; true }
                    event.key == Key.Delete && selected.isNotEmpty() -> { onDeleteSelection(); true }
                    event.key == Key.Enter && selected.isNotEmpty() -> { tasks.firstOrNull { it.id in selected }?.let(onDetails); true }
                    event.key == Key.DirectionDown || event.key == Key.DirectionUp -> {
                        if (tasks.isEmpty()) return@onPreviewKeyEvent false
                        val current = tasks.indexOfFirst { it.id == anchorId }.let { if (it < 0) 0 else it }
                        val next = (current + if (event.key == Key.DirectionDown) 1 else -1).coerceIn(0, tasks.lastIndex)
                        selectTask(tasks[next].id, event.isShiftPressed, false); true
                    }
                    else -> false
                }
            }.border(1.dp, if (tableFocused) blue.copy(alpha = .7f) else Color.Transparent)
    ) {
        val columns = resolveTaskColumns(maxWidth)
        val tableWidth = maxOf(maxWidth, columns.requiredWidth)
        val horizontalState = rememberScrollState()
        Box(Modifier.fillMaxSize().padding(bottom = if (tableWidth > maxWidth) 9.dp else 0.dp).horizontalScroll(horizontalState)) {
            Column(Modifier.width(tableWidth).fillMaxHeight()) {
                Row(Modifier.fillMaxWidth().height(36.dp).background(surface3).border(BorderStroke(1.dp, border)).padding(horizontal = 15.dp), verticalAlignment = Alignment.CenterVertically) {
                    columns.items.forEach { column ->
                        key(column.id) {
                            if (column.id == "name") {
                                Row(Modifier.width(column.width), verticalAlignment = Alignment.CenterVertically) {
                                    Checkbox(tasks.isNotEmpty() && selected.size == tasks.size, { checked -> onSelection(if (checked) tasks.mapTo(mutableSetOf()) { it.id } else emptySet()) }, Modifier.size(18.dp), accessibilityLabel = "选择全部任务")
                                    Spacer(Modifier.width(8.dp))
                                    TaskHeader(column.label, Modifier.weight(1f), column.id, taskSort, onSort)
                                }
                            } else {
                                TaskHeader(column.label, Modifier.width(column.width), column.id, taskSort, onSort)
                            }
                        }
                    }
                    Spacer(Modifier.weight(1f))
                }
                if (tasks.isEmpty()) {
                    Box(Modifier.fillMaxWidth().weight(1f), contentAlignment = Alignment.Center) {
                        Column(horizontalAlignment = Alignment.CenterHorizontally) {
                            if (appIcon != null) Image(appIcon, "HLS Downloader", modifier = Modifier.size(62.dp))
                            else Icon(Icons.Outlined.Downloading, "下载任务", tint = blue, modifier = Modifier.size(34.dp))
                            Spacer(Modifier.height(14.dp))
                            Text("这里还没有下载任务", color = ink, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
                            Spacer(Modifier.height(4.dp))
                            Text("点击工具栏“新建”或粘贴链接开始下载", color = muted, fontSize = 12.sp)
                        }
                    }
                } else {
                    val listState = rememberLazyListState()
                    var dragStart by remember { mutableStateOf<Offset?>(null) }
                    var dragCurrent by remember { mutableStateOf<Offset?>(null) }
                    var dragBaseSelection by remember { mutableStateOf<Set<String>>(emptySet()) }
                    var dragAdditive by remember { mutableStateOf(false) }
                    var dragSelecting by remember { mutableStateOf(false) }
                    var suppressRowClickUntilNanos by remember { mutableLongStateOf(0L) }
                    val selectionBlue = blue
                    val frameReportPath = remember { System.getenv("HLS_UI_FRAME_REPORT").orEmpty() }
                    LaunchedEffect(tasks.size, frameReportPath) {
                        if (tasks.size >= 1_000 && frameReportPath.isNotBlank()) {
                            val settleDelayMs = 3_000L
                            delay(settleDelayMs)
                            val focusDeadline = System.nanoTime() + 15_000_000_000L
                            while (System.getProperty("hls.audit.window.active") != "true" && System.nanoTime() < focusDeadline) {
                                delay(100)
                            }
                            val windowActiveAtStart = System.getProperty("hls.audit.window.active") == "true"
                            val warmupJumpCount = 100
                            repeat(warmupJumpCount) { sample ->
                                listState.scrollToItem((sample * 47) % tasks.size)
                                withFrameNanos { }
                            }
                            listState.scrollToItem(0)
                            withFrameNanos { }
                            val scrollStepPx = 52f
                            val warmupScrollCount = 120
                            repeat(warmupScrollCount) {
                                listState.scrollBy(scrollStepPx)
                                withFrameNanos { }
                            }
                            val idleLatencies = ArrayList<Double>(60)
                            var previousFrame = withFrameNanos { it }
                            repeat(60) {
                                val idleFrame = withFrameNanos { it }
                                idleLatencies += (idleFrame - previousFrame) / 1_000_000.0
                                previousFrame = idleFrame
                            }
                            val renderLatencies = ArrayList<Double>(180)
                            repeat(180) {
                                listState.dispatchRawDelta(scrollStepPx)
                                val presentedFrame = withFrameNanos { it }
                                renderLatencies += (presentedFrame - previousFrame) / 1_000_000.0
                                previousFrame = presentedFrame
                            }
                            val sorted = renderLatencies.sorted()
                            val idleSorted = idleLatencies.sorted()
                            val p50 = sorted[((sorted.size - 1) * 0.50).toInt()]
                            val p90 = sorted[((sorted.size - 1) * 0.90).toInt()]
                            val p95 = sorted[((sorted.size - 1) * 0.95).toInt()]
                            val idleP95 = idleSorted[((idleSorted.size - 1) * 0.95).toInt()]
                            val overThreshold = renderLatencies.count { it > 33.0 }
                            val auditWidth = System.getenv("HLS_UI_AUDIT_WIDTH")?.toIntOrNull() ?: 1400
                            val auditHeight = System.getenv("HLS_UI_AUDIT_HEIGHT")?.toIntOrNull() ?: 820
                            val report = """{"schema":1,"task_count":${tasks.size},"metric":"continuous_scroll_frame_interval_ms","foreground_required":true,"window_active_at_start":$windowActiveAtStart,"settle_delay_ms":$settleDelayMs,"warmup_jump_count":$warmupJumpCount,"warmup_scroll_count":$warmupScrollCount,"scroll_step_rows":1,"sample_count":${renderLatencies.size},"idle_sample_count":${idleLatencies.size},"window_width":$auditWidth,"window_height":$auditHeight,"idle_frame_p95_ms":${"%.3f".format(java.util.Locale.ROOT, idleP95)},"frame_p50_ms":${"%.3f".format(java.util.Locale.ROOT, p50)},"frame_p90_ms":${"%.3f".format(java.util.Locale.ROOT, p90)},"frame_p95_ms":${"%.3f".format(java.util.Locale.ROOT, p95)},"frame_max_ms":${"%.3f".format(java.util.Locale.ROOT, renderLatencies.maxOrNull() ?: 0.0)},"over_threshold_count":$overThreshold,"threshold_ms":33,"passed":${windowActiveAtStart && p95 <= 33.0}}"""
                            withContext(Dispatchers.IO) {
                                File(frameReportPath).also { it.parentFile?.mkdirs() }.writeText(report, Charsets.UTF_8)
                            }
                        }
                    }
                    Box(
                        Modifier.fillMaxWidth().weight(1f)
                            .onPointerEvent(PointerEventType.Press, PointerEventPass.Initial) { event ->
                                if (event.button == PointerButton.Primary || event.buttons.isPrimaryPressed) {
                                    val position = event.changes.firstOrNull()?.position ?: return@onPointerEvent
                                    dragStart = position
                                    dragCurrent = position
                                    dragBaseSelection = selected
                                    dragAdditive = event.keyboardModifiers.isPointerCtrlPressed
                                    dragSelecting = false
                                    val hitRow = listState.layoutInfo.visibleItemsInfo.any { position.y >= it.offset && position.y < it.offset + it.size }
                                    if (!hitRow && !dragAdditive) {
                                        onSelection(emptySet())
                                        anchorId = null
                                    }
                                }
                            }
                            .onPointerEvent(PointerEventType.Move, PointerEventPass.Initial) { event ->
                                val start = dragStart ?: return@onPointerEvent
                                if (!event.buttons.isPrimaryPressed) return@onPointerEvent
                                val current = event.changes.firstOrNull()?.position ?: return@onPointerEvent
                                dragCurrent = current
                                if (!dragSelecting && (current - start).getDistance() >= 5f) dragSelecting = true
                                if (dragSelecting) {
                                    val top = minOf(start.y, current.y)
                                    val bottom = maxOf(start.y, current.y)
                                    val intersecting = listState.layoutInfo.visibleItemsInfo.filter { item ->
                                        item.offset + item.size >= top && item.offset <= bottom
                                    }
                                    if (intersecting.isNotEmpty()) {
                                        val first = intersecting.minOf { it.index }
                                        val last = intersecting.maxOf { it.index }
                                        onSelection(selectionAfterDrag(tasks.map { it.id }, first, last, dragBaseSelection, dragAdditive))
                                        anchorId = tasks.getOrNull(last)?.id
                                    } else if (!dragAdditive) {
                                        onSelection(emptySet())
                                    }
                                    event.changes.forEach { it.consume() }
                                }
                            }
                            .onPointerEvent(PointerEventType.Release, PointerEventPass.Initial) {
                                if (dragSelecting) suppressRowClickUntilNanos = System.nanoTime() + 300_000_000L
                                dragStart = null
                                dragCurrent = null
                                dragSelecting = false
                            }
                            .drawWithContent {
                                drawContent()
                                val start = dragStart
                                val current = dragCurrent
                                if (dragSelecting && start != null && current != null) {
                                    val topLeft = Offset(minOf(start.x, current.x), minOf(start.y, current.y))
                                    val boxSize = androidx.compose.ui.geometry.Size(kotlin.math.abs(current.x - start.x), kotlin.math.abs(current.y - start.y))
                                    drawRect(selectionBlue.copy(alpha = .12f), topLeft = topLeft, size = boxSize)
                                    drawRect(selectionBlue.copy(alpha = .8f), topLeft = topLeft, size = boxSize, style = Stroke(width = 1f))
                                }
                            },
                    ) {
                        LazyColumn(Modifier.fillMaxSize().background(rail).padding(end = 7.dp), state = listState) { items(tasks, key = { it.id }, contentType = { it.status }) { task ->
                            val contextTargets = if (task.id in selected) tasks.filter { it.id in selected } else listOf(task)
                            TaskRow(
                                task,
                                columns,
                                taskSort == "queue:asc",
                                task.id in selected,
                                { shift, toggle -> if (System.nanoTime() >= suppressRowClickUntilNanos) { focusRequester.requestFocus(); selectTask(task.id, shift, toggle) } },
                                { onDetails(task) },
                                { delta -> onQueueMove(task.id, delta) },
                                { position ->
                                    val actions = batchTaskMenuActions(contextTargets.map { it.source })
                                    contextMenu = TaskContextMenuRequest(task, contextTargets, position)
                                    UiTestState.updateContextMenu(position, contextTargets.mapTo(mutableSetOf()) { it.id }, actions)
                                },
                            ) { action -> onAction(task.id, action) }
                        } }
                        if (tasks.size > 8) VerticalScrollbar(rememberScrollbarAdapter(listState), Modifier.align(Alignment.CenterEnd).fillMaxHeight().width(7.dp))
                    }
                }
            }
        }
        if (tableWidth > maxWidth) HorizontalScrollbar(rememberScrollbarAdapter(horizontalState), Modifier.align(Alignment.BottomCenter).fillMaxWidth().height(9.dp))
        contextMenu?.let { request ->
            Popup(
                popupPositionProvider = ContextMenuPositionProvider(request.position),
                onDismissRequest = { contextMenu = null },
                properties = PopupProperties(focusable = true),
            ) {
                Surface(
                    color = dialogSurface,
                    shape = RoundedCornerShape(8.dp),
                    shadowElevation = 8.dp,
                    border = BorderStroke(1.dp, border),
                    modifier = Modifier.width(220.dp),
                ) {
                    Column(Modifier.padding(vertical = 4.dp)) {
                        TaskMenuEntries(
                            request.task,
                            { contextMenu = null },
                            { onDetails(request.task) },
                            { action -> onBatchAction(request.targets.mapTo(mutableSetOf()) { it.id }, action) },
                            actions = batchTaskMenuActions(request.targets.map { it.source }),
                            includeDetails = request.targets.size == 1,
                        )
                    }
                }
            }
        }
    }
}
@Composable
private fun TaskHeader(value: String, modifier: Modifier, field: String, taskSort: String, onSort: (String) -> Unit) {
    val sortable = field != "actions"
    val active = taskSort.substringBefore(':') == field
    Row(
        modifier.height(36.dp).then(if (sortable) Modifier.clickable { onSort(field) } else Modifier),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(value, color = muted, fontSize = 12.sp, fontWeight = FontWeight.SemiBold, maxLines = 1)
        if (active) {
            Spacer(Modifier.width(3.dp))
            Icon(
                if (taskSort.endsWith(":desc")) Icons.Outlined.ArrowDownward else Icons.Outlined.ArrowUpward,
                if (taskSort.endsWith(":desc")) "降序" else "升序",
                Modifier.size(13.dp),
                tint = blue,
            )
        }
    }
}
@OptIn(ExperimentalComposeUiApi::class, ExperimentalFoundationApi::class)
@Composable private fun TaskRow(task: DownloadTask, columns: ResolvedTaskColumns, queueOrder: Boolean, isSelected: Boolean, select: (Boolean, Boolean) -> Unit, onDetails: () -> Unit, onQueueMove: (Int) -> Unit, onContextMenu: (IntOffset) -> Unit, onAction: (String) -> Unit) {
    var overflowOpen by remember { mutableStateOf(false) }
    var rowWindowOrigin by remember { mutableStateOf(Offset.Zero) }
    var rowSize by remember { mutableStateOf(IntSize.Zero) }
    var clickShift by remember { mutableStateOf(false) }
    var clickCtrl by remember { mutableStateOf(false) }
    val rowInteraction = remember { MutableInteractionSource() }
    val hovered by rowInteraction.collectIsHoveredAsState()
    val rowColor = when {
        isSelected -> selectedSurface
        hovered -> surface2
        else -> rail
    }
    val separatorColor = border
    Row(Modifier.fillMaxWidth().height(52.dp).onGloballyPositioned { rowWindowOrigin = it.positionInWindow(); rowSize = it.size }.background(rowColor).drawBehind { drawLine(separatorColor, androidx.compose.ui.geometry.Offset(0f, size.height - 1f), androidx.compose.ui.geometry.Offset(size.width, size.height - 1f), 1f) }.hoverable(rowInteraction).semantics { selected = isSelected; contentDescription = taskAccessibilityLabel(task) }.onPointerEvent(PointerEventType.Press, PointerEventPass.Initial) {
        clickShift = it.keyboardModifiers.isPointerShiftPressed
        clickCtrl = it.keyboardModifiers.isPointerCtrlPressed
        if (it.button == PointerButton.Secondary || it.buttons.isSecondaryPressed) {
            if (!isSelected) select(false, false)
            onContextMenu(contextMenuWindowPosition(rowWindowOrigin, rowSize, it.changes.firstOrNull()?.position ?: Offset.Zero))
        }
    }.combinedClickable(interactionSource = rowInteraction, indication = null, onDoubleClick = { select(false, false); onDetails() }, onClick = { select(clickShift, clickCtrl) }).padding(horizontal = 15.dp), verticalAlignment = Alignment.CenterVertically) {
        columns.items.forEach { column ->
            key(column.id) {
                when (column.id) {
                    "name" -> Row(Modifier.width(column.width), verticalAlignment = Alignment.CenterVertically) { Checkbox(isSelected, { select(false, true) }, Modifier.size(18.dp), accessibilityLabel = "选择 ${task.filename}"); Spacer(Modifier.width(8.dp)); Icon(categoryIcon(taskCategory(task)), null, tint = muted, modifier = Modifier.size(18.dp)); Spacer(Modifier.width(8.dp)); WorkbenchTooltip("${task.filename}\n${safeResourceLocation(task.source.url)}") { Column(Modifier.weight(1f).padding(end = 8.dp)) { Text(task.filename, color = ink, maxLines = 1, overflow = TextOverflow.Ellipsis, fontSize = 12.sp, fontWeight = FontWeight.SemiBold); Text(listOf(taskProtocolLabel(task.source), taskExtensionLabel(task.source)).filter { it.isNotBlank() }.joinToString(" · "), color = Color(0xFFC76545), fontSize = 10.sp, fontWeight = FontWeight.SemiBold) } } }
                    "progress" -> Column(Modifier.width(column.width).padding(end = 8.dp)) {
                        Row(verticalAlignment = Alignment.CenterVertically) { LinearProgressIndicator(progress = { task.progress }, modifier = Modifier.weight(1f).height(6.dp).clip(RoundedCornerShape(3.dp)), color = if (task.status == "已暂停") Color(0xFFD97706) else blue, trackColor = surface3); Spacer(Modifier.width(8.dp)); Text("${(task.progress * 100).toInt()}%", color = faint, fontSize = 10.sp) }
                        Text(
                            when {
                                task.source.resourceKind.equals("torrent", true) -> "${task.segments} · ${task.source.peerCount} Peer"
                                task.source.activeWorkers > 0 -> "${task.segments} · ${task.source.activeWorkers} 连接"
                                else -> task.segments
                            },
                            color = faint, fontSize = 10.sp, maxLines = 1, overflow = TextOverflow.Ellipsis,
                            modifier = Modifier.padding(top = 3.dp),
                        )
                    }
                    "status" -> Column(Modifier.width(column.width)) { StatusBadge(task.status); Text(if (task.status == "进行中") task.remaining else "", color = muted, fontSize = 10.sp, modifier = Modifier.padding(top = 1.dp)) }
                    "speed" -> Text(if (task.status == "进行中") task.speed else "—", Modifier.width(column.width), color = ink, fontSize = 11.sp)
                    "size" -> Text(task.source.totalBytes?.let(::formatBytes) ?: "—", Modifier.width(column.width), color = ink, fontSize = 11.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    "actions" -> Row(Modifier.width(column.width), verticalAlignment = Alignment.CenterVertically) {
                        if (!columns.compact && queueOrder && task.status == "排队中") {
                            var dragY by remember { mutableFloatStateOf(0f) }
                            Icon(Icons.Outlined.DragHandle, "拖动排序", tint = faint, modifier = Modifier.size(28.dp).padding(5.dp).pointerInput(task.id) { detectDragGestures(onDragStart = { dragY = 0f }, onDragEnd = { val delta = (dragY / 52f).toInt(); if (delta != 0) onQueueMove(delta); dragY = 0f }) { change, amount -> change.consume(); dragY += amount.y } })
                        } else if (!columns.compact) Spacer(Modifier.width(28.dp))
                        Box(Modifier.width(42.dp)) { Box(Modifier.size(34.dp).clip(RoundedCornerShape(6.dp)).clickable { if (!isSelected) select(false, false); overflowOpen = true }, contentAlignment = Alignment.Center) { Icon(Icons.Outlined.MoreVert, "更多操作", tint = muted) }; DropdownMenu(expanded = overflowOpen, onDismissRequest = { overflowOpen = false }, shape = RoundedCornerShape(8.dp), containerColor = dialogSurface, tonalElevation = 0.dp, shadowElevation = 6.dp) { TaskMenuEntries(task, { overflowOpen = false }, onDetails, onAction) } }
                    }
                }
            }
        }
        Spacer(Modifier.weight(1f))
    }
}
internal fun contextMenuWindowPosition(rowOrigin: Offset, rowSize: IntSize, pointer: Offset): IntOffset {
    val isRowLocal = pointer.x in 0f..rowSize.width.toFloat() && pointer.y in 0f..rowSize.height.toFloat()
    val windowPointer = if (isRowLocal) rowOrigin + pointer else pointer
    return IntOffset(windowPointer.x.roundToInt(), windowPointer.y.roundToInt())
}

private class ContextMenuPositionProvider(private val pointer: IntOffset) : PopupPositionProvider {
    override fun calculatePosition(
        anchorBounds: IntRect,
        windowSize: IntSize,
        layoutDirection: LayoutDirection,
        popupContentSize: IntSize,
    ): IntOffset = IntOffset(
        pointer.x.coerceIn(0, (windowSize.width - popupContentSize.width).coerceAtLeast(0)),
        pointer.y.coerceIn(0, (windowSize.height - popupContentSize.height).coerceAtLeast(0)),
    )
}
@Composable private fun TaskMenuEntries(task: DownloadTask, dismiss: () -> Unit, onDetails: () -> Unit, onAction: (String) -> Unit, actions: List<String> = taskMenuActions(task.source), includeDetails: Boolean = true) {
    if (includeDetails) DropdownMenuItem(text = { Text("详情与日志", fontSize = 12.sp, color = ink) }, onClick = { dismiss(); onDetails() })
    actions.forEach { action ->
        DropdownMenuItem(text = { Text(actionLabel(action), fontSize = 12.sp, color = ink) }, onClick = { dismiss(); onAction(action) })
    }
    if (includeDetails && task.status == "排队中") {
        HorizontalDivider(color = border)
        listOf("queue_up", "queue_down", "queue_top", "queue_bottom").forEach { action ->
            DropdownMenuItem(text = { Text(actionLabel(action), fontSize = 12.sp, color = ink) }, onClick = { dismiss(); onAction(action) })
        }
    }
}
private fun categoryLabel(category: TaskCategory) = when (category) { TaskCategory.MEDIA -> "媒体"; TaskCategory.PROGRAM -> "程序"; TaskCategory.ARCHIVE -> "压缩包"; TaskCategory.OTHER -> "其他" }
private fun taskProtocolLabel(task: TaskDto) = when {
    task.resourceKind.equals("hls", true) -> "HLS"
    task.resourceKind.equals("dash", true) -> "DASH"
    task.resourceKind.equals("live", true) -> "直播"
    task.resourceKind.equals("torrent", true) || task.url.startsWith("magnet:", true) -> "BT"
    task.url.startsWith("sftp:", true) -> "SFTP"
    task.url.startsWith("ftp:", true) || task.url.startsWith("ftps:", true) -> "FTP"
    task.url.startsWith("http:", true) || task.url.startsWith("https:", true) -> "HTTP"
    else -> resourceKindLabel(task.resourceKind)
}
internal fun taskAccessibilityLabel(task: DownloadTask): String = listOf(
    task.filename,
    listOf(taskProtocolLabel(task.source), taskExtensionLabel(task.source)).filter(String::isNotBlank).joinToString(" "),
    task.status,
    "进度 ${(task.progress.coerceIn(0f, 1f) * 100).toInt()}%",
    if (task.status == "进行中") "速度 ${task.speed}" else null,
    task.source.totalBytes?.let { "大小 ${formatBytes(it)}" },
).filterNotNull().filter(String::isNotBlank).joinToString("，")
internal fun taskExtensionLabel(task: TaskDto): String {
    val filename = task.filename.substringBefore('?').substringAfterLast('/').substringAfterLast('\\')
    val extension = filename.substringAfterLast('.', "").takeIf { filename.contains('.') && it.length in 1..8 }
    return extension?.let { ".${it.lowercase()}" } ?: when (task.resourceKind.lowercase()) {
        "hls", "live" -> ".m3u8"
        "dash" -> ".mpd"
        "torrent" -> ".torrent"
        else -> ""
    }
}
private fun formatBytes(bytes: Long) = when { bytes >= 1024L * 1024L * 1024L -> "%.2f GB".format(bytes / 1024.0 / 1024.0 / 1024.0); bytes >= 1024L * 1024L -> "%.2f MB".format(bytes / 1024.0 / 1024.0); bytes >= 1024L -> "%.1f KB".format(bytes / 1024.0); else -> "$bytes B" }
private fun actionLabel(action: String) = mapOf("details" to "详情与日志", "start" to "开始", "pause" to "暂停", "resume" to "继续", "retry" to "重试", "cancel" to "取消", "open" to "打开文件", "open_folder" to "打开所在位置", "launch" to "运行文件", "copy_file" to "复制文件", "drag_file" to "拖出文件", "delete" to "删除任务", "delete_files" to "删除任务和文件", "play" to "播放", "cast" to "投屏", "push_tvbox" to "TVBox 推送", "move_queue" to "移动到队列", "queue_up" to "上移", "queue_down" to "下移", "queue_top" to "置顶", "queue_bottom" to "置底")[action] ?: action
@Composable private fun StatusBadge(status: String, modifier: Modifier = Modifier) {
    val color = when(status) { "已完成" -> Color(0xFF078C46); "失败" -> Color(0xFFDC2626); "已暂停" -> Color(0xFFD97706); else -> blue }
    val live = status == "进行中" || status == "排队中"
    Surface(modifier, color = color.copy(alpha = .10f), shape = RoundedCornerShape(12.dp)) {
        Row(Modifier.padding(horizontal = 8.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.size(6.dp).clip(RoundedCornerShape(50)).background(color.copy(alpha = if (live) .95f else .7f)))
            Spacer(Modifier.width(5.dp))
            Text(status, color = color, fontSize = 12.sp, fontWeight = FontWeight.SemiBold)
        }
    }
}
private fun taskCount(tasks: List<DownloadTask>, filter: TaskFilter) = when (filter) { TaskFilter.ALL -> tasks.size; else -> tasks.count { it.status == filter.label } }
@Composable private fun ConnectionStatus(tasks: List<DownloadTask>, engine: String, extension: String, modifier: Modifier = Modifier) { val active = tasks.count { it.status == "进行中" }; val bytes = tasks.filter { it.status == "进行中" }.sumOf { it.speedBytes }; Row(modifier.height(28.dp).fillMaxWidth().background(rail).border(BorderStroke(1.dp, border)).padding(horizontal = 12.dp), verticalAlignment = Alignment.CenterVertically) { Text("活动任务 $active", color = muted, fontSize = 11.sp); Spacer(Modifier.width(14.dp)); Text("队列 ${tasks.count { it.status == "排队中" }}", color = muted, fontSize = 11.sp); Spacer(Modifier.width(14.dp)); Text("总速度 ${formatRate(bytes)}", color = blue, fontSize = 11.sp, fontWeight = FontWeight.SemiBold); Spacer(Modifier.weight(1f)); Text(engine, color = if (engine == Product.engineConnected) Color(0xFF16A34A) else Color(0xFFD97706), fontSize = 11.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis); Spacer(Modifier.width(14.dp)); Text(extension, color = faint, fontSize = 11.sp, maxLines = 1, overflow = TextOverflow.Ellipsis) } }

internal data class DialogBounds(val width: Dp, val maxHeight: Dp)

internal fun dialogBounds(requestedWidth: Dp, viewportWidth: Dp, viewportHeight: Dp): DialogBounds = DialogBounds(
    width = requestedWidth.coerceAtMost((viewportWidth - 32.dp).coerceAtLeast(280.dp)),
    maxHeight = (viewportHeight - 32.dp).coerceAtLeast(320.dp).coerceAtMost(680.dp),
)
@Composable
internal fun WorkbenchDialog(
    onDismiss: () -> Unit,
    title: String,
    description: String,
    width: Dp = 520.dp,
    dismissible: Boolean = true,
    scrollable: Boolean = true,
    content: @Composable ColumnScope.() -> Unit,
    actions: @Composable RowScope.() -> Unit,
) {
    Popup(
        alignment = Alignment.Center,
        onDismissRequest = { if (dismissible) onDismiss() },
        properties = PopupProperties(focusable = true),
    ) {
        BoxWithConstraints(
            Modifier
                .fillMaxSize()
                .background(Color.Black.copy(alpha = .34f)),
            contentAlignment = Alignment.Center,
        ) {
            if (dismissible) {
                Box(
                    Modifier
                        .matchParentSize()
                        .clickable(indication = null, interactionSource = remember { MutableInteractionSource() }, onClick = onDismiss)
                        .clearAndSetSemantics { },
                )
            }
            val bounds = dialogBounds(width, maxWidth, maxHeight)
            AnimatedVisibility(
                visible = true,
                enter = fadeIn() + scaleIn(initialScale = .97f),
                exit = fadeOut() + scaleOut(targetScale = .97f),
            ) {
            Surface(
                modifier = Modifier
                    .width(bounds.width)
                    .heightIn(max = bounds.maxHeight)
                    .semantics { paneTitle = title },
                shape = RoundedCornerShape(10.dp),
                color = dialogSurface,
                shadowElevation = 16.dp,
                border = BorderStroke(1.dp, border),
            ) {
                Column(Modifier.fillMaxWidth().padding(18.dp)) {
                    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.Top) {
                        Column(Modifier.weight(1f)) {
                            Text(title, color = ink, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
                            Text(description, color = muted, fontSize = 11.sp, modifier = Modifier.padding(top = 4.dp))
                        }
                        if (dismissible) IconButton(onClick = onDismiss, modifier = Modifier.size(32.dp)) {
                            Icon(Icons.Outlined.Close, "关闭", tint = muted, modifier = Modifier.size(18.dp))
                        }
                    }
                    Spacer(Modifier.height(13.dp))
                    HorizontalDivider(color = border)
                    Spacer(Modifier.height(13.dp))
                    Column(
                        Modifier
                            .fillMaxWidth()
                            .weight(1f, fill = false)
                            .heightIn(max = 420.dp)
                            .then(if (scrollable) Modifier.verticalScroll(rememberScrollState()) else Modifier),
                        content = content,
                    )
                    Spacer(Modifier.height(14.dp))
                    HorizontalDivider(color = border)
                    Spacer(Modifier.height(10.dp))
                    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End, content = actions)
                }
            }
            }
        }
    }
}
@Composable internal fun DialogPrimary(label: String, enabled: Boolean = true, onClick: () -> Unit) = Button(onClick = onClick, enabled = enabled, shape = RoundedCornerShape(7.dp), contentPadding = PaddingValues(horizontal = 15.dp), colors = ButtonDefaults.buttonColors(containerColor = blue, contentColor = Color.White)) { Text(label, fontSize = 12.sp, fontWeight = FontWeight.SemiBold) }
@Composable internal fun DialogSecondary(label: String, onClick: () -> Unit) = TextButton(onClick = onClick, contentPadding = PaddingValues(horizontal = 12.dp)) { Text(label, fontSize = 12.sp, color = muted) }
@Composable internal fun DialogLabel(value: String) = Text(value, color = muted, fontSize = 11.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(bottom = 5.dp))

internal fun queueProfilesValid(profiles: List<QueueProfileDto>): Boolean {
    if (profiles.isEmpty() || profiles.size > 32 || profiles.none { it.id == "default" }) return false
    if (profiles.map { it.id }.distinct().size != profiles.size) return false
    if (profiles.map { it.name.trim().lowercase() }.distinct().size != profiles.size) return false
    val clock = Regex("^(?:[01]\\d|2[0-3]):[0-5]\\d$")
    return profiles.all {
        val days = it.activeDays.split(',').mapNotNull(String::toIntOrNull)
        it.id.matches(Regex("^[a-z0-9][a-z0-9_-]{0,31}$")) &&
            it.name.isNotBlank() && it.name.length <= 40 &&
            it.priority in -100..100 && it.maxActive in 1..64 &&
            it.speedLimitKib in 0..1_048_576 &&
            clock.matches(it.startTime) && clock.matches(it.stopTime) &&
            days.isNotEmpty() && days.size == it.activeDays.split(',').size && days.distinct().size == days.size && days.all { day -> day in 1..7 } &&
            it.completionAction in setOf("none", "sleep", "hibernate", "shutdown")
    }
}

@Composable
private fun QueueAssignDialog(
    count: Int,
    profiles: List<QueueProfileDto>,
    onDismiss: () -> Unit,
    onAssign: (String) -> Unit,
) {
    val available = profiles.filter { it.enabled }
    var selectedId by remember(profiles) { mutableStateOf(available.firstOrNull()?.id.orEmpty()) }
    WorkbenchDialog(
        onDismiss,
        "移动到队列",
        "为已选择的 $count 个任务指定调度队列",
        500.dp,
        content = {
            if (available.isEmpty()) {
                Surface(Modifier.fillMaxWidth(), color = surface2, shape = RoundedCornerShape(8.dp)) {
                    Text("当前没有启用的下载队列，请先在队列管理中启用一个队列。", color = muted, fontSize = 12.sp, modifier = Modifier.padding(14.dp))
                }
            } else {
                available.forEach { profile ->
                    val selected = selectedId == profile.id
                    Row(
                        Modifier.fillMaxWidth().clip(RoundedCornerShape(7.dp))
                            .background(if (selected) selectedSurface else Color.Transparent)
                            .clickable { selectedId = profile.id }
                            .padding(horizontal = 11.dp, vertical = 10.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        RadioButton(selected, { selectedId = profile.id }, accessibilityLabel = "选择队列 ${profile.name}")
                        Spacer(Modifier.width(8.dp))
                        Column(Modifier.weight(1f)) {
                            Text(profile.name, color = ink, fontSize = 12.sp, fontWeight = FontWeight.SemiBold)
                            val speed = if (profile.speedLimitKib > 0) " · ${profile.speedLimitKib} KiB/s" else " · 不限速"
                            Text("优先级 ${profile.priority} · 最多 ${profile.maxActive} 个活动任务$speed", color = faint, fontSize = 10.sp, modifier = Modifier.padding(top = 2.dp))
                        }
                    }
                }
            }
        },
        actions = {
            DialogSecondary("取消", onDismiss)
            DialogPrimary("移动", selectedId.isNotBlank()) { onAssign(selectedId) }
        },
    )
}

@Composable
private fun QueueManagerDialog(
    current: List<QueueProfileDto>,
    onDismiss: () -> Unit,
    onSave: (List<QueueProfileDto>) -> Unit,
) {
    var profiles by remember(current) { mutableStateOf(current.ifEmpty { listOf(QueueProfileDto()) }) }
    var selectedId by remember(current) { mutableStateOf(profiles.first().id) }
    val selected = profiles.firstOrNull { it.id == selectedId } ?: profiles.first()
    val valid = queueProfilesValid(profiles)
    fun update(transform: (QueueProfileDto) -> QueueProfileDto) {
        profiles = profiles.map { if (it.id == selected.id) transform(it) else it }
    }
    fun move(delta: Int) {
        val from = profiles.indexOfFirst { it.id == selected.id }
        val to = (from + delta).coerceIn(0, profiles.lastIndex)
        if (from == to) return
        profiles = profiles.toMutableList().also { list -> list.add(to, list.removeAt(from)) }
    }
    WorkbenchDialog(
        onDismiss,
        "下载队列",
        "不同队列按优先级、并发、限速、时间表和完成动作独立运行",
        780.dp,
        scrollable = false,
        content = {
            Row(Modifier.fillMaxWidth().heightIn(min = 360.dp, max = 420.dp)) {
                Column(Modifier.width(218.dp).fillMaxHeight().padding(end = 14.dp)) {
                    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                        Text("队列", color = muted, fontSize = 11.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
                        IconButton(onClick = {
                            val id = "queue-${System.currentTimeMillis().toString(36)}"
                            profiles = profiles + QueueProfileDto(id = id, name = "新队列", priority = (profiles.maxOfOrNull { it.priority } ?: 0) + 1)
                            selectedId = id
                        }, modifier = Modifier.size(30.dp)) { Icon(Icons.Outlined.Add, "新建队列", tint = blue, modifier = Modifier.size(18.dp)) }
                    }
                    Spacer(Modifier.height(5.dp))
                    Column(Modifier.weight(1f).verticalScroll(rememberScrollState())) {
                        profiles.forEach { profile ->
                            val active = profile.id == selected.id
                            Row(
                                Modifier.fillMaxWidth().clip(RoundedCornerShape(7.dp))
                                    .background(if (active) selectedSurface else Color.Transparent)
                                    .clickable { selectedId = profile.id }
                                    .padding(horizontal = 10.dp, vertical = 9.dp),
                                verticalAlignment = Alignment.CenterVertically,
                            ) {
                                Icon(if (profile.enabled) Icons.Outlined.PlayCircleOutline else Icons.Outlined.PauseCircleOutline, null, tint = if (profile.enabled) blue else faint, modifier = Modifier.size(17.dp))
                                Spacer(Modifier.width(8.dp))
                                Column(Modifier.weight(1f)) {
                                    Text(profile.name, color = ink, fontSize = 12.sp, fontWeight = if (active) FontWeight.SemiBold else FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                    val speed = if (profile.speedLimitKib > 0) " · ${profile.speedLimitKib} KiB/s" else " · 不限速"
                                    Text("优先级 ${profile.priority} · 并发 ${profile.maxActive}$speed", color = faint, fontSize = 9.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                }
                            }
                        }
                    }
                    HorizontalDivider(color = border)
                    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                        IconButton(onClick = { move(-1) }, enabled = profiles.indexOf(selected) > 0) { Icon(Icons.Outlined.KeyboardArrowUp, "上移队列", tint = muted) }
                        IconButton(onClick = { move(1) }, enabled = profiles.indexOf(selected) < profiles.lastIndex) { Icon(Icons.Outlined.KeyboardArrowDown, "下移队列", tint = muted) }
                        IconButton(onClick = {
                            if (selected.id != "default" && profiles.size > 1) {
                                profiles = profiles.filterNot { it.id == selected.id }
                                selectedId = profiles.first().id
                            }
                        }, enabled = selected.id != "default" && profiles.size > 1) { Icon(Icons.Outlined.DeleteOutline, "删除队列", tint = if (selected.id == "default") faint else Color(0xFFDC2626)) }
                    }
                }
                Box(Modifier.width(1.dp).fillMaxHeight().background(border))
                Column(Modifier.weight(1f).fillMaxHeight().padding(start = 16.dp).verticalScroll(rememberScrollState())) {
                    DialogLabel("队列名称")
                    OutlinedTextField(selected.name, { value -> update { it.copy(name = value.take(40)) } }, Modifier.fillMaxWidth(), singleLine = true, isError = selected.name.isBlank(), shape = RoundedCornerShape(7.dp))
                    Spacer(Modifier.height(8.dp))
                    SettingRow("启用队列", "停用后保留任务，但不会开始新的下载", selected.enabled) { value -> update { it.copy(enabled = value) } }
                    Row(Modifier.fillMaxWidth()) {
                        Column(Modifier.weight(1f)) {
                            DialogLabel("优先级 (-100 至 100)")
                            OutlinedTextField(selected.priority.toString(), { value -> value.toIntOrNull()?.let { number -> update { it.copy(priority = number.coerceIn(-100, 100)) } } }, Modifier.fillMaxWidth(), singleLine = true, shape = RoundedCornerShape(7.dp))
                        }
                        Spacer(Modifier.width(10.dp))
                        Column(Modifier.weight(1f)) {
                            DialogLabel("最大活动任务")
                            OutlinedTextField(selected.maxActive.toString(), { value -> value.toIntOrNull()?.let { number -> update { it.copy(maxActive = number.coerceIn(1, 64)) } } }, Modifier.fillMaxWidth(), singleLine = true, shape = RoundedCornerShape(7.dp))
                        }
                    }
                    Spacer(Modifier.height(8.dp))
                    DialogLabel("队列总限速（KiB/s）")
                    OutlinedTextField(
                        selected.speedLimitKib.toString(),
                        { value -> value.toLongOrNull()?.let { number -> update { it.copy(speedLimitKib = number.coerceIn(0, 1_048_576)) } } },
                        Modifier.fillMaxWidth(),
                        singleLine = true,
                        supportingText = { Text("0 表示不限速；同一队列的活动任务共享该速度", color = faint, fontSize = 9.sp) },
                        shape = RoundedCornerShape(7.dp),
                    )
                    SettingRow("启用时间表", "只在指定星期和时段启动新任务", selected.scheduleEnabled) { value -> update { it.copy(scheduleEnabled = value) } }
                    if (selected.scheduleEnabled) {
                        Row(Modifier.fillMaxWidth()) {
                            Column(Modifier.weight(1f)) { DialogLabel("开始时间"); OutlinedTextField(selected.startTime, { value -> update { it.copy(startTime = value.take(5)) } }, Modifier.fillMaxWidth(), singleLine = true, shape = RoundedCornerShape(7.dp)) }
                            Spacer(Modifier.width(10.dp))
                            Column(Modifier.weight(1f)) { DialogLabel("停止时间"); OutlinedTextField(selected.stopTime, { value -> update { it.copy(stopTime = value.take(5)) } }, Modifier.fillMaxWidth(), singleLine = true, shape = RoundedCornerShape(7.dp)) }
                        }
                        Spacer(Modifier.height(8.dp)); DialogLabel("生效星期")
                        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                            listOf("一", "二", "三", "四", "五", "六", "日").forEachIndexed { index, label ->
                                val day = index + 1
                                val days = selected.activeDays.split(',').mapNotNull(String::toIntOrNull)
                                val active = day in days
                                TextButton(onClick = { update { profile -> profile.copy(activeDays = (if (active) days - day else days + day).distinct().sorted().joinToString(",")) } }, modifier = Modifier.weight(1f).clip(RoundedCornerShape(6.dp)).background(if (active) selectedSurface else surface2), contentPadding = PaddingValues(0.dp)) { Text(label, color = if (active) blue else muted, fontSize = 11.sp) }
                            }
                        }
                    }
                    Spacer(Modifier.height(8.dp))
                    DialogLabel("队列全部完成后")
                    Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(7.dp)).background(surface2).padding(3.dp)) {
                        listOf("none" to "无", "sleep" to "睡眠", "hibernate" to "休眠", "shutdown" to "关机").forEach { (value, label) ->
                            val active = selected.completionAction == value
                            TextButton(
                                onClick = { update { it.copy(completionAction = value) } },
                                modifier = Modifier.weight(1f).clip(RoundedCornerShape(5.dp)).background(if (active) selectedSurface else Color.Transparent),
                                contentPadding = PaddingValues(horizontal = 4.dp),
                            ) { Text(label, color = if (active) blue else muted, fontSize = 10.sp) }
                        }
                    }
                    Text("只有该队列中的任务全部成功完成后才会触发，执行前可取消。", color = faint, fontSize = 9.sp, modifier = Modifier.padding(top = 4.dp))
                    if (!valid) Text("请检查队列名称、时间范围和重复名称。", color = Color(0xFFDC2626), fontSize = 10.sp, modifier = Modifier.padding(top = 8.dp))
                }
            }
        },
        actions = {
            DialogSecondary("取消", onDismiss)
            DialogPrimary("保存队列", valid) { onSave(profiles) }
        },
    )
}

@Composable
private fun NewTaskDialog(
    onDismiss: () -> Unit,
    initialUrl: String,
    defaults: EngineSettingsDto,
    onProbe: (TaskDraft) -> Unit,
    onCreate: (TaskDraft) -> Unit,
) {
    var tab by remember { mutableStateOf("基本") }
    var url by remember(initialUrl) { mutableStateOf(initialUrl) }
    var filename by remember(initialUrl) { mutableStateOf("") }
    var directory by remember(defaults) { mutableStateOf(defaults.downloadDirectory) }
    var concurrency by remember(defaults) { mutableStateOf(defaults.defaultConcurrency.toString()) }
    var speed by remember(defaults) { mutableStateOf(defaults.speedLimitKib.toString()) }
    var checksum by remember { mutableStateOf("") }
    var proxy by remember(defaults) { mutableStateOf(defaults.proxyUrl) }
    var mirrors by remember { mutableStateOf("") }
    var referer by remember(defaults) { mutableStateOf(defaults.defaultReferer) }
    var origin by remember(defaults) { mutableStateOf(defaults.defaultOrigin) }
    var cookie by remember { mutableStateOf("") }
    var userAgent by remember(defaults) { mutableStateOf(defaults.defaultUserAgent) }
    var requestHeaders by remember { mutableStateOf("") }
    var requestMethod by remember { mutableStateOf("GET") }
    var startAt by remember { mutableStateOf("") }
    var stopAt by remember { mutableStateOf("") }
    var completionAction by remember(defaults) { mutableStateOf(defaults.completionPowerAction) }
    var allowDuplicate by remember(defaults) { mutableStateOf(defaults.allowDuplicate) }
    val urlFocus = remember { FocusRequester() }
    LaunchedEffect(Unit) { delay(120); urlFocus.requestFocus() }
    val curlMode = EnginePipeClient.isCurlCommand(url)
    val normalized = if (curlMode) null else runCatching { EnginePipeClient.normalizeDownloadUrl(url) }.getOrNull()
    val parsedHeaders = runCatching { parseRequestHeaderLines(requestHeaders) }
    val validInput = curlMode || normalized != null
    val buildDraft = {
        TaskDraft(
            url = normalized.orEmpty(),
            filename = filename.trim(),
            downloadDirectory = directory.trim(),
            concurrency = concurrency.toLongOrNull()?.coerceIn(0, 128) ?: 0,
            speedLimitKib = speed.toLongOrNull()?.coerceAtLeast(0) ?: 0,
            checksum = checksum.trim(),
            proxy = proxy.trim(),
            mirrors = mirrors.lineSequence().map(String::trim).filter(String::isNotBlank).distinct().toList(),
            referer = referer.trim(),
            origin = origin.trim(),
            cookie = cookie.trim(),
            userAgent = userAgent.trim(),
            requestHeaders = parsedHeaders.getOrDefault(emptyMap()),
            requestMethod = requestMethod,
            curlCommand = if (curlMode) url.trim() else "",
            allowDuplicate = allowDuplicate,
            scheduledStartAt = startAt.trim(),
            scheduledStopAt = stopAt.trim(),
            completionAction = completionAction,
        )
    }
    WorkbenchDialog(onDismiss, "新建下载", "创建文件、媒体、远程协议或 BT 下载任务", 760.dp, content = {
        Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(7.dp)).background(surface2).padding(3.dp)) { listOf("基本", "连接", "请求", "计划").forEach { item -> TextButton(onClick = { tab = item }, Modifier.weight(1f).clip(RoundedCornerShape(5.dp)).background(if (tab == item) rail else Color.Transparent)) { Text(item, color = if (tab == item) blue else muted, fontSize = 11.sp, fontWeight = if (tab == item) FontWeight.SemiBold else FontWeight.Normal) } } }
        Spacer(Modifier.height(14.dp))
        when (tab) {
            "基本" -> {
                DialogLabel("下载链接或 cURL 命令")
                OutlinedTextField(
                    url,
                    { url = it },
                    Modifier.fillMaxWidth(),
                    focusRequester = urlFocus,
                    placeholder = { Text("粘贴链接、磁力链接或浏览器“复制为 cURL”") },
                    singleLine = !curlMode,
                    minLines = if (curlMode) 3 else 1,
                    maxLines = if (curlMode) 5 else 1,
                    isError = url.isNotBlank() && !validInput,
                    shape = RoundedCornerShape(7.dp),
                    supportingText = {
                        Text(
                            when {
                                url.isNotBlank() && !validInput -> "链接格式无效或 cURL 命令不完整"
                                curlMode -> "已识别 cURL；请求头、Cookie 与 POST 请求体由下载引擎安全导入"
                                normalized != null -> "类型：${EnginePipeClient.recognizeResourceKind(normalized)}"
                                else -> "等待输入"
                            },
                            fontSize = 10.sp,
                        )
                    },
                )
                Spacer(Modifier.height(8.dp)); DialogLabel("文件名（留空自动识别）"); OutlinedTextField(filename, { filename = it }, Modifier.fillMaxWidth(), singleLine = true, shape = RoundedCornerShape(7.dp)); Spacer(Modifier.height(8.dp)); DialogLabel("保存到"); Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) { OutlinedTextField(directory, { directory = it }, Modifier.weight(1f), singleLine = true, shape = RoundedCornerShape(7.dp)); Spacer(Modifier.width(8.dp)); DialogSecondary("选择目录") { chooseDirectory(directory, "选择下载保存目录")?.let { directory = it } } }; SettingRow("允许重复任务", "同一资源已存在时仍创建新任务", allowDuplicate) { allowDuplicate = it }
            }
            "连接" -> {
                DialogLabel("并发连接数（0 使用默认值）"); OutlinedTextField(concurrency, { concurrency = it.filter(Char::isDigit) }, Modifier.fillMaxWidth(), singleLine = true, shape = RoundedCornerShape(7.dp)); Spacer(Modifier.height(9.dp)); DialogLabel("任务限速 KiB/s（0 不限制）"); OutlinedTextField(speed, { speed = it.filter(Char::isDigit) }, Modifier.fillMaxWidth(), singleLine = true, shape = RoundedCornerShape(7.dp)); Spacer(Modifier.height(9.dp)); DialogLabel("代理地址（可选）"); OutlinedTextField(proxy, { proxy = it }, Modifier.fillMaxWidth(), singleLine = true, shape = RoundedCornerShape(7.dp)); Spacer(Modifier.height(9.dp)); DialogLabel("校验和（算法:值，可选）"); OutlinedTextField(checksum, { checksum = it }, Modifier.fillMaxWidth(), singleLine = true, shape = RoundedCornerShape(7.dp), placeholder = { Text("sha256:...") }); Spacer(Modifier.height(9.dp)); DialogLabel("备用下载地址（每行一个）"); OutlinedTextField(mirrors, { mirrors = it }, Modifier.fillMaxWidth(), minLines = 3, maxLines = 4, shape = RoundedCornerShape(7.dp), placeholder = { Text("https://mirror.example.com/file.bin") }); Text("仅普通 HTTP(S) 文件使用；媒体清单、BT 与远程协议会忽略。", color = faint, fontSize = 10.sp, modifier = Modifier.padding(top = 5.dp))
            }
            "请求" -> {
                if (curlMode) {
                    Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(7.dp)).background(selectedSurface).padding(11.dp), verticalAlignment = Alignment.CenterVertically) {
                        Icon(Icons.Outlined.Security, null, tint = blue, modifier = Modifier.size(18.dp)); Spacer(Modifier.width(9.dp)); Text("cURL 中的请求上下文由下载引擎解析并加密保存，下方字段作为显式覆盖值。", color = muted, fontSize = 10.sp)
                    }
                    Spacer(Modifier.height(10.dp))
                }
                DialogLabel("请求方式")
                Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(7.dp)).background(surface2).padding(3.dp)) {
                    listOf("GET", "POST", "HEAD").forEach { value -> TextButton(onClick = { requestMethod = value }, Modifier.weight(1f).clip(RoundedCornerShape(5.dp)).background(if (requestMethod == value) selectedSurface else Color.Transparent)) { Text(value, color = if (requestMethod == value) blue else muted, fontSize = 11.sp) } }
                }
                Spacer(Modifier.height(9.dp))
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    Column(Modifier.weight(1f)) {
                        DialogLabel("Referer")
                        OutlinedTextField(referer, { referer = it }, Modifier.fillMaxWidth(), singleLine = true, shape = RoundedCornerShape(7.dp))
                    }
                    Column(Modifier.weight(1f)) {
                        DialogLabel("Origin")
                        OutlinedTextField(origin, { origin = it }, Modifier.fillMaxWidth(), singleLine = true, shape = RoundedCornerShape(7.dp))
                    }
                }
                Spacer(Modifier.height(9.dp))
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    Column(Modifier.weight(1f)) {
                        DialogLabel("User-Agent")
                        OutlinedTextField(userAgent, { userAgent = it }, Modifier.fillMaxWidth(), singleLine = true, shape = RoundedCornerShape(7.dp))
                    }
                    Column(Modifier.weight(1f)) {
                        DialogLabel("Cookie")
                        OutlinedTextField(cookie, { cookie = it }, Modifier.fillMaxWidth(), singleLine = true, visualTransformation = PasswordVisualTransformation(), shape = RoundedCornerShape(7.dp), placeholder = { Text("下载引擎加密保存") })
                    }
                }
                Spacer(Modifier.height(9.dp)); DialogLabel("其他请求头（每行“名称: 值”）"); OutlinedTextField(requestHeaders, { requestHeaders = it }, Modifier.fillMaxWidth(), minLines = 3, maxLines = 3, isError = parsedHeaders.isFailure, shape = RoundedCornerShape(7.dp), placeholder = { Text("Authorization: Bearer ...\nX-Playback-Token: ...") }); Text(parsedHeaders.exceptionOrNull()?.message ?: "敏感请求头只保存在下载引擎的加密凭据中。", color = if (parsedHeaders.isFailure) Color(0xFFDC2626) else faint, fontSize = 10.sp, modifier = Modifier.padding(top = 5.dp))
            }
            else -> {
                DialogLabel("计划开始（ISO 时间或留空）"); OutlinedTextField(startAt, { startAt = it }, Modifier.fillMaxWidth(), singleLine = true, shape = RoundedCornerShape(7.dp)); Spacer(Modifier.height(9.dp)); DialogLabel("计划停止（ISO 时间或留空）"); OutlinedTextField(stopAt, { stopAt = it }, Modifier.fillMaxWidth(), singleLine = true, shape = RoundedCornerShape(7.dp)); Spacer(Modifier.height(9.dp)); DialogLabel("完成后动作"); Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(7.dp)).background(surface2).padding(3.dp)) { listOf("none" to "无", "sleep" to "睡眠", "hibernate" to "休眠", "shutdown" to "关机").forEach { (value, label) -> TextButton(onClick = { completionAction = value }, Modifier.weight(1f).clip(RoundedCornerShape(5.dp)).background(if (completionAction == value) selectedSurface else Color.Transparent), contentPadding = PaddingValues(horizontal = 4.dp)) { Text(label, color = if (completionAction == value) blue else muted, fontSize = 10.sp) } } }
            }
        }
    }, actions = {
        DialogSecondary("取消", onDismiss)
        TextButton(
            onClick = { if (normalized != null && parsedHeaders.isSuccess) onProbe(buildDraft()) },
            enabled = normalized != null && parsedHeaders.isSuccess,
            contentPadding = PaddingValues(horizontal = 12.dp),
        ) { Text("分析资源", fontSize = 12.sp, color = if (normalized != null && parsedHeaders.isSuccess) muted else faint) }
        DialogPrimary("创建下载", validInput && parsedHeaders.isSuccess) {
            onCreate(buildDraft())
        }
    })
}

internal enum class ResourceProbeTarget { Url, Torrent }

internal fun taskProbeTarget(draft: TaskDraft): ResourceProbeTarget =
    if (draft.kind.equals("torrent", ignoreCase = true)) ResourceProbeTarget.Torrent else ResourceProbeTarget.Url

internal fun parseRequestHeaderLines(value: String): Map<String, String> {
    val headers = linkedMapOf<String, String>()
    value.lineSequence().map(String::trim).filter(String::isNotBlank).forEachIndexed { index, line ->
        val split = line.indexOf(':')
        require(split > 0) { "第 ${index + 1} 行缺少冒号" }
        val name = line.substring(0, split).trim()
        val headerValue = line.substring(split + 1).trim()
        require(name.matches(Regex("^[!#$%&'*+.^_`|~0-9A-Za-z-]{1,128}$"))) { "第 ${index + 1} 行的请求头名称无效" }
        require(headerValue.none(Char::isISOControl)) { "第 ${index + 1} 行包含控制字符" }
        require(!name.equals("Cookie", true)) { "Cookie 请填写在单独字段" }
        require(name.lowercase() !in setOf("host", "content-length", "connection", "range", "transfer-encoding")) { "请求头 $name 由下载引擎管理" }
        headers[name] = headerValue
    }
    return headers
}
@Composable private fun BatchAddDialog(onDismiss: () -> Unit, onCreate: (List<String>) -> Unit) { var urls by remember { mutableStateOf("") }; val inputFocus = remember { FocusRequester() }; LaunchedEffect(Unit) { delay(120); inputFocus.requestFocus() }; val entries = urls.lineSequence().map(String::trim).filter(String::isNotBlank).toList(); val valid = entries.filter { runCatching { EnginePipeClient.normalizeDownloadUrl(it) }.isSuccess }; WorkbenchDialog(onDismiss, "批量添加", "每行一个下载链接，可一次创建多个任务", 620.dp, content = { DialogLabel("链接列表"); OutlinedTextField(urls, { urls = it }, Modifier.fillMaxWidth().heightIn(min = 180.dp), focusRequester = inputFocus, placeholder = { Text("https://example.com/file.zip") }, minLines = 7, maxLines = 10, shape = RoundedCornerShape(7.dp)); Text("有效 ${valid.size} / 输入 ${entries.size} 条链接", color = if (entries.isNotEmpty() && valid.size != entries.size) Color(0xFFB54708) else faint, fontSize = 11.sp, modifier = Modifier.padding(top = 7.dp)) }, actions = { DialogSecondary("取消", onDismiss); DialogPrimary("创建 ${valid.size} 个任务", valid.isNotEmpty()) { onCreate(valid) } }) }
@Composable
private fun HarvestDialog(
    onDismiss: () -> Unit,
    defaultReferer: String,
    onHarvest: (String, String) -> Unit,
) {
    var url by remember { mutableStateOf("") }
    var referer by remember(defaultReferer) { mutableStateOf(defaultReferer) }
    val inputFocus = remember { FocusRequester() }
    val validUrl = runCatching { EnginePipeClient.normalizeHttpUrl(url) }.isSuccess
    val validReferer = referer.isBlank() || runCatching { EnginePipeClient.normalizeHttpUrl(referer) }.isSuccess
    LaunchedEffect(Unit) { delay(120); inputFocus.requestFocus() }
    WorkbenchDialog(
        onDismiss,
        "页面抓取",
        "只读取当前页面，提取静态文件、媒体清单、FTP 和磁力链接",
        620.dp,
        content = {
            DialogLabel("网页地址")
            OutlinedTextField(
                url,
                { url = it },
                Modifier.fillMaxWidth(),
                focusRequester = inputFocus,
                placeholder = { Text("https://example.com/files/") },
                singleLine = true,
                shape = RoundedCornerShape(7.dp),
                isError = url.isNotBlank() && !validUrl,
            )
            Text(
                "不会执行网页脚本，也不会继续打开子页面；单次最多返回 100 个资源。",
                color = faint,
                fontSize = 11.sp,
                modifier = Modifier.padding(top = 7.dp),
            )
            Spacer(Modifier.height(13.dp))
            DialogLabel("Referer（可选）")
            OutlinedTextField(
                referer,
                { referer = it },
                Modifier.fillMaxWidth(),
                placeholder = { Text("留空时使用上面的网页地址") },
                singleLine = true,
                shape = RoundedCornerShape(7.dp),
                isError = !validReferer,
            )
            Text("用于需要来源页校验的文件和媒体地址。", color = faint, fontSize = 11.sp, modifier = Modifier.padding(top = 7.dp))
        },
        actions = {
            DialogSecondary("取消", onDismiss)
            DialogPrimary("抓取本页链接", validUrl && validReferer) { onHarvest(url.trim(), referer.trim()) }
        },
    )
}
@Composable private fun SettingsDialog(onDismiss: () -> Unit, current: EngineSettingsDto, onSave: (EngineSettingsDto) -> Unit) {
    var tab by remember { mutableStateOf("通用") }
    var dark by remember(current) { mutableStateOf(current.darkMode) }
    var downloadDirectory by remember(current) { mutableStateOf(current.downloadDirectory) }
    var concurrency by remember(current) { mutableStateOf(current.defaultConcurrency.toString()) }
    var limit by remember(current) { mutableStateOf(current.speedLimitKib.toString()) }
    var queueMax by remember(current) { mutableStateOf(current.queueMax.toString()) }
    var retryMax by remember(current) { mutableStateOf(current.autoRetryMax.toString()) }
    var clipboardWatch by remember(current) { mutableStateOf(current.clipboardWatch) }
    var resume by remember(current) { mutableStateOf(current.resumeInterrupted) }
    var sound by remember(current) { mutableStateOf(current.completionSoundEnabled) }
    var progressWindow by remember(current) { mutableStateOf(current.progressWindowEnabled) }
    var completePopup by remember(current) { mutableStateOf(current.completePopupEnabled) }
    var takeoverEnabled by remember(current) { mutableStateOf(current.takeoverEnabled) }
    var takeoverMinimum by remember(current) { mutableStateOf(current.takeoverMinimumBytes.toString()) }
    var subtitles by remember(current) { mutableStateOf(current.downloadSubtitles) }
    var skipAds by remember(current) { mutableStateOf(current.skipAdSegments) }
    var keepTemp by remember(current) { mutableStateOf(current.keepTempFiles) }
    var filePolicy by remember(current) { mutableStateOf(current.existingFilePolicy) }
    var chunkSize by remember(current) { mutableStateOf(current.httpChunkSizeMb.toString()) }
    var liveMax by remember(current) { mutableStateOf(current.liveRecordMaxMinutes.toString()) }
    var referer by remember(current) { mutableStateOf(current.defaultReferer) }
    var userAgent by remember(current) { mutableStateOf(current.defaultUserAgent) }
    WorkbenchDialog(onDismiss, "设置", "界面、下载行为与运行环境", 760.dp, content = {
        Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(8.dp)).background(surface2).padding(4.dp)) { listOf("通用", "下载", "浏览器", "外观").forEach { item -> TextButton(onClick = { tab = item }, Modifier.weight(1f).clip(RoundedCornerShape(6.dp)).background(if (tab == item) rail else Color.Transparent)) { Text(item, color = if (tab == item) blue else muted, fontSize = 12.sp, fontWeight = if (tab == item) FontWeight.SemiBold else FontWeight.Medium) } } }
        Spacer(Modifier.height(16.dp))
        when (tab) {
            "通用" -> SettingsSection("运行行为") { DialogLabel("默认下载目录"); OutlinedTextField(downloadDirectory, { downloadDirectory = it }, Modifier.fillMaxWidth(), singleLine = true, shape = RoundedCornerShape(7.dp), placeholder = { Text("使用下载引擎默认目录") }); Spacer(Modifier.height(10.dp)); SettingRow("监视剪贴板", "检测到下载链接后在界面中提示", clipboardWatch) { clipboardWatch = it }; SettingRow("启动时恢复任务", "恢复上次未完成的下载任务", resume) { resume = it }; SettingRow("下载完成提示音", "任务完成时播放系统提示音", sound) { sound = it }; SettingRow("下载中窗口", "显示当前活动任务的紧凑进度", progressWindow) { progressWindow = it }; SettingRow("完成通知", "下载完成后显示本机完成提示", completePopup) { completePopup = it } }
            "下载" -> SettingsSection("下载引擎") {
                DialogLabel("默认并发连接数"); OutlinedTextField(concurrency, { concurrency = it.filter(Char::isDigit) }, Modifier.fillMaxWidth(), singleLine = true, shape = RoundedCornerShape(7.dp)); Spacer(Modifier.height(10.dp))
                DialogLabel("最大同时任务数"); OutlinedTextField(queueMax, { queueMax = it.filter(Char::isDigit) }, Modifier.fillMaxWidth(), singleLine = true, shape = RoundedCornerShape(7.dp)); Spacer(Modifier.height(10.dp))
                DialogLabel("全局限速 KiB/s（0 为不限速）"); OutlinedTextField(limit, { limit = it.filter(Char::isDigit) }, Modifier.fillMaxWidth(), singleLine = true, shape = RoundedCornerShape(7.dp)); Spacer(Modifier.height(10.dp))
                DialogLabel("失败自动重试次数"); OutlinedTextField(retryMax, { retryMax = it.filter(Char::isDigit) }, Modifier.fillMaxWidth(), singleLine = true, shape = RoundedCornerShape(7.dp)); Spacer(Modifier.height(10.dp))
                DialogLabel("同名文件处理"); Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(7.dp)).background(rail).border(BorderStroke(1.dp, border), RoundedCornerShape(7.dp)).padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) { listOf("rename" to "自动重命名", "overwrite" to "覆盖", "skip" to "跳过").forEach { (value, label) -> TextButton(onClick = { filePolicy = value }, Modifier.weight(1f).clip(RoundedCornerShape(5.dp)).background(if (filePolicy == value) selectedSurface else Color.Transparent)) { Text(label, color = if (filePolicy == value) blue else muted, fontSize = 11.sp) } } }
                Spacer(Modifier.height(10.dp)); DialogLabel("HTTP 分段大小 MiB"); OutlinedTextField(chunkSize, { chunkSize = it.filter(Char::isDigit) }, Modifier.fillMaxWidth(), singleLine = true, shape = RoundedCornerShape(7.dp)); Spacer(Modifier.height(8.dp)); DialogLabel("直播录制时长上限（分钟，0 为不限）"); OutlinedTextField(liveMax, { liveMax = it.filter(Char::isDigit) }, Modifier.fillMaxWidth(), singleLine = true, shape = RoundedCornerShape(7.dp)); SettingRow("下载外挂字幕", "HLS 有字幕时额外保存字幕文件", subtitles) { subtitles = it }; SettingRow("跳过广告分片", "跳过清单明确标记的广告片段", skipAds) { skipAds = it }; SettingRow("保留过程文件", "暂停或失败后保留分片和调试日志", keepTemp) { keepTemp = it }
                DialogLabel("默认 Referer（可选）"); OutlinedTextField(referer, { referer = it }, Modifier.fillMaxWidth(), singleLine = true, shape = RoundedCornerShape(7.dp), placeholder = { Text("留空则按任务来源") }); Spacer(Modifier.height(8.dp)); DialogLabel("默认 User-Agent（可选）"); OutlinedTextField(userAgent, { userAgent = it }, Modifier.fillMaxWidth(), singleLine = true, shape = RoundedCornerShape(7.dp), placeholder = { Text("留空则使用下载引擎默认值") })
            }
            "浏览器" -> SettingsSection("浏览器下载接管") { SettingRow("接管浏览器下载", "浏览器插件识别到资源时显示下载确认", takeoverEnabled) { takeoverEnabled = it }; DialogLabel("最小接管大小（字节，0 为不限制）"); OutlinedTextField(takeoverMinimum, { takeoverMinimum = it.filter(Char::isDigit) }, Modifier.fillMaxWidth(), singleLine = true, shape = RoundedCornerShape(7.dp)); Text("低于此大小的浏览器请求仍由浏览器处理。", color = faint, fontSize = 10.sp, modifier = Modifier.padding(top = 6.dp)) }
            else -> SettingsSection("显示") { SettingRow("深色模式", "切换下载工作台的深色外观", dark) { dark = it } }
        }
    }, actions = { DialogSecondary("取消", onDismiss); DialogPrimary("保存设置") { onSave(current.copy(darkMode = dark, downloadDirectory = downloadDirectory.trim(), defaultConcurrency = concurrency.toLongOrNull()?.coerceIn(1, 128) ?: current.defaultConcurrency, speedLimitKib = limit.toLongOrNull()?.coerceAtMost(10_000_000) ?: current.speedLimitKib, queueMax = queueMax.toLongOrNull()?.coerceIn(1, 128) ?: current.queueMax, autoRetryMax = retryMax.toLongOrNull()?.coerceIn(0, 10) ?: current.autoRetryMax, clipboardWatch = clipboardWatch, resumeInterrupted = resume, completionSoundEnabled = sound, progressWindowEnabled = progressWindow, completePopupEnabled = completePopup, takeoverEnabled = takeoverEnabled, takeoverMinimumBytes = takeoverMinimum.toLongOrNull()?.coerceAtLeast(0) ?: current.takeoverMinimumBytes, downloadSubtitles = subtitles, skipAdSegments = skipAds, keepTempFiles = keepTemp, existingFilePolicy = filePolicy, httpChunkSizeMb = chunkSize.toLongOrNull()?.coerceIn(1, 64) ?: current.httpChunkSizeMb, liveRecordMaxMinutes = liveMax.toLongOrNull()?.coerceIn(0, 2880) ?: current.liveRecordMaxMinutes, defaultReferer = referer.trim(), defaultUserAgent = userAgent.trim())); onDismiss() } })
}
@Composable internal fun SettingsSection(title: String, content: @Composable ColumnScope.() -> Unit) = Column(Modifier.fillMaxWidth().padding(end = 5.dp, bottom = 14.dp), content = { Text(title, color = ink, fontSize = 13.sp, fontWeight = FontWeight.SemiBold); Spacer(Modifier.height(12.dp)); content(); Spacer(Modifier.height(8.dp)) })
@Composable internal fun SettingRow(label: String, detail: String, checked: Boolean, onChecked: (Boolean) -> Unit) = Row(Modifier.fillMaxWidth().padding(vertical = 5.dp), verticalAlignment = Alignment.CenterVertically) { Column(Modifier.weight(1f)) { Text(label, color = ink, fontSize = 12.sp, fontWeight = FontWeight.Medium); Text(detail, color = faint, fontSize = 10.sp, modifier = Modifier.padding(top = 3.dp)) }; Switch(checked, onChecked, accessibilityLabel = label) }

@Composable private fun TaskDetailsDialog(
    task: DownloadTask,
    onDismiss: () -> Unit,
    onRefreshRequest: (String, String) -> Unit,
    onAction: (String) -> Unit,
) {
    var tab by remember(task.id) { mutableStateOf("概览") }
    var speedLimit by remember(task.id, task.source.speedLimitKib) { mutableStateOf(task.source.speedLimitKib.toString()) }
    var refreshUrl by remember(task.id) { mutableStateOf(task.source.url) }
    var refreshCookie by remember(task.id) { mutableStateOf("") }
    var showRefresh by remember(task.id) { mutableStateOf(false) }
    var diagnosticsCopied by remember(task.id) { mutableStateOf(false) }
    var torrentFiles by remember(task.id) { mutableStateOf<List<TorrentFileDto>>(emptyList()) }
    var torrentLoading by remember(task.id) { mutableStateOf(false) }
    var torrentBusy by remember(task.id) { mutableStateOf(false) }
    var torrentNotice by remember(task.id) { mutableStateOf("") }
    val detailScope = rememberCoroutineScope()
    val torrentTask = task.source.resourceKind.equals("torrent", true)
    val canRefresh = task.source.availableActions.any { it in setOf("start", "resume", "retry") }
    val canPreview = task.status == "已完成" && !task.source.outputMissing && isPreviewableImage(task.source.outputPath)
    var preview by remember(task.id, task.source.outputPath) { mutableStateOf<Result<ImageBitmap>?>(null) }
    LaunchedEffect(canPreview, task.source.outputPath) {
        preview = if (canPreview) withContext(Dispatchers.IO) { loadLocalImagePreview(task.source.outputPath) } else null
    }
    LaunchedEffect(task.id, torrentTask) {
        if (!torrentTask) return@LaunchedEffect
        torrentLoading = true
        runCatching { withContext(Dispatchers.IO) { EnginePipeClient().getTaskTorrentFiles(task.id) } }
            .onSuccess { torrentFiles = it.files }
            .onFailure { torrentNotice = it.message ?: "读取 BT 文件清单失败" }
        torrentLoading = false
    }
    WorkbenchDialog(onDismiss, "任务详情", "进度、连接、速度和运行日志", 780.dp, content = {
        Text(task.filename, color = ink, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, maxLines = 2, overflow = TextOverflow.Ellipsis)
        Spacer(Modifier.height(12.dp))
        LinearProgressIndicator(progress = { task.progress }, modifier = Modifier.fillMaxWidth().height(7.dp).clip(RoundedCornerShape(4.dp)), color = blue, trackColor = surface3)
        Row(Modifier.padding(top = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            StatusBadge(task.status); Spacer(Modifier.width(9.dp))
            Text(
                if (torrentTask) "${(task.progress * 100).toInt()}% · ${task.speed} · ${task.segments}"
                else "${(task.progress * 100).toInt()}% · ${task.speed} · ${task.segments} · ${task.source.activeWorkers} 个连接",
                color = muted,
                fontSize = 11.sp,
            )
        }
        Spacer(Modifier.height(14.dp))
        Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(7.dp)).background(surface2).padding(3.dp)) {
            (listOf("概览", "连接", "速度", "日志") + if (canPreview) listOf("预览") else emptyList()).forEach { item ->
                TextButton(onClick = { tab = item; if (item == "日志") onAction("log") }, Modifier.weight(1f).clip(RoundedCornerShape(5.dp)).background(if (tab == item) rail else Color.Transparent)) { Text(item, color = if (tab == item) blue else muted, fontSize = 11.sp, fontWeight = if (tab == item) FontWeight.SemiBold else FontWeight.Normal) }
            }
        }
        Spacer(Modifier.height(12.dp))
        when (tab) {
            "概览" -> {
                DetailLine("链接", safeResourceLocation(task.source.url))
                DetailLine("类型", resourceKindLabel(task.source.resourceKind))
                DetailLine("请求", task.source.requestMethod)
                DetailLine("保存到", task.source.outputPath.ifBlank { task.source.downloadDirectory.ifBlank { "默认下载目录" } })
                DetailLine("阶段", task.source.stage.ifBlank { "—" })
                if (torrentTask) {
                    DetailLine("Piece", "${task.source.completedRanges}/${task.source.totalRanges}")
                    DetailLine("Peer / Seed", "${task.source.peerCount} / ${task.source.seedCount}")
                    DetailLine("上传速度", formatRate(task.source.uploadSpeedBytesPerSecond))
                    if (task.source.uploadedBytes > 0) DetailLine("已上传", formatBytes(task.source.uploadedBytes))
                    DetailLine("已选大小", task.source.totalBytes?.let(::formatBytes) ?: "—")
                } else {
                    DetailLine("连接", "${task.source.activeWorkers}/${task.source.maxWorkers.takeIf { it > 0 } ?: task.source.activeWorkers.coerceAtLeast(1)}")
                }
                if (task.source.scheduledStartAt.isNotBlank()) DetailLine("开始", task.source.scheduledStartAt)
                if (task.source.scheduledStopAt.isNotBlank()) DetailLine("停止", task.source.scheduledStopAt)
                if (task.source.outputMissing) DetailLine("输出", "文件已删除，可重新下载", Color(0xFFB42318))
                TaskVerificationDetails(task.source)
                taskFailureDetails(task.source)?.let { failure ->
                    Spacer(Modifier.height(13.dp))
                    Surface(
                        Modifier.fillMaxWidth(),
                        color = Color(0xFFFFF3F2),
                        shape = RoundedCornerShape(7.dp),
                        border = BorderStroke(1.dp, Color(0xFFF2C9C5)),
                    ) {
                        Column(Modifier.padding(12.dp)) {
                            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                                Icon(Icons.Outlined.ErrorOutline, null, tint = Color(0xFFB42318), modifier = Modifier.size(17.dp))
                                Spacer(Modifier.width(7.dp))
                                Text(failure.title, Modifier.weight(1f), color = Color(0xFF8A1C13), fontSize = 12.sp, fontWeight = FontWeight.SemiBold)
                                TextButton(onClick = {
                                    runCatching { Toolkit.getDefaultToolkit().systemClipboard.setContents(StringSelection(taskFailureDiagnostic(task)), null) }
                                    diagnosticsCopied = true
                                }, contentPadding = PaddingValues(horizontal = 7.dp, vertical = 2.dp)) { Text(if (diagnosticsCopied) "已复制" else "复制诊断", color = blue, fontSize = 10.sp) }
                            }
                            failure.items.forEach { (label, value) -> DetailLine(label, value, Color(0xFF7A2E28)) }
                            task.source.errorMessage?.takeIf(String::isNotBlank)?.let { message ->
                                Spacer(Modifier.height(8.dp)); Text("失败原因", color = Color(0xFF8A1C13), fontSize = 10.sp, fontWeight = FontWeight.SemiBold)
                                Text(redactDiagnosticText(message), color = Color(0xFF7A2E28), fontSize = 11.sp, lineHeight = 16.sp, modifier = Modifier.padding(top = 3.dp))
                            }
                            if (failure.steps.isNotEmpty()) {
                                Spacer(Modifier.height(8.dp)); Text("建议步骤", color = Color(0xFF8A1C13), fontSize = 10.sp, fontWeight = FontWeight.SemiBold)
                                failure.steps.forEachIndexed { index, step -> Text("${index + 1}. $step", color = Color(0xFF7A2E28), fontSize = 11.sp, lineHeight = 17.sp, modifier = Modifier.padding(top = 2.dp)) }
                            }
                        }
                    }
                }
                if (torrentTask) {
                    Spacer(Modifier.height(13.dp)); HorizontalDivider(color = border); Spacer(Modifier.height(11.dp))
                    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                        Text("BT 文件选择", Modifier.weight(1f), color = ink, fontSize = 12.sp, fontWeight = FontWeight.SemiBold)
                        Text("已选 ${torrentFiles.count { it.selected }} / ${torrentFiles.size} · ${formatBytes(torrentFiles.filter { it.selected }.sumOf { it.size })}", color = muted, fontSize = 10.sp)
                    }
                    Row(Modifier.padding(top = 5.dp), horizontalArrangement = Arrangement.spacedBy(5.dp)) {
                        TextButton(onClick = { torrentFiles = torrentFiles.map { it.copy(selected = true) } }, enabled = torrentFiles.isNotEmpty(), contentPadding = PaddingValues(horizontal = 7.dp)) { Text("全选", fontSize = 10.sp) }
                        TextButton(onClick = { torrentFiles = torrentFiles.map { it.copy(selected = !it.selected) } }, enabled = torrentFiles.isNotEmpty(), contentPadding = PaddingValues(horizontal = 7.dp)) { Text("反选", fontSize = 10.sp) }
                    }
                    when {
                        torrentLoading -> Text("正在读取种子元数据…", color = muted, fontSize = 11.sp, modifier = Modifier.padding(vertical = 18.dp))
                        torrentFiles.isEmpty() -> Text("尚未取得文件清单。磁力任务需要先取得元数据。", color = muted, fontSize = 11.sp, modifier = Modifier.padding(vertical = 14.dp))
                        else -> Column(Modifier.fillMaxWidth().heightIn(max = 220.dp).verticalScroll(rememberScrollState()).border(BorderStroke(1.dp, border), RoundedCornerShape(7.dp)).padding(horizontal = 9.dp, vertical = 5.dp)) {
                            torrentFiles.forEachIndexed { index, file ->
                                Row(Modifier.fillMaxWidth().padding(vertical = 3.dp), verticalAlignment = Alignment.CenterVertically) {
                                    Checkbox(file.selected, { checked -> torrentFiles = torrentFiles.toMutableList().also { it[index] = file.copy(selected = checked) } }, accessibilityLabel = "选择 ${file.path}")
                                    Spacer(Modifier.width(6.dp)); Text(file.path, Modifier.weight(1f), color = ink, fontSize = 11.sp, maxLines = 2, overflow = TextOverflow.Ellipsis)
                                    Spacer(Modifier.width(8.dp)); Text(formatBytes(file.size), color = muted, fontSize = 10.sp)
                                }
                            }
                        }
                    }
                    if (task.status == "进行中") Text("当前内置 BT 引擎需先暂停任务，才能安全调整文件选择。", color = Color(0xFFB54708), fontSize = 10.sp, modifier = Modifier.padding(top = 6.dp))
                    if (torrentNotice.isNotBlank()) Text(torrentNotice, color = if (torrentNotice.contains("已保存")) Color(0xFF078C46) else Color(0xFFB42318), fontSize = 10.sp, modifier = Modifier.padding(top = 6.dp))
                    DialogPrimary(if (torrentBusy) "正在保存…" else "保存文件选择", !torrentBusy && task.status != "进行中" && torrentFiles.any { it.selected }) {
                        torrentBusy = true; torrentNotice = ""
                        detailScope.launch {
                            runCatching { withContext(Dispatchers.IO) { EnginePipeClient().setTaskTorrentFiles(task.id, torrentFiles) } }
                                .onSuccess { torrentFiles = it.files; torrentNotice = "文件选择已保存，将在开始或恢复时生效" }
                                .onFailure { torrentNotice = it.message ?: "保存文件选择失败" }
                            torrentBusy = false
                        }
                    }
                }
                if (task.status != "已完成") {
                    Spacer(Modifier.height(13.dp)); HorizontalDivider(color = border); Spacer(Modifier.height(11.dp)); DialogLabel("任务限速 KiB/s（0 不限制）")
                    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) { OutlinedTextField(speedLimit, { speedLimit = it.filter(Char::isDigit) }, Modifier.weight(1f), singleLine = true, shape = RoundedCornerShape(7.dp)); Spacer(Modifier.width(8.dp)); DialogSecondary("应用") { onAction("speed:${speedLimit.toLongOrNull()?.coerceIn(0, 1_048_576) ?: 0}") } }
                    if (canRefresh) { Spacer(Modifier.height(8.dp)); TextButton(onClick = { showRefresh = !showRefresh }, contentPadding = PaddingValues(0.dp)) { Text(if (showRefresh) "收起链接更新" else "更新下载链接", color = blue, fontSize = 11.sp) } }
                    if (showRefresh) {
                        OutlinedTextField(refreshUrl, { refreshUrl = it }, Modifier.fillMaxWidth(), label = { Text("新的资源地址") }, minLines = 2, maxLines = 3, shape = RoundedCornerShape(7.dp))
                        Spacer(Modifier.height(7.dp))
                        OutlinedTextField(refreshCookie, { refreshCookie = it }, Modifier.fillMaxWidth(), singleLine = true, visualTransformation = PasswordVisualTransformation(), label = { Text("新的 Cookie（可选）") }, shape = RoundedCornerShape(7.dp))
                        Text("留空保留同站点原凭据；跨站地址会自动丢弃旧凭据。适合 401、403 和短效签名过期。", color = muted, fontSize = 10.sp, lineHeight = 15.sp, modifier = Modifier.padding(top = 5.dp))
                        DialogPrimary("更新并继续", refreshUrl.isNotBlank()) {
                            onRefreshRequest(refreshUrl.trim(), refreshCookie)
                            refreshCookie = ""
                        }
                    }
                }
            }
            "连接" -> ConnectionMap(task.source.connectionParts, task.source.connectionHint)
            "速度" -> SpeedHistory(task.source.speedHistory)
            "预览" -> ImagePreview(preview)
            else -> Surface(Modifier.fillMaxWidth().heightIn(min = 210.dp, max = 320.dp), color = surface2, shape = RoundedCornerShape(7.dp), border = BorderStroke(1.dp, border)) { Text(task.source.logTail.ifEmpty { listOf("暂无日志记录") }.joinToString("\n"), Modifier.padding(12.dp), color = muted, fontSize = 11.sp, lineHeight = 17.sp) }
        }
    }, actions = {
        val mediaCapable = taskSupportsMediaActions(task.source)
        if (mediaCapable && (task.source.playbackReady || task.source.resourceKind in setOf("hls", "dash", "live"))) DialogSecondary("播放") { onAction("play") }
        if (mediaCapable && task.source.playbackReady) DialogSecondary("投屏") { onAction("cast") }
        if (mediaCapable && task.source.playbackReady) DialogSecondary("TVBox") { onAction("push_tvbox") }
        DialogSecondary("站点规则") { onAction("save_site_profile") }
        task.source.availableActions.filter { it in setOf("start", "pause", "resume", "retry", "open", "open_folder") }.take(2).forEach { action -> DialogSecondary(actionLabel(action)) { onAction(action) } }
        DialogPrimary("关闭", onClick = onDismiss)
    })
}

private val previewImageExtensions = setOf("png", "jpg", "jpeg", "webp", "gif", "bmp")

internal fun isPreviewableImage(path: String): Boolean = path.substringAfterLast('.', "").lowercase() in previewImageExtensions

internal fun loadLocalImagePreview(path: String): Result<ImageBitmap> = runCatching {
    val file = File(path)
    require(file.isFile) { "图片文件不存在" }
    require(file.length() in 1..(32L * 1024 * 1024)) { "图片过大，最多预览 32 MiB" }
    val image = SkiaImage.makeFromEncoded(Files.readAllBytes(file.toPath()))
    require(image.width.toLong() * image.height.toLong() <= 40_000_000L) { "图片尺寸过大" }
    image.toComposeImageBitmap()
}

@Composable private fun ImagePreview(preview: Result<ImageBitmap>?) {
    Surface(Modifier.fillMaxWidth().heightIn(min = 220.dp, max = 380.dp), color = surface2, shape = RoundedCornerShape(7.dp), border = BorderStroke(1.dp, border)) {
        Box(Modifier.fillMaxSize().padding(10.dp), contentAlignment = Alignment.Center) {
            when {
                preview == null -> Text("正在读取图片…", color = muted, fontSize = 11.sp)
                preview.isFailure -> Text(preview.exceptionOrNull()?.message ?: "图片预览失败", color = Color(0xFFB42318), fontSize = 11.sp)
                else -> Image(preview.getOrThrow(), "已下载图片预览", Modifier.fillMaxSize(), contentScale = ContentScale.Fit)
            }
        }
    }
}

@Composable private fun ConnectionMap(parts: List<ConnectionPartDto>, hint: String) {
    Column {
        Text(hint.ifBlank { if (parts.isEmpty()) "当前任务没有活动分段" else "${parts.size} 个连接分段" }, color = muted, fontSize = 11.sp)
        Spacer(Modifier.height(10.dp))
        if (parts.isEmpty()) Surface(Modifier.fillMaxWidth().height(84.dp), color = surface2, shape = RoundedCornerShape(7.dp)) { Box(contentAlignment = Alignment.Center) { Text("下载开始后显示各连接覆盖范围", color = faint, fontSize = 11.sp) } }
        else Column(verticalArrangement = Arrangement.spacedBy(6.dp)) { parts.take(24).chunked(8).forEach { row -> Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(6.dp)) { row.forEach { part -> val length = (part.end - part.start + 1).coerceAtLeast(1); val progress = (part.done.toFloat() / length).coerceIn(0f, 1f); Column(Modifier.weight(1f)) { LinearProgressIndicator(progress = { progress }, modifier = Modifier.fillMaxWidth().height(7.dp).clip(RoundedCornerShape(3.dp)), color = if (part.state == "failed") Color(0xFFDC2626) else blue, trackColor = surface3); Text("${(progress * 100).toInt()}%", color = faint, fontSize = 9.sp) } }; repeat(8 - row.size) { Spacer(Modifier.weight(1f)) } } } }
    }
}

@Composable private fun SpeedHistory(history: List<Long>) {
    val values = history.takeLast(48)
    val maximum = values.maxOrNull()?.coerceAtLeast(1) ?: 1
    Column {
        Text(if (values.isEmpty()) "暂无速度采样" else "峰值 ${formatRate(maximum)} · 最近 ${values.size} 个采样", color = muted, fontSize = 11.sp)
        Spacer(Modifier.height(10.dp))
        Row(Modifier.fillMaxWidth().height(150.dp).clip(RoundedCornerShape(7.dp)).background(surface2).padding(horizontal = 10.dp, vertical = 12.dp), horizontalArrangement = Arrangement.spacedBy(3.dp), verticalAlignment = Alignment.Bottom) {
            if (values.isEmpty()) Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { Text("任务运行后显示实时速度", color = faint, fontSize = 11.sp) }
            else values.forEach { value -> Box(Modifier.weight(1f).fillMaxHeight((value.toFloat() / maximum).coerceIn(.03f, 1f)).clip(RoundedCornerShape(2.dp)).background(blue.copy(alpha = .78f))) }
        }
    }
}
@Composable
private fun TaskVerificationDetails(task: TaskDto) {
    val mirrorResults = task.mirrorStatus.ifEmpty {
        task.mirrors.map { MirrorStatusDto(url = it) }
    }
    if (task.expectedChecksum.isBlank() && task.avScan == null && mirrorResults.isEmpty()) return
    Spacer(Modifier.height(13.dp))
    HorizontalDivider(color = border)
    Text("完成检查", color = ink, fontSize = 12.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(top = 11.dp))
    if (task.expectedChecksum.isNotBlank()) {
        val result = when (task.checksumVerified) {
            true -> "已通过"
            false -> "不匹配"
            null -> "等待下载完成"
        }
        val resultColor = when (task.checksumVerified) {
            true -> Color(0xFF15803D)
            false -> Color(0xFFB42318)
            null -> muted
        }
        DetailLine("文件校验", result, resultColor)
        DetailLine("期望", task.expectedChecksum)
        if (task.checksumAlgorithm.isNotBlank()) DetailLine("算法", task.checksumAlgorithm)
        if (task.checksumActual.isNotBlank()) DetailLine("实际", task.checksumActual, resultColor)
    }
    task.avScan?.let { scan ->
        val result = when (scan.state) {
            "clean" -> "未发现威胁"
            "threat" -> "发现威胁"
            "running" -> "正在扫描"
            "skipped" -> "已跳过"
            else -> "扫描异常"
        }
        DetailLine("病毒扫描", result, if (scan.state == "clean") Color(0xFF15803D) else if (scan.state == "threat") Color(0xFFB42318) else muted)
        if (scan.engine.isNotBlank()) DetailLine("扫描引擎", scan.engine)
        if (scan.detail.isNotBlank()) DetailLine("扫描详情", scan.detail)
    }
    mirrorResults.forEach { mirror ->
        val label = when (mirror.state) {
            "active" -> "镜像使用中"
            "failed" -> "镜像失败"
            "skipped" -> "镜像已跳过"
            else -> "镜像待探测"
        }
        val detail = buildList {
            add(safeResourceLocation(mirror.url))
            if (mirror.ranges) add("支持分段")
            mirror.detail.takeIf(String::isNotBlank)?.let(::add)
        }.joinToString(" · ")
        DetailLine(label, detail, if (mirror.state == "failed") Color(0xFFB42318) else null)
    }
}

@Composable private fun DetailLine(label: String, value: String, color: Color? = null) = Row(Modifier.fillMaxWidth().padding(top = 9.dp)) { Text(label, Modifier.width(72.dp), color = faint, fontSize = 11.sp); Text(value, Modifier.weight(1f), color = color ?: muted, fontSize = 11.sp, maxLines = 2, overflow = TextOverflow.Ellipsis) }

internal fun safeResourceLocation(value: String): String {
    val raw = value.trim()
    if (raw.isBlank()) return "—"
    val uri = runCatching { URI(raw) }.getOrNull()
    if (uri?.scheme.equals("magnet", true)) return "magnet · BT 资源"
    val host = uri?.host?.takeIf(String::isNotBlank) ?: return raw.substringBefore('?').substringBefore('#').take(180)
    val port = uri.port.takeIf { it > 0 }?.let { ":$it" }.orEmpty()
    val segments = uri.path.orEmpty().split('/').filter(String::isNotBlank).takeLast(2)
    return buildString {
        append(host)
        append(port)
        if (segments.isNotEmpty()) append('/').append(segments.joinToString("/"))
    }.take(180)
}

internal data class TaskFailureDetails(
    val title: String,
    val items: List<Pair<String, String>>,
    val steps: List<String>,
)

internal fun taskFailureDetails(task: TaskDto): TaskFailureDetails? {
    if (task.errorCode.isNullOrBlank() && task.errorMessage.isNullOrBlank()) return null
    val items = buildList {
        task.errorStage.takeIf(String::isNotBlank)?.let { add("发生阶段" to failureStageLabel(it)) }
        task.httpStatus?.let { add("HTTP 状态" to it.toString()) }
        task.errorCode?.takeIf(String::isNotBlank)?.let { add("错误代码" to it) }
        task.errorAttempt.takeIf { it > 0 }?.let { add("尝试次数" to "$it 次") }
        task.errorUrl.takeIf(String::isNotBlank)?.let { add("资源地址" to safeResourceLocation(it)) }
    }
    val steps = when (task.httpStatus) {
        401 -> listOf("确认已登录原网站", "通过浏览器插件重新发送并授权当前站点凭据")
        403 -> listOf("回到来源网页刷新并重新打开下载入口", "通过浏览器插件重新发送资源，不要只重复点击重试")
        404 -> listOf("回到来源网页确认资源仍可访问", "重新识别并创建有效下载地址")
        408, 425, 429 -> listOf("降低任务并发与同时下载数", "等待片刻后重试")
        in 500..599 -> listOf("稍后重试", "有备用地址时切换镜像")
        else -> task.errorHint.takeIf(String::isNotBlank)?.let(::listOf).orEmpty()
    }
    return TaskFailureDetails(
        title = task.errorCode?.takeIf(String::isNotBlank)?.let { "下载失败 · $it" } ?: "下载失败",
        items = items,
        steps = steps,
    )
}

internal fun taskFailureDiagnostic(task: DownloadTask): String {
    val failure = taskFailureDetails(task.source)
    return buildList {
        add("任务: ${task.filename}")
        add("链接（已脱敏）: ${safeResourceLocation(task.source.url)}")
        add("状态: ${task.source.status}")
        failure?.items?.forEach { (label, value) -> add("$label: $value") }
        task.source.errorMessage?.takeIf(String::isNotBlank)?.let { add("失败原因: ${redactDiagnosticText(it)}") }
        add("最近日志: ${task.source.logTail.lastOrNull()?.let(::redactDiagnosticText) ?: "—"}")
    }.joinToString("\n")
}

internal fun redactDiagnosticText(value: String): String = Regex("https?://\\S+", RegexOption.IGNORE_CASE)
    .replace(value) { match -> safeResourceLocation(match.value.trimEnd('.', ',', ';', ')')) }
    .take(1200)

private fun failureStageLabel(stage: String) = when (stage.lowercase()) {
    "transfer", "downloading" -> "下载文件"
    "downloading_m3u8" -> "获取播放清单"
    "downloading_segments" -> "下载媒体分片"
    "merging" -> "合并文件"
    "remuxing" -> "转封装"
    "checksum" -> "文件校验"
    "size" -> "大小校验"
    "av_scan" -> "病毒扫描"
    else -> stage
}

@Composable private fun ExtensionDialog(status: String, onDismiss: () -> Unit) = WorkbenchDialog(onDismiss, "浏览器插件", "识别网页媒体并交给下载器", 550.dp, content = { Surface(Modifier.fillMaxWidth(), color = if (status.contains("已连接")) Color(0xFFEAF8EF) else surface2, shape = RoundedCornerShape(8.dp)) { Row(Modifier.padding(13.dp), verticalAlignment = Alignment.CenterVertically) { Icon(if (status.contains("已连接")) Icons.Outlined.CheckCircle else Icons.Outlined.Extension, null, tint = if (status.contains("已连接")) Color(0xFF16A34A) else blue); Spacer(Modifier.width(10.dp)); Text(status, color = if (status.contains("已连接")) Color(0xFF15803D) else ink, fontSize = 12.sp, fontWeight = FontWeight.SemiBold) } }; Spacer(Modifier.height(14.dp)); Text("安装或更新插件后，重新打开浏览器标签页即可建立连接。插件会识别下载点击、媒体清单、音视频轨道和网页播放器，不影响页面的其他功能。", color = muted, fontSize = 12.sp, lineHeight = 19.sp) }, actions = { DialogPrimary("完成", onClick = onDismiss) })

@Composable private fun NoticeToast(signal: UiSignal.Notice, onDismiss: () -> Unit) {
    LaunchedEffect(signal) { delay(3_600); onDismiss() }
    Popup(alignment = Alignment.TopEnd, offset = androidx.compose.ui.unit.IntOffset(-18, 72), properties = PopupProperties(focusable = false)) {
        val tone = if (signal.level == "error") Color(0xFFB42318) else if (signal.level == "success") Color(0xFF078C46) else blue
        Surface(color = dialogSurface, shape = RoundedCornerShape(8.dp), shadowElevation = 8.dp, border = BorderStroke(1.dp, border), modifier = Modifier.widthIn(min = 300.dp, max = 440.dp)) {
            Row(Modifier.padding(horizontal = 14.dp, vertical = 12.dp), verticalAlignment = Alignment.CenterVertically) {
                Icon(if (signal.level == "error") Icons.Outlined.ErrorOutline else Icons.Outlined.Info, null, tint = tone, modifier = Modifier.size(19.dp))
                Spacer(Modifier.width(10.dp)); Text(signal.message, color = ink, fontSize = 12.sp, modifier = Modifier.weight(1f), maxLines = 3, overflow = TextOverflow.Ellipsis)
                IconButton(onClick = onDismiss, modifier = Modifier.size(28.dp)) { Icon(Icons.Outlined.Close, "关闭", tint = muted, modifier = Modifier.size(16.dp)) }
            }
        }
    }
}

@Composable private fun TorrentSelectionDialog(data: TorrentProbeDto, onDismiss: () -> Unit, onCreate: (List<TorrentFileDto>) -> Unit) {
    var files by remember(data) { mutableStateOf(data.files.map { it.copy(selected = true) }) }
    val selectedBytes = files.filter { it.selected }.sumOf { it.size }
    WorkbenchDialog(onDismiss, "选择种子文件", "${data.name} · ${files.size} 个文件", 680.dp, content = {
        Row(Modifier.fillMaxWidth().padding(bottom = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            Checkbox(files.all { it.selected }, { checked -> files = files.map { it.copy(selected = checked) } }, accessibilityLabel = "选择全部种子文件")
            Spacer(Modifier.width(6.dp)); Text("全选", color = ink, fontSize = 12.sp)
            Spacer(Modifier.weight(1f)); Text("已选 ${files.count { it.selected }} / ${files.size} · ${formatBytes(selectedBytes)}", color = muted, fontSize = 11.sp)
        }
        HorizontalDivider(color = border)
        if (files.isEmpty()) {
            Column(Modifier.fillMaxWidth().padding(vertical = 34.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                Text("未取得种子文件清单", color = ink, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
                Text("磁力链接还没有返回完整元数据", color = muted, fontSize = 11.sp, modifier = Modifier.padding(top = 6.dp))
            }
        } else {
            Column(Modifier.fillMaxWidth().heightIn(max = 390.dp).verticalScroll(rememberScrollState())) {
                files.forEachIndexed { index, file ->
                    Row(Modifier.fillMaxWidth().padding(vertical = 5.dp), verticalAlignment = Alignment.CenterVertically) {
                        Checkbox(file.selected, { checked -> files = files.toMutableList().also { it[index] = file.copy(selected = checked) } }, accessibilityLabel = "选择 ${file.path}")
                        Spacer(Modifier.width(6.dp)); Column(Modifier.weight(1f)) {
                            Text(file.path, color = ink, fontSize = 12.sp, maxLines = 2, overflow = TextOverflow.Ellipsis)
                            Text(formatBytes(file.size), color = muted, fontSize = 10.sp)
                        }
                    }
                }
            }
        }
    }, actions = {
        DialogSecondary("取消", onDismiss)
        if (files.isEmpty()) {
            DialogPrimary("直接创建磁力任务", true) { onCreate(emptyList()) }
        } else {
            DialogPrimary("创建已选任务", files.any { it.selected }) { onCreate(files) }
        }
    })
}

@Composable private fun ProbeResultDialog(signal: UiSignal.Probe, onDismiss: () -> Unit, onCreate: (StreamVariantDto?) -> Unit) {
    var selected by remember(signal) { mutableStateOf<StreamVariantDto?>(signal.variants.maxByOrNull { it.bandwidth }) }
    WorkbenchDialog(onDismiss, "媒体资源", "选择清晰度、音轨或直接使用自动选择", 660.dp, content = {
        DetailLine("资源链接", safeResourceLocation(signal.url))
        Spacer(Modifier.height(12.dp))
        if (signal.variants.isEmpty()) Text("下载引擎未发现可选择的媒体轨道，将使用自动识别结果。", color = muted, fontSize = 12.sp)
        else signal.variants.forEach { variant ->
            val active = selected == variant
            Row(Modifier.fillMaxWidth().padding(vertical = 3.dp).clip(RoundedCornerShape(7.dp)).background(if (active) selectedSurface else surface2).clickable { selected = variant }.padding(12.dp), verticalAlignment = Alignment.CenterVertically) {
                RadioButton(active, { selected = variant }, accessibilityLabel = "选择 ${variant.label}"); Spacer(Modifier.width(8.dp))
                Column(Modifier.weight(1f)) { Text(variant.label.ifBlank { variant.name.ifBlank { "媒体轨道" } }, color = ink, fontSize = 12.sp, fontWeight = FontWeight.SemiBold); Text(listOfNotNull(variant.height.takeIf { it > 0 }?.let { "${it}p" }, variant.bandwidth.takeIf { it > 0 }?.let { formatRate(it / 8) }, variant.kind.takeIf(String::isNotBlank)).joinToString(" · "), color = muted, fontSize = 10.sp) }
            }
        }
    }, actions = { DialogSecondary("取消", onDismiss); DialogPrimary("创建下载") { onCreate(selected) } })
}

private val harvestCategories = listOf(
    "all" to "全部",
    "video" to "视频",
    "audio" to "音频",
    "archive" to "压缩包",
    "document" to "文档",
    "program" to "程序",
    "playlist" to "清单",
    "torrent" to "种子",
    "other" to "其他",
)

@Composable
private fun HarvestChip(label: String, active: Boolean, enabled: Boolean = true, onClick: () -> Unit) {
    val foreground = when {
        !enabled -> faint
        active -> blue
        else -> muted
    }
    Surface(
        modifier = Modifier.height(28.dp).clip(RoundedCornerShape(7.dp)).clickable(enabled = enabled, onClick = onClick),
        color = if (active) selectedSurface else surface2,
        shape = RoundedCornerShape(7.dp),
        border = BorderStroke(1.dp, if (active) blue.copy(alpha = .55f) else border),
    ) {
        Box(Modifier.padding(horizontal = 9.dp), contentAlignment = Alignment.Center) {
            Text(label, color = foreground, fontSize = 10.sp, fontWeight = if (active) FontWeight.SemiBold else FontWeight.Normal)
        }
    }
}

@Composable
private fun HarvestResultDialog(
    signal: UiSignal.Harvest,
    initialReferer: String,
    defaultConcurrency: Long,
    probing: Boolean,
    onDismiss: () -> Unit,
    onProbe: (List<String>, String) -> Unit,
    onCreate: (HarvestCreateRequest) -> Unit,
) {
    var selected by remember(signal.url) { mutableStateOf(signal.links.map { it.url }.toSet()) }
    var category by remember(signal.url) { mutableStateOf("all") }
    var minimumBytes by remember(signal.url) { mutableLongStateOf(0) }
    var probeRequested by remember(signal.url) { mutableStateOf(false) }
    var referer by remember(signal.url) { mutableStateOf(initialReferer) }
    var concurrency by remember(signal.url) { mutableStateOf(defaultConcurrency.coerceIn(1, 64).toString()) }
    val counts = harvestFilterCounts(signal.links)
    val visible = visibleHarvestLinks(signal.links, category, minimumBytes)
    val selectedVisible = visible.filter { it.url in selected }
    val effectiveReferer = referer.trim().ifBlank { signal.url }
    val refererValid = runCatching { EnginePipeClient.normalizeHttpUrl(effectiveReferer) }.isSuccess
    val parsedConcurrency = concurrency.toLongOrNull()?.coerceIn(1, 64) ?: 0
    val probed = probeRequested && !probing

    WorkbenchDialog(
        onDismiss,
        "网页抓取结果",
        "从当前页面提取到 ${signal.links.size} 个可下载资源",
        800.dp,
        content = {
            DetailLine("来源页面", safeResourceLocation(signal.url))
            Spacer(Modifier.height(11.dp))
            if (signal.links.isEmpty()) {
                Text("页面未发现可下载的静态文件链接。", color = muted, fontSize = 12.sp)
                Text("这里只读取当前页面 HTML，不执行脚本，也不继续打开子页面。", color = faint, fontSize = 11.sp, modifier = Modifier.padding(top = 6.dp))
            } else {
                Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    harvestCategories.filter { (id, _) -> id == "all" || counts.getOrDefault(id, 0) > 0 }.forEach { (id, label) ->
                        HarvestChip("$label ${counts.getOrDefault(id, 0)}", category == id) { category = id }
                    }
                }
                Spacer(Modifier.height(9.dp))
                Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
                    HarvestChip("全选当前分类", false) { selected = selected + visible.map { it.url } }
                    HarvestChip("取消当前分类", false) { selected = selected - visible.map { it.url }.toSet() }
                    HarvestChip(if (probing) "读取大小中…" else "读取文件大小", false, !probing) {
                        probeRequested = true
                        onProbe(signal.links.map { it.url }, effectiveReferer)
                    }
                    HarvestChip("全部大小", minimumBytes == 0L) { minimumBytes = 0L }
                    HarvestChip("≥ 1 MB", minimumBytes == 1024L * 1024L, probed) { minimumBytes = 1024L * 1024L }
                    HarvestChip("≥ 10 MB", minimumBytes == 10L * 1024L * 1024L, probed) { minimumBytes = 10L * 1024L * 1024L }
                    Text("已选 ${selectedVisible.size} / ${visible.size}", color = muted, fontSize = 10.sp)
                }
                Spacer(Modifier.height(9.dp))
                Column(Modifier.fillMaxWidth().border(1.dp, border, RoundedCornerShape(7.dp)).clip(RoundedCornerShape(7.dp))) {
                    visible.forEachIndexed { index, item ->
                        Row(
                            Modifier.fillMaxWidth().clickable {
                                selected = if (item.url in selected) selected - item.url else selected + item.url
                            }.background(if (item.url in selected) selectedSurface.copy(alpha = .55f) else Color.Transparent).padding(horizontal = 9.dp, vertical = 7.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Checkbox(
                                item.url in selected,
                                { checked -> selected = if (checked) selected + item.url else selected - item.url },
                                accessibilityLabel = "选择 ${item.filename.ifBlank { item.url }}",
                            )
                            Spacer(Modifier.width(7.dp))
                            Column(Modifier.weight(1f)) {
                                Text(item.filename.ifBlank { item.url.substringAfterLast('/') }, color = ink, fontSize = 12.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                val details = buildList {
                                    add(harvestCategories.firstOrNull { it.first == item.category }?.second ?: "文件")
                                    item.extension.trimStart('.').takeIf(String::isNotBlank)?.let { add(".$it") }
                                    if (item.size > 0) add(formatBytes(item.size)) else if (probed) add("大小未知")
                                }
                                Text(details.joinToString(" · "), color = muted, fontSize = 10.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            }
                            WorkbenchTooltip(item.url) {
                                Icon(Icons.Outlined.Link, null, tint = faint, modifier = Modifier.size(16.dp))
                            }
                        }
                        if (index < visible.lastIndex) HorizontalDivider(color = border)
                    }
                }
                if (visible.isEmpty()) Text("当前筛选条件下没有资源。", color = muted, fontSize = 11.sp, modifier = Modifier.padding(top = 9.dp))
            }
            Spacer(Modifier.height(12.dp))
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Column(Modifier.weight(1f)) {
                    DialogLabel("Referer（可选）")
                    OutlinedTextField(referer, { referer = it }, Modifier.fillMaxWidth(), singleLine = true, shape = RoundedCornerShape(7.dp), isError = !refererValid)
                }
                Column(Modifier.width(120.dp)) {
                    DialogLabel("并发")
                    OutlinedTextField(concurrency, { concurrency = it.filter(Char::isDigit) }, Modifier.fillMaxWidth(), singleLine = true, shape = RoundedCornerShape(7.dp), isError = parsedConcurrency == 0L)
                }
            }
        },
        actions = {
            DialogSecondary("取消", onDismiss)
            DialogPrimary("创建 ${selectedVisible.size} 个下载", selectedVisible.isNotEmpty() && refererValid && parsedConcurrency > 0L && !probing) {
                onCreate(HarvestCreateRequest(selectedVisible.map { it.url }, effectiveReferer, parsedConcurrency))
            }
        },
    )
}

@Composable private fun MediaSourcePickerDialog(mode: String, onDismiss: () -> Unit, onChoose: (MediaSourceSelection) -> Unit) {
    var source by remember { mutableStateOf("local") }
    var url by remember { mutableStateOf("") }
    var title by remember { mutableStateOf("") }
    var error by remember { mutableStateOf("") }
    val verb = if (mode == "tvbox") "TVBox 推送" else "投屏"
    val validUrl = url.trim().let { it.startsWith("http://", true) || it.startsWith("https://", true) }
    WorkbenchDialog(onDismiss, "选择${verb}内容", "先选择媒体来源，再确认接收设备", 600.dp, content = {
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            listOf("local" to Triple(Icons.Outlined.VideoFile, "本机文件", "共享电脑中的视频或音频"), "url" to Triple(Icons.Outlined.Link, "媒体链接", "发送设备可直接访问的链接")).forEach { (id, item) ->
                val active = source == id
                Row(
                    Modifier.weight(1f).height(64.dp).clip(RoundedCornerShape(8.dp)).background(if (active) selectedSurface else surface2)
                        .border(1.dp, if (active) blue else border, RoundedCornerShape(8.dp)).clickable { source = id; error = "" }.padding(11.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Icon(item.first, null, tint = if (active) blue else muted, modifier = Modifier.size(20.dp)); Spacer(Modifier.width(10.dp))
                    Column { Text(item.second, color = ink, fontSize = 12.sp, fontWeight = FontWeight.SemiBold); Text(item.third, color = muted, fontSize = 10.sp, maxLines = 1) }
                }
            }
        }
        Spacer(Modifier.height(14.dp))
        if (source == "local") {
            Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(7.dp)).background(surface2).padding(12.dp), verticalAlignment = Alignment.Top) {
                Icon(Icons.Outlined.Info, null, tint = blue, modifier = Modifier.size(17.dp)); Spacer(Modifier.width(8.dp))
                Text("文件由下载引擎临时共享到局域网，支持电视端 Range 拖动；关闭主界面不会中断播放地址。", color = muted, fontSize = 10.sp, lineHeight = 15.sp)
            }
        } else {
            OutlinedTextField(url, { url = it; error = "" }, Modifier.fillMaxWidth(), label = { Text("媒体链接") }, placeholder = { Text("https://example.com/video.mp4") }, singleLine = true, shape = RoundedCornerShape(7.dp))
            Spacer(Modifier.height(8.dp))
            OutlinedTextField(title, { title = it }, Modifier.fillMaxWidth(), label = { Text("显示名称（可选）") }, singleLine = true, shape = RoundedCornerShape(7.dp))
            Text("需要登录、Cookie 或即将过期的链接可能无法被电视直接访问。", color = muted, fontSize = 10.sp, modifier = Modifier.padding(top = 7.dp))
        }
        if (error.isNotEmpty()) Text(error, color = Color(0xFFDC2626), fontSize = 11.sp, modifier = Modifier.padding(top = 9.dp))
    }, actions = {
        DialogSecondary("取消", onDismiss)
        if (source == "local") DialogPrimary("选择本机文件") {
            val path = chooseMediaPath()
            if (path != null) onChoose(MediaSourceSelection(path = path, title = java.io.File(path).name))
        } else DialogPrimary("继续选择设备", validUrl) {
            if (validUrl) onChoose(MediaSourceSelection(url = url.trim(), title = title.trim())) else error = "请输入有效的 HTTP(S) 媒体链接"
        }
    })
}

@Composable private fun DevicePickerDialog(
    signal: UiSignal.Devices,
    mode: String,
    source: MediaSourceSelection?,
    busy: Boolean,
    connecting: Boolean,
    preferredDeviceId: String,
    onDismiss: () -> Unit,
    onRescan: () -> Unit,
    onPublish: () -> Unit,
    onSelect: (CastDeviceDto) -> Unit,
) {
    val devices = if (mode == "tvbox") signal.devices.filter { it.serviceType.equals("tvbox", true) || it.id.startsWith("tvbox:") } else signal.devices.filterNot { it.serviceType.equals("tvbox", true) || it.id.startsWith("tvbox:") }
    val verb = if (mode == "tvbox") "TVBox 推送" else "投屏"
    var selected by remember(devices, preferredDeviceId) {
        mutableStateOf(devices.firstOrNull { it.id == preferredDeviceId })
    }
    WorkbenchDialog(onDismiss, "选择${verb}设备", if (mode == "tvbox") "自动搜索同一局域网内的 TVBox，发送前确认目标" else "自动搜索同一局域网内的 DLNA 和 Chromecast", 620.dp, dismissible = !connecting, content = {
        source?.let { media ->
            Surface(Modifier.fillMaxWidth(), color = selectedSurface, shape = RoundedCornerShape(8.dp), border = BorderStroke(1.dp, blue.copy(alpha = .35f))) {
                Row(Modifier.padding(horizontal = 12.dp, vertical = 10.dp), verticalAlignment = Alignment.CenterVertically) {
                    Icon(if (media.path.isNotBlank()) Icons.Outlined.VideoFile else Icons.Outlined.Link, null, tint = blue, modifier = Modifier.size(18.dp))
                    Spacer(Modifier.width(9.dp))
                    Column(Modifier.weight(1f)) {
                        Text(media.title.ifBlank { "待发送媒体" }, color = ink, fontSize = 11.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        Text(if (media.path.isNotBlank()) media.path else safeResourceLocation(media.url), color = muted, fontSize = 10.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    }
                }
            }
            Spacer(Modifier.height(10.dp))
        }
        when {
            busy -> repeat(3) { index ->
                Row(Modifier.fillMaxWidth().padding(vertical = 4.dp).clip(RoundedCornerShape(8.dp)).background(surface2).padding(14.dp), verticalAlignment = Alignment.CenterVertically) {
                    Surface(Modifier.size(34.dp), color = surface3, shape = RoundedCornerShape(7.dp)) {}
                    Spacer(Modifier.width(12.dp))
                    Column(Modifier.weight(1f)) {
                        Surface(Modifier.width((150 + index * 28).dp).height(11.dp), color = surface3, shape = RoundedCornerShape(4.dp)) {}
                        Spacer(Modifier.height(8.dp)); Surface(Modifier.width(210.dp).height(8.dp), color = surface3, shape = RoundedCornerShape(4.dp)) {}
                    }
                }
            }
            devices.isEmpty() -> Box(Modifier.fillMaxWidth().height(170.dp), contentAlignment = Alignment.Center) {
                Column(horizontalAlignment = Alignment.CenterHorizontally) {
                    Icon(Icons.Outlined.Cast, null, tint = faint, modifier = Modifier.size(38.dp))
                    Spacer(Modifier.height(11.dp)); Text("没有发现可用设备", color = ink, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
                    Text("确认电脑和电视连接到同一局域网，然后重新搜索。", color = muted, fontSize = 11.sp, modifier = Modifier.padding(top = 4.dp))
                }
            }
            else -> devices.forEach { device ->
                val active = selected?.id == device.id
                val protocol = when {
                    device.serviceType.equals("chromecast", true) || device.id.startsWith("chromecast:") -> "Chromecast"
                    device.serviceType.equals("tvbox", true) || device.id.startsWith("tvbox:") -> "TVBox"
                    else -> "DLNA"
                }
                val metadata = if (device.label.contains(protocol, ignoreCase = true)) device.location else listOf(protocol, device.location).filter(String::isNotBlank).joinToString(" · ")
                Row(
                    Modifier.fillMaxWidth().padding(vertical = 4.dp).clip(RoundedCornerShape(8.dp))
                        .background(if (active) selectedSurface else surface2)
                        .border(1.dp, if (active) blue else Color.Transparent, RoundedCornerShape(8.dp))
                        .clickable { selected = device }.padding(13.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Surface(Modifier.size(36.dp), color = if (active) blue else surface3, shape = RoundedCornerShape(7.dp)) { Box(contentAlignment = Alignment.Center) { Icon(Icons.Outlined.Tv, null, tint = if (active) Color.White else muted, modifier = Modifier.size(20.dp)) } }
                    Spacer(Modifier.width(12.dp)); Column(Modifier.weight(1f)) {
                        Text(device.label, color = ink, fontSize = 12.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        Text(metadata, color = muted, fontSize = 10.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    }
                    if (active) Icon(Icons.Outlined.CheckCircle, "已选择", tint = blue, modifier = Modifier.size(19.dp))
                }
            }
        }
        if (!busy) {
            Spacer(Modifier.height(10.dp)); Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(7.dp)).background(surface2).padding(10.dp), verticalAlignment = Alignment.CenterVertically) {
                Icon(Icons.Outlined.Info, null, tint = muted, modifier = Modifier.size(16.dp)); Spacer(Modifier.width(8.dp))
                Text(if (mode == "tvbox") "TVBox 需要开启接收服务；确认推送后会在电视端打开媒体。" else "设备无法接收时，可发布局域网播放地址并在电视播放器中打开。", color = muted, fontSize = 10.sp)
            }
        }
    }, actions = {
        DialogSecondary("取消", onDismiss)
        TextButton(onClick = onRescan, enabled = !busy && !connecting) { Icon(Icons.Outlined.Refresh, null, Modifier.size(16.dp)); Spacer(Modifier.width(5.dp)); Text("重新搜索", fontSize = 12.sp) }
        if (mode != "tvbox") DialogPrimary("局域网播放", !busy && !connecting, onPublish)
        DialogPrimary(if (connecting) "正在连接…" else if (mode == "tvbox") "确认推送" else "连接设备", selected != null && !busy && !connecting) { selected?.let(onSelect) }
    })
}

@Composable private fun DuplicateDialog(signal: UiSignal.Duplicate, onDismiss: () -> Unit, onConfirm: () -> Unit) = WorkbenchDialog(onDismiss, "发现重复任务", signal.message, 520.dp, content = {
    Text("已有任务可以${actionLabel(signal.action)}，继续将对现有任务执行该操作。", color = muted, fontSize = 12.sp)
}, actions = { DialogSecondary("取消", onDismiss); DialogPrimary(actionLabel(signal.action), onClick = onConfirm) })

@Composable private fun UpdateDialog(signal: UiSignal.Update, busy: Boolean, onDismiss: () -> Unit, onRelease: () -> Unit, onDownload: () -> Unit) = WorkbenchDialog(onDismiss, "发现新版本", "安全覆盖升级", 590.dp, dismissible = !busy, content = {
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        UpdateVersionCell("当前版本", signal.current, Modifier.weight(1f))
        Icon(Icons.AutoMirrored.Outlined.ArrowForward, null, tint = muted, modifier = Modifier.align(Alignment.CenterVertically).size(18.dp))
        UpdateVersionCell("可用版本", signal.latest, Modifier.weight(1f), emphasized = true)
    }
    Spacer(Modifier.height(12.dp))
    if (signal.notes.isNotBlank()) Text(signal.notes, color = ink, fontSize = 12.sp, lineHeight = 19.sp, maxLines = 7, overflow = TextOverflow.Ellipsis)
    Spacer(Modifier.height(12.dp))
    Surface(Modifier.fillMaxWidth(), color = surface2, shape = RoundedCornerShape(8.dp)) {
        Column(Modifier.padding(horizontal = 13.dp, vertical = 11.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Text(signal.installerName.ifBlank { "未找到可自动安装的 Windows x64 MSI" }, color = ink, fontSize = 11.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(if (signal.installerSize > 0) "${formatBytes(signal.installerSize)} · SHA-256 ${if (signal.sha256Verified) "发布方摘要已确认" else "摘要缺失"}" else "自动升级已停用，请从发布页人工核验", color = if (signal.sha256Verified) successColor else warningColor, fontSize = 10.sp)
        }
    }
    Spacer(Modifier.height(12.dp)); Text("安装程序将覆盖当前版本，配置、任务数据库和下载文件会保留。开始安装前会安全暂停活动任务并保存断点；升级不会自动重启 Windows。", color = muted, fontSize = 11.sp, lineHeight = 17.sp)
}, actions = {
    DialogSecondary("稍后", onDismiss)
    if (signal.releaseUrl.isNotBlank()) DialogSecondary("查看发布页", onRelease)
    DialogPrimary(if (busy) "下载并校验中…" else "下载更新", !busy && signal.installerSize > 0 && signal.sha256Verified && signal.installerName.endsWith(".msi", true), onDownload)
})

@Composable private fun UpdatePreparedDialog(signal: UiSignal.UpdatePrepared, busy: Boolean, onDismiss: () -> Unit, onInstall: () -> Unit) = WorkbenchDialog(
    onDismiss = onDismiss,
    title = "更新已准备",
    description = "版本 ${signal.latest} 已完成完整性与产品身份校验",
    width = 590.dp,
    dismissible = !busy,
    content = {
        Surface(Modifier.fillMaxWidth(), color = surface2, shape = RoundedCornerShape(8.dp)) {
            Column(Modifier.padding(horizontal = 13.dp, vertical = 11.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                Row(Modifier.fillMaxWidth()) {
                    Text("产品", color = muted, fontSize = 10.sp, modifier = Modifier.width(82.dp))
                    Text("${signal.productName} ${signal.productVersion}", color = ink, fontSize = 11.sp, fontWeight = FontWeight.Medium)
                }
                Row(Modifier.fillMaxWidth()) {
                    Text("安装包", color = muted, fontSize = 10.sp, modifier = Modifier.width(82.dp))
                    Text(signal.installerPath.substringAfterLast('/').substringAfterLast('\\'), color = ink, fontSize = 11.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
                Row(Modifier.fillMaxWidth()) {
                    Text("SHA-256", color = muted, fontSize = 10.sp, modifier = Modifier.width(82.dp))
                    Text(signal.sha256.take(16) + "…" + signal.sha256.takeLast(8), color = successColor, fontSize = 10.sp)
                }
            }
        }
        Spacer(Modifier.height(12.dp))
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.Top) {
            Icon(Icons.Outlined.RestartAlt, null, tint = blue, modifier = Modifier.size(18.dp))
            Spacer(Modifier.width(9.dp))
            Text("开始后，下载引擎会暂停活动任务并保存断点，工作台和临时窗口随后关闭。独立更新助手会等待文件释放、执行覆盖安装并重新打开 HLS Downloader。", color = muted, fontSize = 11.sp, lineHeight = 17.sp)
        }
    },
    actions = {
        DialogSecondary("稍后安装", onDismiss)
        DialogPrimary(if (busy) "正在保存断点…" else "立即安装并重启", !busy, onInstall)
    },
)

@Composable private fun UpdateVersionCell(label: String, value: String, modifier: Modifier = Modifier, emphasized: Boolean = false) {
    Column(modifier.clip(RoundedCornerShape(8.dp)).background(if (emphasized) selectedSurface else surface2).padding(horizontal = 13.dp, vertical = 11.dp)) {
        Text(label, color = muted, fontSize = 10.sp)
        Text(value.ifBlank { "-" }, color = if (emphasized) blue else ink, fontSize = 16.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(top = 2.dp))
    }
}

@Composable private fun DestructiveConfirmDialog(request: DestructiveRequest, onDismiss: () -> Unit, onConfirm: () -> Unit) = WorkbenchDialog(
    onDismiss, if (request.action == "delete_files") "删除任务和文件" else "删除任务", "此操作将影响 ${request.taskIds.size} 个任务", 520.dp,
    content = {
        Surface(color = Color(0xFFFFF1F0), shape = RoundedCornerShape(7.dp), modifier = Modifier.fillMaxWidth()) {
            Row(Modifier.padding(13.dp), verticalAlignment = Alignment.Top) { Icon(Icons.Outlined.WarningAmber, null, tint = Color(0xFFB42318)); Spacer(Modifier.width(10.dp)); Text(if (request.action == "delete_files") "任务记录、已下载文件和过程文件都会删除。" else "只删除任务记录，已完成文件将保留。", color = Color(0xFF7A271A), fontSize = 12.sp, lineHeight = 18.sp) }
        }
    }, actions = { DialogSecondary("取消", onDismiss); Button(onClick = onConfirm, colors = ButtonDefaults.buttonColors(containerColor = Color(0xFFB42318), contentColor = Color.White), shape = RoundedCornerShape(7.dp)) { Text("确认删除", fontSize = 12.sp, fontWeight = FontWeight.SemiBold) } },
)

@Composable private fun PowerActionDialog(signal: UiSignal.PowerPending, onCancel: () -> Unit, onConfirm: () -> Unit) = WorkbenchDialog(
    onDismiss = onCancel,
    title = "下载完成",
    description = "完成后操作确认",
    width = 480.dp,
    content = {
        val action = when (signal.action) { "shutdown" -> "关机"; "sleep" -> "进入睡眠"; "hibernate" -> "进入休眠"; else -> "执行系统操作" }
        Text("${signal.title.ifBlank { "下载任务" }} 已完成。${signal.delaySeconds} 秒后将$action。", color = ink, fontSize = 12.sp, lineHeight = 19.sp)
        Text("可以立即执行，或取消本次操作。", color = muted, fontSize = 11.sp, modifier = Modifier.padding(top = 8.dp))
    },
    actions = { DialogSecondary("取消操作", onCancel); DialogPrimary("立即执行", onClick = onConfirm) },
)

@Composable private fun PlayerSessionHud(signal: UiSignal.Player, busy: Boolean, onAction: (String) -> Unit) {
    val speeds = listOf(1.0, 1.25, 1.5, 2.0)
    val nextSpeed = speeds[(speeds.indexOfFirst { kotlin.math.abs(it - signal.speed) < 0.01 }.takeIf { it >= 0 } ?: 0).let { (it + 1) % speeds.size }]
    var scrubPosition by remember(signal.positionSeconds) { mutableFloatStateOf(signal.positionSeconds.toFloat()) }
    var audioMenu by remember { mutableStateOf(false) }
    var subtitleMenu by remember { mutableStateOf(false) }
    Popup(alignment = Alignment.BottomEnd, offset = androidx.compose.ui.unit.IntOffset(-18, -46), properties = PopupProperties(focusable = false)) {
        Surface(color = dialogSurface, shape = RoundedCornerShape(9.dp), shadowElevation = 10.dp, border = BorderStroke(1.dp, border), modifier = Modifier.width(430.dp)) {
            Column(Modifier.padding(14.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Surface(Modifier.size(34.dp), color = selectedSurface, shape = RoundedCornerShape(7.dp)) { Box(contentAlignment = Alignment.Center) { Icon(Icons.Outlined.SmartDisplay, null, tint = blue, modifier = Modifier.size(19.dp)) } }
                    Spacer(Modifier.width(10.dp)); Column(Modifier.weight(1f)) {
                        Text(signal.title.ifBlank { "正在播放" }, color = ink, fontSize = 12.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        Text("本机播放器 · ${playerStatusLabel(signal.status)} · ${formatPlayerSpeed(signal.speed)}", color = muted, fontSize = 10.sp)
                    }
                    if (busy) CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp, color = blue)
                    IconButton(onClick = { if (!busy) onAction("stop") }, enabled = !busy) { Icon(Icons.Outlined.Stop, "停止播放", tint = muted) }
                }
                Spacer(Modifier.height(8.dp)); HorizontalDivider(color = border); Spacer(Modifier.height(7.dp))
                if (signal.positionAvailable && signal.durationSeconds > 0.0) {
                    Slider(
                        value = scrubPosition.coerceIn(0f, signal.durationSeconds.toFloat()),
                        onValueChange = { if (!busy) scrubPosition = it },
                        onValueChangeFinished = { if (!busy) onAction("preview:${(scrubPosition / signal.durationSeconds * 100.0).coerceIn(0.0, 100.0)}") },
                        valueRange = 0f..signal.durationSeconds.toFloat(),
                        modifier = Modifier.fillMaxWidth().height(26.dp),
                        colors = SliderDefaults.colors(thumbColor = blue, activeTrackColor = blue, inactiveTrackColor = surface3),
                        accessibilityLabel = "播放位置",
                    )
                    Row(Modifier.fillMaxWidth()) {
                        Text(formatClock(scrubPosition.toLong()), color = muted, fontSize = 10.sp)
                        Spacer(Modifier.weight(1f))
                        Text(formatClock(signal.durationSeconds.toLong()), color = muted, fontSize = 10.sp)
                    }
                    if (signal.audioTracks > 0 || signal.subtitleTracks > 0) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Text("音轨 ${signal.audioTracks} · 字幕 ${signal.subtitleTracks}", color = muted, fontSize = 10.sp)
                            Spacer(Modifier.weight(1f))
                            if (signal.audioTracks > 0) Box {
                                TextButton(onClick = { audioMenu = true }, enabled = !busy, contentPadding = PaddingValues(horizontal = 5.dp, vertical = 0.dp), modifier = Modifier.height(25.dp)) { Text("选择音轨", color = blue, fontSize = 10.sp) }
                                DropdownMenu(expanded = audioMenu, onDismissRequest = { audioMenu = false }, shape = RoundedCornerShape(7.dp), containerColor = dialogSurface) {
                                    (1..signal.audioTracks).forEach { id -> DropdownMenuItem(text = { Text("音轨 $id", color = ink, fontSize = 11.sp) }, onClick = { audioMenu = false; onAction("audio:$id") }) }
                                }
                            }
                            if (signal.subtitleTracks > 0) Box {
                                TextButton(onClick = { subtitleMenu = true }, enabled = !busy, contentPadding = PaddingValues(horizontal = 5.dp, vertical = 0.dp), modifier = Modifier.height(25.dp)) { Text("选择字幕", color = blue, fontSize = 10.sp) }
                                DropdownMenu(expanded = subtitleMenu, onDismissRequest = { subtitleMenu = false }, shape = RoundedCornerShape(7.dp), containerColor = dialogSurface) {
                                    (1..signal.subtitleTracks).forEach { id -> DropdownMenuItem(text = { Text("字幕 $id", color = ink, fontSize = 11.sp) }, onClick = { subtitleMenu = false; onAction("subtitle:$id") }) }
                                }
                            }
                        }
                    }
                    Spacer(Modifier.height(4.dp))
                }
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceEvenly, verticalAlignment = Alignment.CenterVertically) {
                    ToolbarIcon(Icons.AutoMirrored.Outlined.VolumeDown, "降低音量") { if (!busy) onAction("vol_down") }
                    ToolbarIcon(Icons.Outlined.Replay10, "后退 10 秒") { if (!busy) onAction("seek_back") }
                    ToolbarIcon(if (signal.paused) Icons.Outlined.PlayArrow else Icons.Outlined.Pause, if (signal.paused) "继续播放" else "暂停") { if (!busy) onAction(if (signal.paused) "resume" else "pause") }
                    ToolbarIcon(Icons.Outlined.Forward10, "前进 10 秒") { if (!busy) onAction("seek_fwd") }
                    ToolbarIcon(Icons.AutoMirrored.Outlined.VolumeUp, "提高音量") { if (!busy) onAction("vol_up") }
                    TextButton(onClick = { if (!busy) onAction("speed:$nextSpeed") }, enabled = !busy, contentPadding = PaddingValues(horizontal = 7.dp), modifier = Modifier.height(34.dp)) { Text(formatPlayerSpeed(signal.speed), color = blue, fontSize = 10.sp, fontWeight = FontWeight.SemiBold) }
                    ToolbarIcon(Icons.Outlined.PictureInPictureAlt, "画中画") { if (!busy) onAction("pip") }
                    ToolbarIcon(Icons.Outlined.Fullscreen, "全屏") { if (!busy) onAction("fullscreen") }
                }
            }
        }
    }
}

private fun formatPlayerSpeed(speed: Double) = if (kotlin.math.abs(speed - speed.toInt()) < 0.01) "${speed.toInt()}x" else "${"%.2f".format(speed).trimEnd('0')}x"
private fun playerStatusLabel(status: String) = when (status.uppercase()) { "PAUSED" -> "已暂停"; "FULLSCREEN" -> "全屏播放"; "PIP" -> "画中画"; "STOPPED" -> "已停止"; else -> "正在播放" }

@Composable private fun CastSessionHud(signal: UiSignal.Cast, task: DownloadTask?, busy: Boolean, raised: Boolean, onCopy: (String) -> Unit, onAction: (String, Long) -> Unit) {
    var scrubPosition by remember(signal.positionSeconds) { mutableFloatStateOf(signal.positionSeconds.toFloat()) }
    val offline = signal.status.equals("OFFLINE", true)
    val controllable = signal.supportedActions.contains("play") && !offline
    Popup(alignment = Alignment.BottomEnd, offset = androidx.compose.ui.unit.IntOffset(-18, if (raised) -190 else -46), properties = PopupProperties(focusable = false)) {
        Surface(color = dialogSurface, shape = RoundedCornerShape(9.dp), shadowElevation = 10.dp, border = BorderStroke(1.dp, border), modifier = Modifier.width(420.dp)) {
            Column(Modifier.padding(14.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Surface(Modifier.size(34.dp), color = selectedSurface, shape = RoundedCornerShape(7.dp)) { Box(contentAlignment = Alignment.Center) { Icon(Icons.Outlined.CastConnected, null, tint = blue, modifier = Modifier.size(19.dp)) } }
                    Spacer(Modifier.width(10.dp)); Column(Modifier.weight(1f)) {
                        Text(signal.title.ifBlank { task?.filename ?: "正在投屏" }, color = ink, fontSize = 12.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        val protocol = castProtocolLabel(signal.deviceKind)
                        Text(listOf(signal.device, protocol.takeUnless { signal.device.contains(it, ignoreCase = true) }.orEmpty(), castStatusLabel(signal.status)).filter(String::isNotBlank).joinToString(" · "), color = muted, fontSize = 10.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    }
                    if (offline) IconButton(onClick = { onAction("status", 0) }, enabled = !busy) { Icon(Icons.Outlined.Refresh, "重新连接", tint = blue) }
                    IconButton(onClick = { onAction("stop", 0) }, enabled = !busy) { Icon(Icons.Outlined.Stop, "停止投屏", tint = muted) }
                }
                if (controllable) {
                    Spacer(Modifier.height(9.dp))
                    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.Center, verticalAlignment = Alignment.CenterVertically) {
                        ToolbarIcon(Icons.Outlined.Replay10, "后退 10 秒") { onAction("seek", -10) }
                        ToolbarIcon(if (signal.playing && !signal.paused) Icons.Outlined.Pause else Icons.Outlined.PlayArrow, if (signal.playing && !signal.paused) "暂停" else "播放") { onAction(if (signal.playing && !signal.paused) "pause" else "play", 0) }
                        ToolbarIcon(Icons.Outlined.Forward10, "前进 10 秒") { onAction("seek", 10) }
                    }
                    if (signal.positionAvailable && signal.durationSeconds > 0) {
                        Slider(value = scrubPosition.coerceIn(0f, signal.durationSeconds.toFloat()), onValueChange = { scrubPosition = it }, onValueChangeFinished = { onAction("seek_to", scrubPosition.toLong()) }, valueRange = 0f..signal.durationSeconds.toFloat(), modifier = Modifier.fillMaxWidth().height(28.dp), colors = SliderDefaults.colors(thumbColor = blue, activeTrackColor = blue, inactiveTrackColor = surface3, activeTickColor = blue, inactiveTickColor = surface3), accessibilityLabel = "投屏播放位置")
                        Row(Modifier.fillMaxWidth()) { Text(formatClock(scrubPosition.toLong()), color = muted, fontSize = 10.sp); Spacer(Modifier.weight(1f)); Text(formatClock(signal.durationSeconds), color = muted, fontSize = 10.sp) }
                    }
                } else {
                    Spacer(Modifier.height(9.dp)); Text(if (offline) "与接收设备的连接已中断。检查电视和局域网后点击重新连接。" else if (signal.deviceKind == "tvbox") "已推送到 TVBox。此类设备没有统一的远程控制协议，请在电视端操作。" else "局域网播放地址已发布，请在接收设备中控制播放。", color = if (offline) Color(0xFFDC2626) else muted, fontSize = 10.sp, lineHeight = 15.sp)
                }
                if (signal.deviceKind == "lan" && signal.mediaUrl.isNotBlank()) {
                    Spacer(Modifier.height(10.dp))
                    Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(7.dp)).background(surface2).border(1.dp, border, RoundedCornerShape(7.dp)).padding(start = 10.dp, end = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                        Text(signal.mediaUrl, color = muted, fontSize = 10.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                        IconButton(onClick = { onCopy(signal.mediaUrl) }, modifier = Modifier.size(32.dp)) { Icon(Icons.Outlined.ContentCopy, "复制播放地址", tint = blue, modifier = Modifier.size(16.dp)) }
                    }
                }
                task?.let {
                    Spacer(Modifier.height(10.dp)); HorizontalDivider(color = border); Spacer(Modifier.height(9.dp))
                    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) { Text("下载进度", color = muted, fontSize = 10.sp); Spacer(Modifier.weight(1f)); Text("${(it.progress * 100).toInt()}% · ${it.speed}", color = muted, fontSize = 10.sp) }
                    Spacer(Modifier.height(5.dp)); LinearProgressIndicator(progress = { it.progress }, modifier = Modifier.fillMaxWidth().height(4.dp).clip(RoundedCornerShape(2.dp)), color = blue, trackColor = surface3)
                }
            }
        }
    }
}

private fun formatClock(seconds: Long): String {
    val value = seconds.coerceAtLeast(0)
    return if (value >= 3600) "%d:%02d:%02d".format(value / 3600, (value / 60) % 60, value % 60) else "%02d:%02d".format(value / 60, value % 60)
}

private fun castProtocolLabel(kind: String) = when (kind.lowercase()) {
    "chromecast" -> "Chromecast"
    "tvbox" -> "TVBox"
    "dlna" -> "DLNA"
    "lan" -> "局域网播放"
    else -> kind
}

private fun castStatusLabel(status: String) = when (status.uppercase()) {
    "PLAYING" -> "正在播放"
    "PAUSED", "PAUSED_PLAYBACK" -> "已暂停"
    "BUFFERING", "TRANSITIONING" -> "正在缓冲"
    "IDLE", "STOPPED" -> "已停止"
    "PUBLISHED" -> "已发布"
    "UNKNOWN" -> "状态未知"
    "OFFLINE" -> "连接中断"
    else -> status
}

@Composable private fun BrowserHandoffDialog(
    offer: HandoffOfferDto,
    settings: EngineSettingsDto,
    duplicate: DownloadTask?,
    pendingCount: Int,
    busy: Boolean,
    onAccept: (HandoffDecision) -> Unit,
    onReject: (Boolean) -> Unit,
) {
    var filename by remember(offer.handoffId) { mutableStateOf(offer.filename.ifBlank { offer.title.ifBlank { "download" } }) }
    var category by remember(offer.handoffId) { mutableStateOf(handoffCategory(offer)) }
    fun directoryFor(value: TaskCategory) = when (value) {
        TaskCategory.MEDIA -> settings.categoryDirMedia
        TaskCategory.PROGRAM -> settings.categoryDirProgram
        TaskCategory.ARCHIVE -> settings.categoryDirArchive
        TaskCategory.OTHER -> settings.categoryDirOther
    }.ifBlank { settings.downloadDirectory }
    var directory by remember(offer.handoffId, settings) { mutableStateOf(directoryFor(category)) }
    var rememberDirectory by remember(offer.handoffId) { mutableStateOf(true) }
    var suppressSiteKind by remember(offer.handoffId) { mutableStateOf(false) }
    val validFilename = runCatching { EnginePipeClient.normalizeHandoffFilename(filename) }.isSuccess
    val source = offer.sourcePageUrl.ifBlank { offer.url }
    val sourceHost = runCatching { URI(source).host.orEmpty() }.getOrDefault("")
    val downloadHost = runCatching { URI(offer.url).host.orEmpty() }.getOrDefault("")
    val extension = filename.substringAfterLast('.', "").lowercase().takeIf { it.isNotBlank() }?.let { ".$it" } ?: "未知后缀"
    val accept = {
        if (validFilename && !busy) onAccept(HandoffDecision(filename.trim(), directory.trim(), category, rememberDirectory))
    }
    WorkbenchDialog(
        onDismiss = { if (!busy) onReject(false) },
        title = "浏览器下载",
        description = if (pendingCount > 1) "确认后加入下载队列 · 后面还有 ${pendingCount - 1} 个请求" else "确认保存位置后加入下载队列",
        width = 650.dp,
        dismissible = !busy,
        content = {
            Column(
                Modifier.fillMaxWidth().onPreviewKeyEvent { event ->
                    if (event.type != KeyEventType.KeyDown || busy) return@onPreviewKeyEvent false
                    when (event.key) {
                        Key.Enter -> { accept(); true }
                        Key.Escape -> { onReject(false); true }
                        else -> false
                    }
                },
            ) {
                Surface(Modifier.fillMaxWidth(), color = selectedSurface, shape = RoundedCornerShape(8.dp)) {
                    Row(Modifier.padding(14.dp), verticalAlignment = Alignment.CenterVertically) {
                        Icon(Icons.Outlined.Downloading, "下载文件", tint = blue, modifier = Modifier.size(26.dp))
                        Spacer(Modifier.width(11.dp))
                        Column(Modifier.weight(1f)) {
                            Text(filename.ifBlank { offer.title.ifBlank { "新下载" } }, color = ink, fontSize = 14.sp, fontWeight = FontWeight.SemiBold, maxLines = 2, overflow = TextOverflow.Ellipsis)
                            Text(
                                listOf(resourceKindLabel(offer.resourceKind), extension, offer.mimeType.takeIf { it.isNotBlank() }, if (offer.size > 0) formatBytes(offer.size) else "大小未知").filterNotNull().joinToString(" · "),
                                color = muted,
                                fontSize = 11.sp,
                                modifier = Modifier.padding(top = 3.dp),
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                            )
                            if (downloadHost.isNotBlank()) Text(downloadHost, color = faint, fontSize = 10.sp, modifier = Modifier.padding(top = 2.dp), maxLines = 1, overflow = TextOverflow.Ellipsis)
                        }
                    }
                }
                duplicate?.let {
                    Spacer(Modifier.height(9.dp))
                    Surface(Modifier.fillMaxWidth(), color = Color(0xFFFFF7E8), shape = RoundedCornerShape(7.dp)) {
                        Row(Modifier.padding(horizontal = 11.dp, vertical = 9.dp), verticalAlignment = Alignment.CenterVertically) {
                            Icon(Icons.Outlined.WarningAmber, "重复任务", tint = Color(0xFFD97706), modifier = Modifier.size(17.dp))
                            Spacer(Modifier.width(8.dp))
                            Text("已有同一地址的任务：${it.filename}（${it.status}）", color = ink, fontSize = 11.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        }
                    }
                }
                Spacer(Modifier.height(13.dp))
                DialogLabel("文件名")
                OutlinedTextField(filename, { filename = it }, Modifier.fillMaxWidth(), enabled = !busy, singleLine = true, isError = filename.isNotBlank() && !validFilename, shape = RoundedCornerShape(7.dp), supportingText = { if (filename.isNotBlank() && !validFilename) Text("文件名不能包含路径或控制字符", fontSize = 10.sp) })
                Spacer(Modifier.height(7.dp))
                DialogLabel("分类")
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(7.dp)) {
                    TaskCategory.entries.forEach { choice ->
                        Button(
                            onClick = { category = choice; directory = directoryFor(choice) },
                            modifier = Modifier.weight(1f),
                            enabled = !busy,
                            border = BorderStroke(1.dp, if (category == choice) blue else border),
                            colors = ButtonDefaults.buttonColors(if (category == choice) selectedSurface else rail, if (category == choice) blue else ink),
                        ) { Text(choice.label, fontSize = 11.sp, fontWeight = if (category == choice) FontWeight.SemiBold else FontWeight.Normal) }
                    }
                }
                Spacer(Modifier.height(10.dp))
                DialogLabel("保存到")
                Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                    OutlinedTextField(directory, { directory = it }, Modifier.weight(1f), enabled = !busy, singleLine = true, shape = RoundedCornerShape(7.dp), placeholder = { Text("使用下载引擎默认目录") })
                    Spacer(Modifier.width(7.dp))
                    Button(onClick = { chooseDirectory(directory, "选择下载保存位置")?.let { directory = it } }, enabled = !busy, colors = ButtonDefaults.buttonColors(rail, ink), border = BorderStroke(1.dp, border)) {
                        Icon(Icons.Outlined.FolderOpen, "选择保存文件夹", modifier = Modifier.size(17.dp))
                        Spacer(Modifier.width(6.dp)); Text("选择", fontSize = 11.sp)
                    }
                }
                Row(Modifier.fillMaxWidth().padding(top = 7.dp), verticalAlignment = Alignment.CenterVertically) {
                    Checkbox(rememberDirectory, { rememberDirectory = it }, accessibilityLabel = "记住此分类的保存位置")
                    Spacer(Modifier.width(8.dp)); Text("记住“${category.label}”文件的保存位置", color = ink, fontSize = 11.sp)
                }
                Spacer(Modifier.height(11.dp))
                Surface(Modifier.fillMaxWidth(), color = dialogSurface, shape = RoundedCornerShape(8.dp), border = BorderStroke(1.dp, border)) {
                    Column(Modifier.padding(11.dp), verticalArrangement = Arrangement.spacedBy(5.dp)) {
                        Row(Modifier.fillMaxWidth()) {
                            Icon(Icons.Outlined.GppGood, "请求上下文已继承", tint = Color(0xFF16A34A), modifier = Modifier.size(16.dp))
                            Spacer(Modifier.width(7.dp)); Text("网站请求上下文", color = ink, fontSize = 11.sp, fontWeight = FontWeight.SemiBold)
                            Spacer(Modifier.weight(1f)); Text("由下载引擎安全保管", color = Color(0xFF15803D), fontSize = 10.sp)
                        }
                        DetailLine("来源网页", sourceHost.ifBlank { "未捕获" })
                        DetailLine("来源地址", safeResourceLocation(source))
                        DetailLine("下载地址", safeResourceLocation(offer.url))
                        Text("支持沿用 Referer、Origin、User-Agent、Cookie 与 Authorization；敏感值只由下载引擎保管。", color = faint, fontSize = 10.sp, lineHeight = 15.sp)
                    }
                }
                if (sourceHost.isNotBlank()) Row(Modifier.fillMaxWidth().padding(top = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                    Checkbox(suppressSiteKind, { suppressSiteKind = it }, accessibilityLabel = "以后不再提示此网站的同类资源")
                    Spacer(Modifier.width(8.dp)); Text("以后不再自动提示 $sourceHost 的${resourceKindLabel(offer.resourceKind)}", color = ink, fontSize = 11.sp)
                }
            }
        },
        actions = {
            DialogSecondary("保留浏览器下载") { if (!busy) onReject(suppressSiteKind) }
            DialogPrimary(if (busy) "处理中…" else "确认下载", enabled = validFilename && !busy, onClick = accept)
        },
    )
}

private fun handoffCategory(offer: HandoffOfferDto): TaskCategory {
    if (offer.resourceKind.lowercase() in setOf("hls", "dash", "live", "media")) return TaskCategory.MEDIA
    return when (offer.filename.substringAfterLast('.', "").lowercase()) {
        "mp4", "mkv", "webm", "mov", "avi", "m4v", "ts", "mp3", "m4a", "flac", "wav", "jpg", "png", "gif", "webp" -> TaskCategory.MEDIA
        "exe", "msi", "msix", "appx", "bat", "cmd" -> TaskCategory.PROGRAM
        "zip", "7z", "rar", "tar", "gz", "bz2", "xz", "iso" -> TaskCategory.ARCHIVE
        else -> TaskCategory.OTHER
    }
}

private fun canonicalHandoffUrl(value: String) = value.substringBefore('#').trimEnd('/')

private fun resourceKindLabel(kind: String) = when (kind.lowercase()) {
    "hls" -> "HLS 媒体"
    "dash" -> "DASH 媒体"
    "live" -> "直播流"
    "torrent" -> "BT 任务"
    "ftp" -> "FTP 文件"
    "sftp" -> "SFTP 文件"
    else -> "文件"
}
