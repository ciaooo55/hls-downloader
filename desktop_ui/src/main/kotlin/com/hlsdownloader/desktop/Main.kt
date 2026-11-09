package com.hlsdownloader.desktop

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.MutableTransitionState
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.animation.slideInVertically
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
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.gestures.scrollBy
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.HorizontalScrollbar
import androidx.compose.foundation.VerticalScrollbar
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsHoveredAsState
import androidx.compose.foundation.interaction.collectIsFocusedAsState
import androidx.compose.foundation.interaction.collectIsPressedAsState
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
import com.hlsdownloader.desktop.icons.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.ExperimentalComposeUiApi
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.graphicsLayer
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
import androidx.compose.ui.input.pointer.PointerIcon
import androidx.compose.ui.input.pointer.pointerHoverIcon
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalViewConfiguration
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.paneTitle
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInWindow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.IntRect
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.window.Window
import androidx.compose.ui.window.WindowPlacement
import androidx.compose.ui.window.WindowScope
import androidx.compose.ui.window.Popup
import androidx.compose.ui.window.PopupPositionProvider
import androidx.compose.ui.window.PopupProperties
import androidx.compose.ui.window.application
import androidx.compose.ui.window.WindowPosition
import androidx.compose.ui.window.rememberWindowState
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.delay
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.coroutines.launch
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.sync.Semaphore
import kotlinx.coroutines.sync.withPermit
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import java.awt.event.ComponentAdapter
import java.awt.event.ComponentEvent
import java.awt.geom.RoundRectangle2D
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
import java.util.UUID
import javax.swing.JFileChooser
import javax.swing.filechooser.FileNameExtensionFilter
import kotlin.math.roundToInt
import org.jetbrains.skia.Image as SkiaImage

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
internal data class HandoffDecision(
    val filename: String,
    val directory: String,
    val category: TaskCategory,
    val rememberDirectory: Boolean,
)
private data class NoticeHistoryEntry(val at: Long, val level: String, val message: String)
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
    }}

/**
 * 任务表的键盘命令。整张表的键盘模型（Ctrl+A 全选 / Esc 清选 / Delete 删除 /
 * Enter 详情 / 上下键移动）原先写在 `onPreviewKeyEvent` 的 lambda 里：
 * 合成期才存在，**没法单测**，而它又是本产品里唯一成体系的表格键盘交互。
 *
 * 抽成纯函数后每个分支都能被真实断言。"到真机上按一遍"在这里不可行——
 * 本机 Xvfb 没有窗口管理器，Robot 的按键到不了任何窗口（已用纯 AWT 探针证明），
 * 所以剩下唯一可靠的验证手段就是把映射本身变成可测代码。
 *
 * 返回 null 表示"这个键不属于本表"，由调用方交还给焦点链上游。
 */
internal enum class TaskTableKeyCommand { SelectAll, ClearSelection, DeleteSelection, ActivateSelection, MoveSelection }

internal fun resolveTaskTableKeyCommand(key: Key, ctrl: Boolean, selectedCount: Int, taskCount: Int): TaskTableKeyCommand? = when {
    // Ctrl+A 不判断空列表：与原有行为一致——0 行时也消费该键并清空选择/锚点。
    ctrl && key == Key.A -> TaskTableKeyCommand.SelectAll
    key == Key.Escape && selectedCount > 0 -> TaskTableKeyCommand.ClearSelection
    key == Key.Delete && selectedCount > 0 -> TaskTableKeyCommand.DeleteSelection
    key == Key.Enter && selectedCount > 0 -> TaskTableKeyCommand.ActivateSelection
    (key == Key.DirectionDown || key == Key.DirectionUp) && taskCount > 0 -> TaskTableKeyCommand.MoveSelection
    else -> null
}

/**
 * `/state` 的三个镜子：当前选择、当前筛选、分类/队列选中项。
 *
 * 折叠侧栏只剩图标后，"点得中、点得对"光看截图证明不了，所以校验脚本靠这几个字段断言；
 * 侧栏同理。它们必须**跟着写入方**更新，而不是跟着"调用方作用域何时恰好重组"更新。
 *
 * 这里曾经是三个 `SideEffect`，直接挂在 `AppShell` 里，结果 `/state` 的
 * `selectedCount` 永远停在 0：`selected` 只在内层的 `TaskTable(...)` 调用点被读取，
 * 外层作用域不会因此失效，而 SideEffect 只在包含它的作用域重新执行时才触发——
 * Linux 上用 UI 测试 API 实测复现（点中任务行 1 秒后仍报 selected=0，而行已高亮；
 * 追踪日志里 `onSelection` 以 size=1 被调用，却没有后续的 SideEffect）。
 *
 * `LaunchedEffect` 的 key 在**组合期**读取，会让所在作用域按预期失效，
 * 因此镜像与选择/筛选的真实状态一致。key 里带上 `visibleCount`：
 * 可见任务集变化后选择与可见行的对应关系要重算（否则筛选后残留旧 id）。
 */
@Composable
internal fun UiTestStateMirror(selected: Set<String>, visibleCount: Int, filterLabel: String, categoryLabel: String, selectedQueueId: String?) {
    LaunchedEffect(selected, visibleCount) { UiTestState.updateSelection(selected) }
    LaunchedEffect(filterLabel, categoryLabel, selectedQueueId) {
        UiTestState.updateFilter(filterLabel)
        UiTestState.updateSidebarSelection(categoryLabel, selectedQueueId ?: "")
    }
}

/**
 * 可见任务集变化后，选择集该如何收敛。
 *
 * 为什么需要它：批量操作的目标是 `tasks.filter { it.id in selected }`（`TaskRow`
 * 的菜单与快捷键都走这条），而表头的"N 已选"读的是 `selected.isNotEmpty()`。
 * 一旦 `selected` 里留下当前筛选看不到的 id，这两个读数就会一起骗人：
 * 表头说有 4 个，屏幕上 0 个，删除却命中 4 个（其中 3 个用户看不见）。
 *
 * 规则：
 * - 选择为空、或可见集为空（筛选无结果 / 列表正在重载）时原样返回：
 *   这两种情况下清空都只会让用户丢选择，却换不来任何一致性。
 * - 否则收敛到可见行上；没有变化时**返回同一个实例**，调用方因此可以跳过写入，
 *   也就不会在每次列表刷新（进度变化同样会重建 `visible`）时空转一次重组。
 */
internal fun selectionAfterVisibleSetChange(selected: Set<String>, visibleIds: Set<String>, visibleCount: Int): Set<String> {
    if (selected.isEmpty() || visibleCount == 0) return selected
    if (selected.all { it in visibleIds }) return selected
    return selected.filterTo(HashSet()) { it in visibleIds }
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
private data class DestructiveRequest(val action: String, val taskIds: Set<String>)
internal data class MediaSourceSelection(val path: String = "", val url: String = "", val title: String = "")

/** 展开态侧栏宽度；折叠态见 [SidebarRail]。 */
internal val SIDEBAR_WIDTH = 190.dp

/** 折叠态图标栏宽度：40dp 点击区 + 两侧 8dp 内边距。 */
internal val SIDEBAR_RAIL_WIDTH = 56.dp

/**
 * 侧栏折叠门槛 = 930 + 190 = 1120dp。
 *
 * 不是拍出来的数：窗口内容区窄于 1120dp 时，扣掉侧栏后表格可用宽度就跌破 930dp，
 * 表格会自己切到精简列。这时"完整侧栏"和"完整表格列"只能保一个——
 * 折叠侧栏（56dp）能拿回 134dp，表格可用宽度回到 1064dp，重新越过 930dp 门槛。
 *
 * 门槛之上折叠是纯损失（侧栏的计数和分组标题都还有位置放），所以只在门槛之下折叠。
 */
internal val SIDEBAR_COLLAPSE_WIDTH = TASK_TABLE_COMPACT_WIDTH + SIDEBAR_WIDTH

private data class WorkbenchPalette(
    val canvas: Color, val rail: Color, val ink: Color, val muted: Color, val faint: Color,
    val blue: Color, val onBlue: Color, val border: Color, val surface2: Color, val surface3: Color,
    val selected: Color, val dialog: Color, val success: Color, val warning: Color,
    val errorSurface: Color, val errorBorder: Color, val errorStrong: Color, val errorBody: Color,
    val warnSurface: Color, val successSurface: Color,
)

private val lightPalette = WorkbenchPalette(
    canvas = Color(0xFFEEF2F6), rail = Color.White, ink = Color(0xFF0F172A), muted = Color(0xFF475569), faint = Color(0xFF64748B),
    blue = Color(0xFF2563EB), onBlue = Color.White, border = Color(0xFFD8E0EA), surface2 = Color(0xFFF5F7FA), surface3 = Color(0xFFE8EDF3),
    selected = Color(0xFFE7F0FF), dialog = Color(0xFFFCFDFE), success = Color(0xFF16794B), warning = Color(0xFF9A5B00),
    errorSurface = Color(0xFFFFF3F2), errorBorder = Color(0xFFF2C9C5), errorStrong = Color(0xFF8A1C13), errorBody = Color(0xFF7A2E28),
    warnSurface = Color(0xFFFFF7E8), successSurface = Color(0xFFEAF8EF),
)
private val darkPalette = WorkbenchPalette(
    canvas = Color(0xFF151719), rail = Color(0xFF1C1F23), ink = Color(0xFFF4F5F6), muted = Color(0xFFC5C9CF), faint = Color(0xFF969CA4),
    blue = Color(0xFF5EA2F3), onBlue = Color(0xFF151719), border = Color(0xFF383D43), surface2 = Color(0xFF23272B), surface3 = Color(0xFF2B3035),
    // Keep blue text above AA with margin (4.65:1) without changing the shared accent.
    selected = Color(0xFF25364B), dialog = Color(0xFF22262A), success = Color(0xFF72D6A5), warning = Color(0xFFFFC46B),
    errorSurface = Color(0xFF3B2422), errorBorder = Color(0xFF61302B), errorStrong = Color(0xFFFFB3A7), errorBody = Color(0xFFF2BEB7),
    warnSurface = Color(0xFF39301C), successSurface = Color(0xFF1F3A2C),
)
private val LocalWorkbenchPalette = staticCompositionLocalOf { lightPalette }
private const val ENGINE_RECONNECTING_NOTICE = "下载引擎暂时无法连接，正在自动重试"
internal val canvas: Color @Composable @ReadOnlyComposable get() = LocalWorkbenchPalette.current.canvas
internal val rail: Color @Composable @ReadOnlyComposable get() = LocalWorkbenchPalette.current.rail
internal val ink: Color @Composable @ReadOnlyComposable get() = LocalWorkbenchPalette.current.ink
internal val muted: Color @Composable @ReadOnlyComposable get() = LocalWorkbenchPalette.current.muted
internal val faint: Color @Composable @ReadOnlyComposable get() = LocalWorkbenchPalette.current.faint
internal val blue: Color @Composable @ReadOnlyComposable get() = LocalWorkbenchPalette.current.blue

// 蓝色填充上的前景色，刻意跟主题走：调色板里的 blue 在深色主题下是**浅蓝**（#5EA2F3），
// 因为它绝大多数时候是当"墨色"用的——图标 tint、文字、焦点环、选中标记——压在近黑画布上有 6.8:1。
// 但白字压在这个浅蓝上只有 2.65:1，悬停色 #7EB7F6 上更是 2.10:1，连 3:1 的控件底线都不到。
// 和 destructiveFill 是同一个坑：**对当墨色正确的令牌，当填充就是错的**。
// 所以填充保留浅蓝，前景改用深墨（画布色本身），对 blue 6.8:1、对浅蓝变体 8.6:1。
// 浅色主题不受影响：#2563EB 够深，白字有 5.2:1，onBlue 就是白色。
// 改回 Color.White 前请重新核对：DialogPrimary、ToolbarButton(primary=true)、工具栏"新建"、
// 图标按钮的 active 态、复选框对勾。
internal val onBlue: Color @Composable @ReadOnlyComposable get() = LocalWorkbenchPalette.current.onBlue
internal val border: Color @Composable @ReadOnlyComposable get() = LocalWorkbenchPalette.current.border
internal val surface2: Color @Composable @ReadOnlyComposable get() = LocalWorkbenchPalette.current.surface2
internal val surface3: Color @Composable @ReadOnlyComposable get() = LocalWorkbenchPalette.current.surface3
internal val selectedSurface: Color @Composable @ReadOnlyComposable get() = LocalWorkbenchPalette.current.selected
// 按压档位的底色。这个表达式此前在 Main.kt 手写了 23 遍、SettingsV7.kt 1 遍：想调一档得数
// 24 处，漏一处就是某个按钮按住时比同屏的其它按钮暗一格/亮一格，而且肉眼很难归因。
// 刻意做成**派生**令牌而不是 WorkbenchPalette 的新字段：它必须跟着 surface3 / ink 走，
// 两套调色板各自算出自己的按下色；若存成实字段，将来改 surface3 就会漏改按下档。
// 方向与 WorkbenchComponents.kt 里 hover 档的论证同源：两个主题下 blendToward(ink, …)
// 都是“更远离轨道底色”的那一侧。
internal val pressedSurface: Color @Composable @ReadOnlyComposable get() = surface3.blendToward(ink, .05f)
internal val dialogSurface: Color @Composable @ReadOnlyComposable get() = LocalWorkbenchPalette.current.dialog
internal val successColor: Color @Composable @ReadOnlyComposable get() = LocalWorkbenchPalette.current.success
internal val warningColor: Color @Composable @ReadOnlyComposable get() = LocalWorkbenchPalette.current.warning
internal val errorSurface: Color @Composable @ReadOnlyComposable get() = LocalWorkbenchPalette.current.errorSurface
internal val errorBorder: Color @Composable @ReadOnlyComposable get() = LocalWorkbenchPalette.current.errorBorder
internal val errorStrong: Color @Composable @ReadOnlyComposable get() = LocalWorkbenchPalette.current.errorStrong
internal val errorBody: Color @Composable @ReadOnlyComposable get() = LocalWorkbenchPalette.current.errorBody
internal val warnSurface: Color @Composable @ReadOnlyComposable get() = LocalWorkbenchPalette.current.warnSurface
internal val successSurface: Color @Composable @ReadOnlyComposable get() = LocalWorkbenchPalette.current.successSurface

// 危险操作按钮的填充色刻意不跟主题走：调色板里的 errorStrong 是"深色主题下的浅色文字色"，
// 拿它当填充会让白字对比度掉到 1.7:1，反而不如固定的深红（对白字 6.57:1）。
private val destructiveFill = Color(0xFFB42318)

fun main() {
    val instanceLock = WorkbenchInstanceLock.acquire()
    if (instanceLock == null) {
        wakeRunningWorkbench()
        return
    }
    instanceLock.use {
        System.setProperty("compose.accessibility.enable", "true")
        System.setProperty("javax.accessibility.assistive_technologies", "com.sun.java.accessibility.AccessBridge")
        application {
    val auditSurface = System.getenv("HLS_UI_AUDIT_SURFACE").orEmpty().lowercase()
    val auditWidth = System.getenv("HLS_UI_AUDIT_WIDTH")?.toIntOrNull()?.coerceAtLeast(1024)
    val auditHeight = System.getenv("HLS_UI_AUDIT_HEIGHT")?.toIntOrNull()?.coerceAtLeast(600)
    // 夹具模式必须锁死尺寸，否则截图会被上一次运行留下的窗口尺寸污染。
    val pinnedGeometry = auditWidth != null || auditHeight != null
    val restoredGeometry = remember { if (pinnedGeometry) null else WindowGeometryStore.load() }
    val initialWidth = auditWidth ?: restoredGeometry?.first ?: 1400
    val initialHeight = auditHeight ?: restoredGeometry?.second ?: 820
    // 夹具模式把窗口锁到 (0,0)，而不是交给平台默认位置。
    //
    // 原来用 PlatformDefault 是为了"保持历史截图可比"，但平台默认位置是**层叠的、非确定的**：
    // 同一轮采集里实测出现过 (48,48) 和 (805,245) 两种。工作区只有 1536dp 宽，
    // 1110dp 的夹具落到 (805,245) 时右侧 379dp 直接出屏，截出来一片黑——
    // 而它仍然"长得像"一张正常截图，不会自己报错，属于会静默污染证据的那类故障。
    //
    // (0,0) 是唯一与窗口尺寸无关的确定位置；夹具尺寸下限是 1024×600，一定完整落在 1536×864 工作区内。
    // 真实运行仍按工作区居中，见 centeredPosition。
    val initialPosition = remember(initialWidth, initialHeight) {
        if (pinnedGeometry) 0 to 0 else WindowGeometryStore.centeredPosition(initialWidth, initialHeight)
    }
    val state = rememberWindowState(
        position = initialPosition?.let { WindowPosition(it.first.dp, it.second.dp) }
            ?: WindowPosition.PlatformDefault,
        width = initialWidth.dp,
        height = initialHeight.dp,
    )
    val appIcon = remember { loadDesktopIcon() }
    var placementBeforePlayback by remember { mutableStateOf(WindowPlacement.Floating) }
    // Native Core owns the sole resident tray; hiding here avoids creating a second AWT tray icon.
    val requestClose = { WorkbenchWindow.hideToTray() }
    var droppedPaths by remember { mutableStateOf<List<String>>(emptyList()) }
    var dropActive by remember { mutableStateOf(false) }
    var presenterAvailable by remember { mutableStateOf(false) }
    var presenterProbeComplete by remember { mutableStateOf(false) }
    Window(
        onCloseRequest = requestClose,
        title = "HLS Downloader",
        icon = appIcon?.let(::BitmapPainter),
        state = state,
        undecorated = true,
        transparent = false,
    ) {
        val windowCornerDiameter = with(LocalDensity.current) { Radius.lg.toPx() * 2 } /
            window.graphicsConfiguration.defaultTransform.scaleX
        DisposableEffect(window, state.placement, windowCornerDiameter) {
            val floating = state.placement == WindowPlacement.Floating
            fun updateWindowShape() {
                // 内容 clip 不会裁掉不透明 HWND 的方角；原生形状随窗口尺寸同步。
                window.shape = if (floating && window.extendedState == java.awt.Frame.NORMAL && window.width > 0 && window.height > 0)
                    RoundRectangle2D.Double(0.0, 0.0, window.width.toDouble(), window.height.toDouble(), windowCornerDiameter, windowCornerDiameter)
                else null
            }
            val listener = object : ComponentAdapter() {
                override fun componentResized(event: ComponentEvent) = updateWindowShape()
            }
            window.addComponentListener(listener)
            updateWindowShape()
            onDispose { window.removeComponentListener(listener) }
        }
        // 拖动缩放时尺寸连续变化，用 collectLatest + delay 去抖，避免每个像素变化都写盘。
        // 最大化 / 全屏下的尺寸不记——否则下次会以一个"屏幕大小"的浮动窗口打开。
        LaunchedEffect(pinnedGeometry) {
            if (pinnedGeometry) return@LaunchedEffect
            snapshotFlow { state.placement to state.size }
                .distinctUntilChanged()
                .collectLatest { (placement, size) ->
                    if (placement != WindowPlacement.Floating) return@collectLatest
                    delay(700)
                    // WindowState.size 是 DpSize；按 dp 存取是自洽的，高 DPI 下同样往返一致。
                    WindowGeometryStore.save(size.width.value.roundToInt(), size.height.value.roundToInt())
                }
        }
        LaunchedEffect(Unit) {
            // 最小尺寸按工作区夹过：写死 1024×600 时，工作区更小的设备（1366×768@150%、
            // 1920×1080@200% 等）会得到一个比屏幕还大的窗口，而且用户再也缩不回来。
            val (minWidth, minHeight) = WindowGeometryStore.minimumWindowSize()
            window.minimumSize = Dimension(minWidth, minHeight)
            var probeDelayMs = 5_000L
            while (true) {
                presenterAvailable = withContext(Dispatchers.IO) { EnginePipeClient.ensurePresenterStarted() }
                presenterProbeComplete = true
                // presenter 缺失时指数退避（5s 翻倍至 60s 上限），避免永久空转探测
                delay(probeDelayMs)
                probeDelayMs = if (presenterAvailable) 5_000L else (probeDelayMs * 2).coerceAtMost(60_000L)
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
            val listener = object : DropTargetAdapter() {
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
            }
            val targets = mutableListOf<Triple<java.awt.Component, DropTarget?, DropTarget>>()
            fun attach(component: java.awt.Component) {
                val previous = component.dropTarget
                targets += Triple(component, previous, DropTarget(component, listener))
                if (component is java.awt.Container) component.components.forEach(::attach)
            }
            // 文件实际落在 Skia 的子控件上，单独注册 Window 收不到跨进程拖放。
            attach(window)
            onDispose {
                uiTestApi?.close()
                targets.asReversed().forEach { (component, previous, target) ->
                    target.component = null
                    component.dropTarget = previous
                }
            }
        }
        DisposableEffect(window) {
            WorkbenchWindow.awtWindow = window
            instanceLock.listenForWake()
            onDispose {
                WorkbenchWindow.awtWindow = null
            }
        }
        AppShell(
            maximized = state.placement == WindowPlacement.Maximized,
            appIcon = appIcon,
            presenterAvailable = presenterAvailable,
            presenterProbeComplete = presenterProbeComplete,
            externalDropPaths = droppedPaths,
            externalDropActive = dropActive,
            onExternalDropConsumed = { droppedPaths = emptyList() },
            onAttention = {
                EventQueue.invokeLater {
                    // 前台任务失败时不能把焦点从 Compose 画布移到外层 Frame，
                    // 否则下一次点击只恢复画布焦点，按钮操作会丢失。
                    if (window.isVisible && !window.isMinimized && window.isActive) return@invokeLater
                    // Tray restore must reverse hideToTray() before focus/raise.
                    // A hidden Compose window can keep receiving native focus calls
                    // while remaining absent from the screen and accessibility tree.
                    window.isVisible = true
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
            onPlayerFullscreen = { fullscreen ->
                if (fullscreen && state.placement != WindowPlacement.Fullscreen) {
                    placementBeforePlayback = state.placement
                    state.placement = WindowPlacement.Fullscreen
                } else if (!fullscreen && state.placement == WindowPlacement.Fullscreen) {
                    state.placement = placementBeforePlayback
                }
            },
            titleBar = { WindowTitleBar(appIcon, state.placement == WindowPlacement.Maximized, { window.isMinimized = true }, { state.placement = if (state.placement == WindowPlacement.Maximized) WindowPlacement.Floating else WindowPlacement.Maximized }, requestClose) },
        )
        }
    }
}
}

internal fun auditSettingsTab(surface: String): String? = when (surface) {
    "settings" -> "通用"
    "settings_plan" -> "计划"
    "settings_download" -> "下载"
    "settings_network" -> "网络"
    "settings_devices" -> "投屏与推送"
    "settings_appearance" -> "外观"
    else -> null
}

@Composable @Preview
fun AppShell(maximized: Boolean = false, appIcon: ImageBitmap? = null, presenterAvailable: Boolean = false, presenterProbeComplete: Boolean = false, externalDropPaths: List<String> = emptyList(), externalDropActive: Boolean = false, onExternalDropConsumed: () -> Unit = {}, onAttention: () -> Unit = {}, onExit: () -> Unit = {}, onPlayerFullscreen: (Boolean) -> Unit = {}, titleBar: @Composable () -> Unit = {}) {
    val visualFixture = remember { System.getenv("HLS_UI_AUDIT_SURFACE").orEmpty().lowercase() }
    val visualTheme = remember { System.getenv("HLS_UI_AUDIT_THEME").orEmpty().lowercase() }
    var filter by remember { mutableStateOf(TaskFilter.ALL) }
    var category by remember { mutableStateOf<TaskCategory?>(null) }
    var selectedQueueId by remember { mutableStateOf<String?>(null) }
    var query by remember { mutableStateOf("") }
    var searchFocused by remember { mutableStateOf(false) }
    var engineHasConnected by remember { mutableStateOf(visualFixture == "tasks_1000") }
    var engineText by remember { mutableStateOf(if (visualFixture == "tasks_1000") Product.engineConnected else Product.engineStarting) }
    var extensionText by remember { mutableStateOf(Product.extensionDisconnected) }
    var selected by remember { mutableStateOf<Set<String>>(emptySet()) }
    var darkMode by remember { mutableStateOf(visualTheme == "dark") }
    var newTaskDialog by remember { mutableStateOf(visualFixture == "new_task") }
    var batchDialog by remember { mutableStateOf(visualFixture == "batch") }
    var batchCreateBusy by remember { mutableStateOf(false) }
    var harvestDialog by remember { mutableStateOf(visualFixture == "harvest") }
    var parityTool by remember { mutableStateOf("") }
    var settingsDialog by remember { mutableStateOf(auditSettingsTab(visualFixture) != null) }
    var settingsDeviceScanActive by remember { mutableStateOf(visualFixture == "settings_devices") }
    var queueManagerDialog by remember { mutableStateOf(visualFixture == "queues") }
    var queueSaveBusy by remember { mutableStateOf(false) }
    var queueAssignTaskIds by remember { mutableStateOf<Set<String>>(emptySet()) }
    var extensionDialog by remember { mutableStateOf(visualFixture == "extension") }
    var newTaskUrl by remember { mutableStateOf("") }
    var detailTaskId by remember { mutableStateOf<String?>(null) }
    // 详情弹窗原先只从任务列表里取（tasks.firstOrNull），列表里没有就静默不弹窗。
    // 这里补一个按 id 单独取的兜底：**列表优先，取回来的只在前者没有时使用**——
    // 反过来（取回的优先）会让弹窗停在打开那一刻的快照，因为事件更新的是 tasks 而不是这份兜底拷贝。
    var detailTaskFallback by remember { mutableStateOf<DownloadTask?>(null) }
    var settings by remember { mutableStateOf(EngineSettingsDto()) }
    val columnLayoutWrites = remember { Mutex() }
    // 本地先行修改（深色模式/排序）的代数：异步加载返回时若代数已前进则丢弃结果，避免旧值回跳
    var settingsEpoch by remember { mutableIntStateOf(0) }
    var settingsSaveBusy by remember { mutableStateOf(false) }
    var handoffBusy by remember { mutableStateOf(false) }
    var refreshKey by remember { mutableIntStateOf(0) }
    var snapshotReady by remember { mutableStateOf(visualFixture == "tasks_1000") }
    var eventSequence by remember { mutableLongStateOf(0) }
    var notice by remember { mutableStateOf<UiSignal.Notice?>(null) }
    val shellFocus = remember { FocusRequester() }
    var playerTaskId by remember { mutableStateOf<String?>(null) }
    var playerOverlayVisible by remember { mutableStateOf(false) }
    var playerOpenRequest by remember { mutableIntStateOf(0) }
    var probeResult by remember { mutableStateOf<UiSignal.Probe?>(null) }
    var probeDraft by remember { mutableStateOf<TaskDraft?>(null) }
    var newTaskProbeBusy by remember { mutableStateOf(false) }
    var newTaskCreateBusy by remember { mutableStateOf(false) }
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
    var castOperationEpoch by remember { mutableIntStateOf(0) }
    // 关闭或替换选择器后，旧操作的异步结果不得修改新选择器。
    val resetCastState = {
        castOperationEpoch++
        deviceResult = null
        pendingCastTask = null
        pendingMediaSource = null
        pendingCastMode = ""
        pendingPushRequestId = null
        castDiscovering = false
        castConnecting = false
    }
    var castControlBusy by remember { mutableStateOf(false) }
    var updateDownloadBusy by remember { mutableStateOf(false) }
    var castPollFailures by remember { mutableIntStateOf(0) }
    var destructiveRequest by remember { mutableStateOf<DestructiveRequest?>(null) }
    var aboutDialog by remember { mutableStateOf(false) }
    var noticesDialog by remember { mutableStateOf(false) }
    val noticeHistory = remember { mutableStateListOf<NoticeHistoryEntry>() }
    fun recordNotice(level: String, message: String) {
        val last = noticeHistory.lastOrNull()
        if (last != null && last.level == level && last.message == message) return
        noticeHistory.add(NoticeHistoryEntry(System.currentTimeMillis(), level, message))
        while (noticeHistory.size > 100) noticeHistory.removeAt(0)
    }
    val scope = rememberCoroutineScope()
    suspend fun resolveMediaPushTerminalReliably(
        requestId: String,
        status: String,
        message: String,
        location: String = "",
    ) {
        var failures = 0
        while (failures < maxMediaPushResolutionAttempts) {
            // 发送已结束的请求必须同步终态，即使选择器已被后续请求替换。
            val result = runCatching {
                withContext(Dispatchers.IO) {
                    EnginePipeClient.resolveMediaPush(requestId, status, message, location)
                }
            }
            if (result.isSuccess) return
            val error = result.exceptionOrNull() ?: return
            if (error is CancellationException) throw error
            if (!shouldRetryMediaPushResolution(error)) {
                UiDiagnostics.warning(
                    "media_push.resolve_terminal_rejected",
                    describeTaskActionFailure(error, "媒体推送终态同步被下载引擎拒绝"),
                    requestId = requestId,
                )
                if (pendingPushRequestId == requestId) notice = UiSignal.Notice(
                    "error",
                    if ((error as? EngineProtocolException)?.code == "media_push_not_found") {
                        "投送请求已过期，浏览器端将按失败处理"
                    } else {
                        "投送状态同步被下载引擎拒绝"
                    },
                )
                return
            }
            failures++
            if (failures == 1) {
                UiDiagnostics.error("media_push.resolve_terminal", error, requestId = requestId)
                if (pendingPushRequestId == requestId) notice = UiSignal.Notice("error", "投送结果已确定，但浏览器状态同步失败，正在自动重试")
            }
            delay(mediaPushResolutionRetryDelayMillis(failures))
        }
        if (pendingPushRequestId != requestId) return
        UiDiagnostics.error(
            "media_push.resolve_terminal_exhausted",
            IllegalStateException("媒体推送终态同步在 $maxMediaPushResolutionAttempts 次尝试后仍未完成"),
            requestId = requestId,
        )
        notice = UiSignal.Notice("error", "投送状态同步多次失败，请在浏览器端重试该投送")
    }
    val tasks = remember(visualFixture) {
        mutableStateListOf<DownloadTask>().apply {
            if (visualFixture == "tasks_1000") addAll(auditTasks(1_000))
        }
    }
    val taskPositions = remember(tasks) {
        mutableStateMapOf<String, Int>().apply { tasks.forEachIndexed { index, task -> put(task.id, index) } }
    }
    val taskLogs = remember { mutableStateMapOf<String, List<String>>() }
    val taskLogOrder = remember { ArrayDeque<String>() }
    fun recordTaskLog(taskId: String, lines: List<String>) {
        if (taskId !in taskLogs) {
            taskLogOrder.addLast(taskId)
            while (taskLogOrder.size > 512) {
                taskLogs.remove(taskLogOrder.removeFirst())
            }
        }
        taskLogs[taskId] = lines
    }
    fun forgetTaskLog(taskId: String) {
        taskLogs.remove(taskId)
        taskLogOrder.remove(taskId)
    }
    val handoffQueue = remember { mutableStateListOf<HandoffOfferDto>() }
    // presenterReady 与 handoffQueue 同属"演示端在不在"这一组状态，放在一起；
    // applyEngineEventHere 要读它，而 Kotlin 的局部变量必须声明在使用之前。
    val presenterReady = rememberUpdatedState(presenterProbeComplete && presenterAvailable)
    LaunchedEffect(presenterAvailable, presenterProbeComplete) {
        if (presenterAvailable) {
            handoffQueue.removeAll { it.presentation != "fallback" }
        } else if (presenterProbeComplete) {
            refreshKey++
        }
    }

    // applyEngineEvent 的四个回调（setExtension / offerHandoff / resolveHandoff / taskDeleted）
    // 在 AppShell 里被逐字抄了 6 遍：单条任务操作、批量任务操作、剪贴板探测 poller、拖入导入、
    // 媒体推送握手、结果轮询各一份。抄 6 遍的意思是改一次要记得改 6 处——漏掉 taskDeleted 就会留下
    // “任务已删但日志缓存还在”这种只在特定入口复现的问题。
    // 这里只收拢「事件怎么落到任务表上」；信号分发仍由调用方自己写，因为不同入口要接的 UiSignal
    // 分支不一样（只有剪贴板探测那段要额外弹新任务框、塞探测结果）。
    // 必须排在 handoffQueue / presenterReady 之后：Kotlin 的局部变量要在使用前声明。
    val applyEngineEventHere: (kotlinx.serialization.json.JsonObject) -> UiSignal? = { event ->
        applyEngineEvent(
            tasks,
            taskPositions,
            event,
            setExtension = { connected -> extensionText = connected },
            offerHandoff = { offer ->
                if (!presenterReady.value && handoffQueue.none { it.handoffId == offer.handoffId }) {
                    handoffQueue += offer
                }
            },
            resolveHandoff = { handoffId -> handoffQueue.removeAll { it.handoffId == handoffId } },
            taskDeleted = { deletedId -> forgetTaskLog(deletedId) },
        )
    }
    fun discoverCastForPicker(mode: String) {
        val operationEpoch = castOperationEpoch
        castDiscovering = true
        scope.launch {
            runCatching { withContext(Dispatchers.IO) { EnginePipeClient.discoverCastDevices(mode) } }
                .onSuccess { result ->
                    if (castOperationEpoch == operationEpoch) {
                        result.events.forEach { envelope ->
                            when (val signal = applyEngineEventHere(envelope.event)) {
                                is UiSignal.Devices -> deviceResult = signal
                                is UiSignal.Notice -> notice = signal
                                else -> Unit
                            }
                        }
                        castDiscovering = false
                    }
                }
                .onFailure { error ->
                    if (error is CancellationException) throw error
                    if (castOperationEpoch == operationEpoch) {
                        castDiscovering = false
                        notice = UiSignal.Notice("error", describeFailure(error, "设备搜索失败"))
                    }
                }
        }
    }
    val fallbackCandidate = if (presenterProbeComplete && !presenterAvailable) {
        handoffQueue.firstOrNull { it.presentation != "fallback" }
    } else {
        null
    }
    val activeHandoff = if (presenterProbeComplete) {
        handoffQueue.firstOrNull { it.presentation == "fallback" }
    } else {
        null
    }
    val visible by remember(tasks) {
        derivedStateOf {
            sortTasks(visibleTasks(tasks, filter, category, query, selectedQueueId), settings.taskSort)
        }
    }
    // 可见集一变，选择就要收敛到可见行上。否则筛选后台头仍显示"4 已选"，
    // 而屏幕上一行都没选中，紧接着的批量删除会连被筛掉的 3 个任务一起删——
    // 用户在屏幕上根本看不到它们。Linux 上实测复现（见迭代日志第三十四轮）：
    // 连点 全部→进行中→排队中→已暂停→已完成→失败，selectedCount 一直是 4。
    val visibleIds by remember { derivedStateOf { visible.mapTo(HashSet()) { it.id } } }
    LaunchedEffect(visibleIds, visible.size) {
        val pruned = selectionAfterVisibleSetChange(selected, visibleIds, visible.size)
        if (pruned != selected) selected = pruned
    }
    UiTestStateMirror(selected, visible.size, filter.label, category?.label ?: "", selectedQueueId)
    fun performTaskAction(taskId: String, action: String) {
        if (action == "play") {
            playerOverlayVisible = true
            playerOpenRequest++
            playerTaskId = taskId
            return
        }
        if (action in setOf("delete", "delete_files")) {
            destructiveRequest = DestructiveRequest(action, setOf(taskId))
            return
        }
        if (action == "cast" || action == "push_tvbox") {
            resetCastState()
            settingsDeviceScanActive = false
            pendingCastTask = taskId
            pendingCastMode = if (action == "push_tvbox") "tvbox" else "cast"
            deviceResult = UiSignal.Devices(emptyList())
            discoverCastForPicker(pendingCastMode)
            return
        }
        if (action == "move_queue") {
            queueAssignTaskIds = setOf(taskId)
            return
        }
        val requestId = UUID.randomUUID().toString()
        scope.launch {
            runCatching { withContext(Dispatchers.IO) {
                when (action) {
                    "open" -> EnginePipeClient.openCompleted(taskId, false)
                    "open_folder" -> EnginePipeClient.openCompleted(taskId, true)
                    "log" -> EnginePipeClient.getTaskLog(taskId)
                    "save_site_profile" -> EnginePipeClient.saveSiteProfile(taskId)
                    else -> EnginePipeClient.taskAction(taskId, action)
                }
            } }.onSuccess { result ->
                if (action == "log") recordTaskLog(taskId, result.taskLogLines())
                // 不再 refreshKey++：命令响应里已经带回本次动作的事件（applyEngineEvent 用的是和
                // 事件轮询同一个函数），省掉 effect 重跑带来的 loadHandoffs + loadSettings 两次往返。
                result.events.forEach { envelope ->
                    handleSignal(applyEngineEventHere(envelope.event)) { signal -> if (signal is UiSignal.Notice) notice = signal }
                }
            }.onFailure { error ->
                UiDiagnostics.error("task_action.$action", error, taskId, requestId)
                notice = UiSignal.Notice("error", describeTaskActionFailure(error))
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
                // 每项独立接收错误，避免一项状态已变化就跳过后续任务；成功结果仍增量更新。
                runCatching { withContext(Dispatchers.IO) { taskIds.map { it to runCatching { EnginePipeClient.taskAction(it, action) } } } }
                    .onSuccess { results ->
                        results.forEach { (_, result) ->
                            result.onSuccess { response ->
                                response.events.forEach { envelope ->
                                    handleSignal(applyEngineEventHere(envelope.event)) { signal -> if (signal is UiSignal.Notice) notice = signal }
                                }
                            }
                        }
                        val failures = results.filter { it.second.isFailure }
                        if (failures.isNotEmpty()) {
                            val (taskId, failure) = failures.first()
                            val name = tasks.firstOrNull { it.id == taskId }?.filename ?: taskId
                            notice = UiSignal.Notice("error", "${failures.size} 个任务操作失败；首项 $name：${describeTaskActionFailure(failure.exceptionOrNull()!!)}")
                        }
                    }
                    .onFailure { notice = UiSignal.Notice("error", describeTaskActionFailure(it, "批量操作失败")) }
            }
        }
    }
    fun applyWorkbenchShortcut(action: String?): Boolean = when (action) {
        "new" -> { newTaskUrl = ""; newTaskDialog = true; true }
        "batch" -> { batchDialog = true; true }
        "settings" -> { settingsDialog = true; true }
        "refresh" -> { refreshKey++; true }
        "escape" -> when {
            newTaskDialog -> { if (!newTaskProbeBusy && !newTaskCreateBusy) newTaskDialog = false; true }
            batchDialog -> { if (!batchCreateBusy) batchDialog = false; true }
            harvestDialog -> { harvestDialog = false; true }
            settingsDialog -> { if (!settingsSaveBusy) settingsDialog = false; true }
            queueManagerDialog -> { if (!queueSaveBusy) queueManagerDialog = false; true }
            extensionDialog -> { extensionDialog = false; true }
            detailTaskId != null -> { detailTaskId = null; detailTaskFallback = null; true }
            searchFocused && query.isNotEmpty() -> { query = ""; true }
            // 列表清选由 TaskTable 处理，避免 AWT 在菜单/播放器之前吞掉 Esc。
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
            runCatching { withContext(Dispatchers.IO) { EnginePipeClient.importPaths(paths) } }
                .onSuccess { result ->
                    val count = result.events.count { it.event["kind"]?.jsonPrimitive?.content == "task_created" }
                    notice = UiSignal.Notice("success", "已导入 $count 项任务")
                    refreshKey++
                }
                .onFailure { notice = UiSignal.Notice("error", describeFailure(it, "拖入文件失败")) }
            onExternalDropConsumed()
        }
    }
    LaunchedEffect(refreshKey) {
        if (visualFixture == "tasks_1000") {
            engineText = Product.engineConnected
            return@LaunchedEffect
        }
        if (!snapshotReady) engineText = if (engineHasConnected) Product.engineReconnecting else Product.engineStarting
        var attempts = 0
        var lastSpawnAt = 0L
        var spawnDelayMs = 2_000L
        while (!snapshotReady) {
            val snapshot = runCatching { withContext(Dispatchers.IO) { EnginePipeClient.snapshotState() } }
            val state = snapshot.getOrNull()
            if (state != null) {
                taskPositions.clear()
                tasks.clear()
                state.tasks.forEach { upsertDownloadTask(tasks, taskPositions, downloadTask(it)) }
                eventSequence = state.latestSequence
                extensionText = if (state.browserConnected) "浏览器插件 · 已连接" else Product.extensionDisconnected
                snapshotReady = true
                engineHasConnected = true
                engineText = Product.engineConnected
                spawnDelayMs = 2_000L
                if (notice?.message == ENGINE_RECONNECTING_NOTICE) notice = null
                break
            }
            val error = snapshot.exceptionOrNull() ?: IllegalStateException("下载引擎连接失败")
            if (attempts == 0) UiDiagnostics.error("engine.snapshot", error)
            attempts++
            engineText = if (engineHasConnected) Product.engineReconnecting else Product.engineStarting
            if (attempts == 12) notice = UiSignal.Notice("error", ENGINE_RECONNECTING_NOTICE)
            // 引擎进程拉起采用指数退避（2s 翻倍至 32s 上限），避免引擎损坏时无限重启进程
            val now = System.currentTimeMillis()
            if (now - lastSpawnAt >= spawnDelayMs) {
                withContext(Dispatchers.IO) { EnginePipeClient.ensureStarted() }
                lastSpawnAt = now
                spawnDelayMs = (spawnDelayMs * 2).coerceAtMost(32_000L)
            }
            delay((100L + attempts * 40L).coerceAtMost(500L))
        }
        runCatching { withContext(Dispatchers.IO) { EnginePipeClient.loadHandoffs() } }
            .onSuccess { offers ->
                offers.forEach { offer ->
                    val index = handoffQueue.indexOfFirst { it.handoffId == offer.handoffId }
                    if (index < 0) {
                        if (!presenterAvailable || offer.presentation == "fallback") handoffQueue += offer
                    } else if (offer.presentation == "fallback") {
                        handoffQueue[index] = offer
                    }
                }
            }
            .onFailure { error ->
                // LaunchedEffect(refreshKey) 每次 refreshKey 变化都会取消并重启这个协程；
                // 取消时 withContext(loadHandoffs()) 抛出的 CancellationException 会被
                // runCatching 吞掉，其 message 是组合框架的
                // "The coroutine scope left the composition"。那是**正常取消**，不是失败——
                // 必须原样抛回，绝不能变成一条 error 通知给用户看。
                if (error is CancellationException) throw error
                UiDiagnostics.error("handoff.restore", error)
                notice = UiSignal.Notice("error", error.message ?: "读取浏览器接管请求失败")
            }
        val settingsEpochAtLoad = settingsEpoch
        runCatching { withContext(Dispatchers.IO) { EnginePipeClient.loadSettings() } }
            .onSuccess { loaded ->
                if (settingsEpochAtLoad == settingsEpoch) {
                    settings = loaded
                    darkMode = when (visualTheme) { "light" -> false; "dark" -> true; else -> loaded.darkMode }
                }
            }
        if (visualFixture.isBlank()) {
            runCatching { withContext(Dispatchers.IO) { EnginePipeClient.loadMediaPushRequests().firstOrNull() } }
                .onSuccess { request ->
                    if (request != null && pendingPushRequestId == null) {
                        resetCastState()
                        settingsDeviceScanActive = false
                        pendingPushRequestId = request.id
                        pendingCastTask = null
                        pendingCastMode = request.pushKind
                        pendingMediaSource = MediaSourceSelection(url = request.url, title = request.title)
                        deviceResult = UiSignal.Devices(emptyList())
                        discoverCastForPicker(request.pushKind)
                    }
                }
        }
    }
    LaunchedEffect(Unit) {
        if (visualFixture == "tasks_1000") return@LaunchedEffect
        while (true) {
            if (!snapshotReady) { delay(80); continue }
            val received = runCatching { withContext(Dispatchers.IO) { EnginePipeClient.waitEvents(eventSequence) } }
            received.onSuccess { events ->
                events.forEach { envelope -> try {
                    // Core 的事件序号单调且随快照下发基线；快照后的下一个事件应为 eventSequence + 1，
                    // 无论基线是否为 0（Core 重新启动时基线来自持久化检查点）
                    if (envelope.sequence > eventSequence + 1) {
                        snapshotReady = false
                        refreshKey++
                    }
                    eventSequence = maxOf(eventSequence, envelope.sequence)
                    val eventKind = envelope.event.string("kind")
                    if (eventKind == "settings_changed" ||
                        (eventKind == "ui_show" && envelope.event.stringOrNull("surface") == "main")) {
                        refreshKey++
                    }
                    val attentionRequired = when (eventKind) {
                        // 浏览器交接由独立确认窗处理，工作台只响应明确的打开请求。
                        "error", "open_main" -> true
                        "ui_show" -> envelope.event.stringOrNull("surface") == "main"
                        "update_install_result" -> envelope.event.stringOrNull("status") != "success"
                        "task_created", "task_updated", "task_progress" -> {
                            val snapshot = envelope.event.objectOrNull("snapshot")
                            val taskId = snapshot?.string("task_id").orEmpty()
                            val previousStatus = taskPositions[taskId]?.let { tasks[it].source.status }.orEmpty().lowercase()
                            val nextStatus = snapshot?.string("status").orEmpty().lowercase()
                            previousStatus != nextStatus && nextStatus in setOf("failed", "error")
                        }
                        else -> false
                    }
                    if (eventKind == "error") {
                        if (envelope.event.stringOrNull("code") == "torrent_probe_failed") {
                            torrentDraft = null
                        }
                        val engineMessage = envelope.event.stringOrNull("message")?.takeIf(String::isNotBlank) ?: "下载引擎报告错误"
                        UiDiagnostics.warning(
                            "engine.event.error",
                            engineMessage,
                            envelope.event.string("task_id"),
                            envelope.event.string("request_id"),
                        )
                        recordNotice("error", engineMessage)
                    }
                    if (eventKind in setOf("task_created", "task_updated", "task_progress")) {
                        val snapshot = envelope.event.objectOrNull("snapshot")
                        val status = snapshot?.string("status").orEmpty().lowercase()
                        if (status in setOf("failed", "error")) {
                            val hint = snapshot?.string("error_hint").orEmpty()
                            val message = snapshot?.string("error_message").orEmpty()
                            if (hint.isNotBlank() || message.isNotBlank()) {
                                notice = UiSignal.Notice("error", hint.ifBlank { message })
                            }
                        }
                    }
                    handleSignal(applyEngineEventHere(envelope.event), { signal ->
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
                            is UiSignal.TorrentProbe -> {
                                if (torrentDraft?.url?.trim() == signal.data.source.trim()) {
                                    newTaskDialog = false
                                    torrentProbe = signal
                                }
                            }
                            is UiSignal.TorrentSelection -> notice = UiSignal.Notice("success", "已选择 ${signal.files.count { it.selected }} 个文件，共 ${formatBytes(signal.totalSize)}")
                            is UiSignal.Devices -> {
                                // 选择器只接收自身命令的结果，迟到的广播不能覆盖后续扫描。
                                if (settingsDeviceScanActive) {
                                    deviceResult = signal
                                    castDiscovering = false
                                }
                            }
                            is UiSignal.MediaPush -> {
                                if (pendingPushRequestId != signal.request.id) {
                                    resetCastState()
                                    settingsDeviceScanActive = false
                                    pendingPushRequestId = signal.request.id
                                    pendingCastTask = null
                                    pendingCastMode = signal.request.pushKind
                                    pendingMediaSource = MediaSourceSelection(url = signal.request.url, title = signal.request.title)
                                    deviceResult = UiSignal.Devices(emptyList())
                                    discoverCastForPicker(signal.request.pushKind)
                                }
                            }
                            is UiSignal.MediaPushResolved -> {
                                val request = signal.request
                                val level = when (request.status.lowercase()) {
                                    "done" -> "success"
                                    "canceled" -> "info"
                                    else -> "error"
                                }
                                if (shouldCloseMediaPushPicker(pendingPushRequestId, request.id, request.status)) {
                                    resetCastState()
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
                                recordTaskLog(signal.taskId, signal.lines.takeLast(500))
                                val index = taskPositions[signal.taskId]
                                if (index != null) tasks[index] = tasks[index].copy(source = tasks[index].source.copy(logTail = signal.lines.takeLast(500)))
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
                        }
                    })
                    if (attentionRequired) onAttention()
                } catch (cancelled: kotlinx.coroutines.CancellationException) {
                    throw cancelled
                } catch (error: Throwable) {
                    // One malformed envelope must not end live event delivery:
                    // this poller is keyed on Unit, so an exception escaping the
                    // per-envelope processing cancelled the coroutine and it
                    // never restarted -- no progress, no new tasks, no handoffs,
                    // no cast/player/update events until the process restarted.
                    UiDiagnostics.error("engine.apply_event", error)
                } }
                engineHasConnected = true
                engineText = Product.engineConnected
            }.onFailure { error ->
                UiDiagnostics.error("engine.wait_events", error)
                engineText = if (engineHasConnected) Product.engineReconnecting else Product.engineStarting
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
            runCatching { withContext(Dispatchers.IO) { EnginePipeClient.checkUpdate(silent = true) } }
                .onFailure { UiDiagnostics.warning("update.check.silent", describeFailure(it, "静默检查更新失败")) }
            delay(24 * 60 * 60 * 1_000L)
        }
    }
    LaunchedEffect(castSession?.active, castSession?.supportedActions) {
        var castPollReachable = true
        while (visualFixture !in setOf("cast", "cast_tvbox", "cast_lan", "cast_offline", "media_stack") && castSession?.active == true && castSession?.supportedActions?.contains("status") == true) {
            delay(1_000)
            runCatching { withContext(Dispatchers.IO) { EnginePipeClient.controlCast("status") } }
                .onSuccess { castPollReachable = true }
                .onFailure { error ->
                    castPollFailures++
                    if (castPollFailures >= 2) castSession = castSession?.copy(status = "OFFLINE", playing = false, paused = true)
                    // 只在设备从可达变为不可达时提示一次，避免每秒轮询失败都弹出错误通知
                    if (castPollReachable) notice = UiSignal.Notice("error", describeFailure(error, "无法读取投屏设备状态"))
                    castPollReachable = false
                    delay(2_000)
                }
        }
    }
    LaunchedEffect(playerTaskId) { if (playerTaskId == null) shellFocus.requestFocus() }
    val taskActionLimiter = remember { Semaphore(8) }
    fun launchTaskActions(entries: List<Pair<String, String>>) {
        if (entries.isEmpty()) {
            notice = UiSignal.Notice("info", "当前没有支持此操作的任务")
            return
        }
        scope.launch {
            val results = entries.map { (taskId, action) ->
                async { taskActionLimiter.withPermit {
                    // 同上：靠命令响应里已经带回的事件增量更新任务行，不再拉全量快照。
                    val result = runCatching { withContext(Dispatchers.IO) { EnginePipeClient.taskAction(taskId, action) } }
                        .onSuccess { result ->
                            result.events.forEach { envelope ->
                                handleSignal(applyEngineEventHere(envelope.event)) { signal -> if (signal is UiSignal.Notice) notice = signal }
                            }
                        }
                        .onFailure { error ->
                            UiDiagnostics.error("task.batch.$action", error, taskId)
                            notice = UiSignal.Notice("error", describeTaskActionFailure(error, "批量操作失败"))
                        }
                    taskId to result
                } }
            }.awaitAll()
            val failures = results.filter { it.second.isFailure }
            if (failures.isNotEmpty()) {
                val (taskId, failure) = failures.first()
                val name = tasks.firstOrNull { it.id == taskId }?.filename ?: taskId
                notice = UiSignal.Notice("error", "${failures.size} 个任务操作失败；首项 $name：${describeTaskActionFailure(failure.exceptionOrNull()!!)}")
            }
        }
    }
    val modalVisible = newTaskDialog || batchDialog || harvestDialog || parityTool.isNotEmpty() || settingsDialog || queueManagerDialog ||
        queueAssignTaskIds.isNotEmpty() || detailTaskId != null || extensionDialog || activeHandoff != null ||
        probeResult != null || torrentDraft != null || torrentProbe != null || harvestResult != null || mediaSourceDialog.isNotEmpty() ||
        (!settingsDeviceScanActive && deviceResult != null) || duplicateResult != null || updateResult != null ||
        preparedUpdate != null || powerPending != null || destructiveRequest != null || aboutDialog || noticesDialog || playerOverlayVisible
    fun updateTaskColumns(layout: String) {
        settingsEpoch++
        settings = settings.copy(taskColumnLayout = layout)
        scope.launch {
            columnLayoutWrites.withLock {
                runCatching { withContext(Dispatchers.IO) { EnginePipeClient.storeSetting("task_column_layout", layout) } }
                    .onFailure { notice = UiSignal.Notice("error", "列宽已调整，但未能保存；重启后可能恢复原布局。${describeFailure(it, "列宽保存失败")}") }
            }
        }
    }
    val toolbarTargets = toolbarTaskTargets(tasks, visible, selected)
    CompositionLocalProvider(LocalWorkbenchPalette provides if (darkMode) darkPalette else lightPalette) {
    Box(Modifier.fillMaxSize()) {
    val windowShape = RoundedCornerShape(if (maximized) 0.dp else Radius.lg)
    Column(Modifier.fillMaxSize().focusRequester(shellFocus).focusable().onPreviewKeyEvent { playerOverlayVisible }.background(canvas, windowShape).border(1.dp, border, windowShape).clip(windowShape).then(if (modalVisible) Modifier.clearAndSetSemantics { } else Modifier)) {
        titleBar()
        DesktopToolbar(query, { query = it }, { newTaskUrl = ""; newTaskDialog = true }, {
            scope.launch {
                val clipboard = runCatching { withContext(Dispatchers.IO) { Toolkit.getDefaultToolkit().systemClipboard.getData(DataFlavor.stringFlavor) as? String } }.getOrNull().orEmpty()
                newTaskUrl = clipboard.trim(); newTaskDialog = true
            }
        }, { batchDialog = true }, { harvestDialog = true }, { launchTaskActions(toolbarTargets.mapNotNull { task -> taskStartAction(task.source)?.let { task.id to it } }) }, { launchTaskActions(toolbarTargets.filter { "pause" in it.source.availableActions }.map { it.id to "pause" }) }, { mediaSourceDialog = "cast" }, { mediaSourceDialog = "tvbox" }, { extensionDialog = true }, { settingsDialog = true }, darkMode, {
            settingsEpoch++
            darkMode = !darkMode
            val requestedDarkMode = darkMode
            scope.launch {
                runCatching { withContext(Dispatchers.IO) { EnginePipeClient.storeSetting("dark_mode", requestedDarkMode) } }
                    .onFailure { notice = UiSignal.Notice("error", "主题已切换，但未能保存；重启后可能恢复原主题。${describeFailure(it, "主题保存失败")}") }
            }
        }, canStart = toolbarTargets.any { taskStartAction(it.source) != null }, canPause = toolbarTargets.any { "pause" in it.source.availableActions },
            selectedCount = selected.size,
            canDeleteSelected = selected.isNotEmpty() && toolbarTargets.any { "delete" in taskMenuActions(it.source) },
            onDeleteSelected = {
                val ids = selectedTaskActionTargets(toolbarTargets.map { it.source }, "delete").mapTo(mutableSetOf()) { it.first }
                if (ids.isNotEmpty()) destructiveRequest = DestructiveRequest("delete", ids)
            }, onSearchFocus = { searchFocused = it }, onZip = { parityTool = "zip" }, onCrawl = { parityTool = "crawl" })
        Row(Modifier.weight(1f).fillMaxWidth()) {
                // 侧栏宽度断点。Row 会把自身的最大宽度原样传给非 weight 子项，所以这里量到的
                // maxWidth 就是"窗口内容区宽度"，正是断点需要比较的量。
                BoxWithConstraints(Modifier.fillMaxHeight()) {
                    // 各组计数面向全部任务；切换入口时清掉其他组的筛选，避免有计数却看不到任务。
                    Sidebar(
                        filter, category, selectedQueueId, settings.queueProfiles, tasks, maxWidth < SIDEBAR_COLLAPSE_WIDTH,
                        { filter = it; category = null; selectedQueueId = null },
                        { category = if (category == it) null else it; filter = TaskFilter.ALL; selectedQueueId = null },
                        { selectedQueueId = if (selectedQueueId == it) null else it; filter = TaskFilter.ALL; category = null },
                        { queueManagerDialog = true },
                    )
                }
                Column(Modifier.weight(1f).fillMaxHeight().background(canvas)) {
                ContentHeader(category?.label ?: settings.queueProfiles.firstOrNull { it.id == selectedQueueId }?.name ?: filter.label, visible.size, visible.filter { it.id in selected }, visible.count { it.status == "已完成" }, { selected = emptySet() }, { refreshKey++ }, {
                    val completedIds = visible.filter { it.status == "已完成" }.map { it.id }.toSet()
                    if (completedIds.isNotEmpty()) {
                        destructiveRequest = DestructiveRequest("delete", completedIds)
                    }
                }, onMore = { action ->
                    when (action) {
                        "import" -> chooseImportPaths()?.let { paths -> scope.launch { runCatching { withContext(Dispatchers.IO) { EnginePipeClient.importPaths(paths) } }.onSuccess { result -> notice = UiSignal.Notice("success", "已导入 ${result.events.count { it.event["kind"]?.jsonPrimitive?.content == "task_created" }} 项任务"); refreshKey++ }.onFailure { notice = UiSignal.Notice("error", describeFailure(it, "导入失败")) } } }
                        "export" -> chooseExportPath()?.let { path -> scope.launch { runCatching { withContext(Dispatchers.IO) { exportTaskList(path, selected.toList()) } }.onSuccess { result -> notice = UiSignal.Notice("success", "已导出 ${result.taskCount} 项任务") }.onFailure { notice = UiSignal.Notice("error", describeFailure(it, "导出失败")) } } }
                        "update" -> scope.launch { runCatching { withContext(Dispatchers.IO) { EnginePipeClient.checkUpdate(silent = false) } }.onFailure { notice = UiSignal.Notice("error", describeTaskActionFailure(it, "检查更新失败")) } }
                        "cancel_power" -> scope.launch { runCatching { withContext(Dispatchers.IO) { EnginePipeClient.cancelPowerAction() } }.onFailure { notice = UiSignal.Notice("error", describeFailure(it, "取消电源动作失败")) } }
                        "notices" -> noticesDialog = true
                        "reset_columns" -> updateTaskColumns("")
                        "about" -> aboutDialog = true
                        "exit" -> onExit()
                    }
                }) { action ->
                    val targets = selectedTaskActionTargets(visible.filter { it.id in selected }.map { it.source }, action)
                    if (targets.isNotEmpty()) {
                        val ids = targets.mapTo(mutableSetOf()) { it.first }
                        when (action) {
                            "delete", "delete_files" -> destructiveRequest = DestructiveRequest(action, ids)
                            "move_queue" -> queueAssignTaskIds = ids
                            "play", "cast", "push_tvbox" -> targets.singleOrNull()?.let { performTaskAction(it.first, it.second) }
                            else -> launchTaskActions(targets)
                        }
                    }
                }
                TaskTable(
                    visible,
                    selected,
                    appIcon,
                    settings.taskSort,
                    { selected = it },
                    { detailTaskId = it },
                    onDeleteSelection = { if (selected.isNotEmpty()) destructiveRequest = DestructiveRequest("delete", selected) },
                    onQueueMove = { taskId, delta -> scope.launch {
                        runCatching { withContext(Dispatchers.IO) { EnginePipeClient.reorderQueue(taskId, delta) } }
                            .onSuccess { refreshKey++ }.onFailure { notice = UiSignal.Notice("error", describeTaskActionFailure(it, "队列排序失败")) }
                    } },
                    modifier = Modifier.weight(1f),
                    onBatchAction = ::performTaskActions,
                    hasTasks = tasks.isNotEmpty(),
                    onClearFilters = { filter = TaskFilter.ALL; category = null; selectedQueueId = null; query = ""; selected = emptySet() },
                    onSort = { field ->
                        val next = nextTaskSort(settings.taskSort, field)
                        settingsEpoch++
                        settings = settings.copy(taskSort = next)
                        scope.launch {
                            runCatching { withContext(Dispatchers.IO) { EnginePipeClient.storeSetting("task_sort", next) } }
                                .onFailure { notice = UiSignal.Notice("error", "排序已切换，但未能保存；重启后可能恢复原排序。${describeFailure(it, "排序保存失败")}") }
                        }
                    },
                    columnLayout = settings.taskColumnLayout,
                    onColumnLayoutChange = ::updateTaskColumns,
                ) { taskId, action -> performTaskAction(taskId, action) }
                }
        }
        ConnectionStatus(tasks, engineText, extensionText)
    }
    playerTaskId?.let { taskId -> key(taskId) {
        LocalVideoPlayer(taskId, tasks.firstOrNull { it.id == taskId }?.source,
            onClose = { playerTaskId = null; playerOverlayVisible = false }, onFullscreen = onPlayerFullscreen,
            onModalChanged = { playerOverlayVisible = it }, openRequest = playerOpenRequest)
    } }
    // 使用主窗口内的 dp 布局；独立 Popup 的像素偏移在高 DPI 下会盖住工具栏。
    if (!modalVisible) notice?.let { signal ->
        NoticeToast(signal, Modifier.align(Alignment.TopEnd).padding(top = 96.dp, end = 18.dp)) { notice = null }
    }
    }
    if (externalDropActive || visualFixture == "drop") {
        Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
            Surface(color = dialogSurface, shape = RoundedCornerShape(Radius.lg), shadowElevation = Elevation.e3, border = BorderStroke(2.dp, blue), modifier = Modifier.width(390.dp)) {
                Row(Modifier.padding(horizontal = 20.dp, vertical = 18.dp), verticalAlignment = Alignment.CenterVertically) {
                    Surface(Modifier.size(42.dp), color = selectedSurface, shape = RoundedCornerShape(Radius.md)) { Box(contentAlignment = Alignment.Center) { Icon(Icons.Outlined.FileDownload, null, tint = blue, modifier = Modifier.size(23.dp)) } }
                    Spacer(Modifier.width(13.dp)); Column { Text("松开以导入", color = ink, fontSize = TypeScale.title, fontWeight = FontWeight.SemiBold); Text("支持任务 JSON、种子、Metalink 和 URL 列表", color = muted, fontSize = TypeScale.micro, modifier = Modifier.padding(top = 4.dp)) }
                }
            }
        }
    }
    if (newTaskDialog) NewTaskDialog({ if (!newTaskProbeBusy && !newTaskCreateBusy) newTaskDialog = false }, newTaskUrl, settings, probing = newTaskProbeBusy, creating = newTaskCreateBusy, onProbe = { draft ->
        if (newTaskProbeBusy || newTaskCreateBusy) return@NewTaskDialog
        newTaskProbeBusy = true
        if (taskProbeTarget(draft) == ResourceProbeTarget.Torrent) {
            torrentDraft = draft.copy(queueId = selectedQueueId ?: "default")
            scope.launch {
                runCatching { withContext(Dispatchers.IO) { EnginePipeClient.probeTorrent(draft.url) } }
                    .onSuccess { newTaskProbeBusy = false; newTaskDialog = false }
                    .onFailure { newTaskProbeBusy = false; torrentDraft = null; notice = UiSignal.Notice("error", describeTaskActionFailure(it, "种子分析失败")) }
            }
        } else {
            scope.launch {
                probeDraft = draft
                runCatching { withContext(Dispatchers.IO) { EnginePipeClient.probeUrl(draft) } }
                    .onSuccess { newTaskProbeBusy = false; newTaskDialog = false }
                    .onFailure { newTaskProbeBusy = false; notice = UiSignal.Notice("error", describeTaskActionFailure(it, "资源分析失败")) }
            }
        }
    }) { draft ->
        if (newTaskCreateBusy || newTaskProbeBusy) return@NewTaskDialog
        newTaskCreateBusy = true
        if (draft.kind.equals("torrent", true)) {
            torrentDraft = draft.copy(queueId = selectedQueueId ?: "default")
            scope.launch {
                runCatching { withContext(Dispatchers.IO) { EnginePipeClient.probeTorrent(draft.url) } }
                    .onSuccess { newTaskDialog = false }
                    .onFailure { torrentDraft = null; notice = UiSignal.Notice("error", describeTaskActionFailure(it, "种子分析失败")) }
                newTaskCreateBusy = false
            }
            return@NewTaskDialog
        }
        scope.launch {
            runCatching { withContext(Dispatchers.IO) {
                val queued = draft.copy(queueId = selectedQueueId ?: "default")
                if (queued.curlCommand.isNotBlank()) EnginePipeClient.importCurl(queued) else EnginePipeClient.createTask(queued)
            } }
                .onSuccess { newTaskDialog = false; refreshKey++ }
                .onFailure { notice = UiSignal.Notice("error", describeTaskActionFailure(it, "创建下载失败")) }
            newTaskCreateBusy = false
        }
    }
    if (torrentDraft != null && torrentProbe == null && !newTaskDialog) {
        WorkbenchDialog({ torrentDraft = null }, "正在解析种子", "取得文件清单后可选择需要下载的文件", 520.dp, content = {
            Row(Modifier.fillMaxWidth().padding(vertical = 20.dp), verticalAlignment = Alignment.CenterVertically) {
                CircularProgressIndicator(Modifier.size(24.dp), strokeWidth = 2.dp, color = blue)
                Spacer(Modifier.width(14.dp))
                Text("正在连接节点并读取种子元数据…", color = ink, fontSize = TypeScale.body)
            }
            Text("节点无响应时解析可能较慢。关闭窗口不会创建下载任务。", color = muted, fontSize = TypeScale.caption)
        }, actions = { DialogSecondary("关闭") { torrentDraft = null } })
    }
    torrentProbe?.let { signal -> TorrentSelectionDialog(signal.data, newTaskCreateBusy, { torrentProbe = null; torrentDraft = null }) { files ->
        if (newTaskCreateBusy) return@TorrentSelectionDialog
        val draft = torrentDraft ?: return@TorrentSelectionDialog
        newTaskCreateBusy = true
        scope.launch {
            try {
                runCatching {
                    withContext(Dispatchers.IO) {
                        EnginePipeClient.selectTorrentFiles(draft.url, files)
                        EnginePipeClient.createTask(draft.copy(torrentSelection = files))
                    }
                }.onSuccess { torrentProbe = null; torrentDraft = null; refreshKey++ }
                    .onFailure { notice = UiSignal.Notice("error", describeTaskActionFailure(it, "创建种子任务失败")) }
            } finally {
                newTaskCreateBusy = false
            }
        }
    } }
    if (batchDialog) BatchAddDialog({ if (!batchCreateBusy) batchDialog = false }, batchCreateBusy) { urls ->
        batchCreateBusy = true
        val queueId = selectedQueueId ?: "default"
        try {
            val failures = withContext(Dispatchers.IO) {
                urls.mapNotNull { url ->
                    runCatching { EnginePipeClient.createTask(TaskDraft(url = url, queueId = queueId)) }
                        .exceptionOrNull()?.let { url to describeTaskActionFailure(it, "创建任务失败") }
                }
            }
            refreshKey++
            if (failures.isEmpty()) {
                batchDialog = false
                notice = UiSignal.Notice("success", "已处理 ${urls.size} 条下载链接")
            }
            failures
        } finally {
            batchCreateBusy = false
        }
    }
    if (parityTool == "zip") ZipPreviewDialog { parityTool = "" }
    if (parityTool == "crawl") CrawlProjectsDialog(settings.downloadDirectory) { parityTool = "" }
    if (harvestDialog) HarvestDialog({ harvestDialog = false }, settings.defaultReferer) { url, referer ->
        val effectiveReferer = referer.ifBlank { url }
        harvestReferer = effectiveReferer
        scope.launch {
            runCatching { withContext(Dispatchers.IO) { EnginePipeClient.harvestPage(url, effectiveReferer) } }
                .onSuccess { refreshKey++ }
                .onFailure { notice = UiSignal.Notice("error", describeTaskActionFailure(it, "页面抓取失败")) }
        }
        harvestDialog = false
    }
    if (settingsDialog) FullSettingsDialog(
        onDismiss = {
            settingsDialog = false
            settingsSaveBusy = false
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
                runCatching { withContext(Dispatchers.IO) { EnginePipeClient.discoverCastDevices(mode) } }
                    .onSuccess { result ->
                        result.events.forEach { envelope ->
                            val signal = applyEngineEvent(tasks, taskPositions, envelope.event, { extensionText = it }, {}, {}, {})
                            if (signal is UiSignal.Devices && settingsDeviceScanActive) {
                                deviceResult = signal
                            }
                        }
                        if (settingsDeviceScanActive) castDiscovering = false
                    }
                    .onFailure { error ->
                        castDiscovering = false
                        notice = UiSignal.Notice("error", describeFailure(error, "设备扫描失败"))
                    }
            }
        },
        initialTab = auditSettingsTab(visualFixture) ?: "通用",
        saving = settingsSaveBusy,
    ) { updated, defaultCookie, siteRuleCredentialEdits, subtitleEdit ->
        settingsSaveBusy = true
        scope.launch {
            runCatching { withContext(Dispatchers.IO) {
                val client = EnginePipeClient
                var saved = client.saveSettings(updated)
                if (defaultCookie != null) saved = client.saveDefaultCookie(defaultCookie)
                siteRuleCredentialEdits.forEach { edit -> saved = client.saveSiteRuleCredential(edit) }
                if (subtitleEdit != null) client.saveSubtitleConfiguration(subtitleEdit)
                saved
            } }
                .onSuccess { saved ->
                    settingsEpoch++
                    settings = saved
                    darkMode = saved.darkMode
                    settingsDialog = false
                    settingsDeviceScanActive = false
                    if (pendingCastTask == null && pendingMediaSource == null) deviceResult = null
                    notice = UiSignal.Notice("success", "设置已保存")
                }
                .onFailure { error -> notice = UiSignal.Notice("error", describeFailure(error, "设置保存失败")) }
            settingsSaveBusy = false
        }
    }
    if (queueManagerDialog) QueueManagerDialog(settings.queueProfiles, { if (!queueSaveBusy) queueManagerDialog = false }, saving = queueSaveBusy) { profiles ->
        if (queueSaveBusy) return@QueueManagerDialog
        queueSaveBusy = true
        scope.launch {
            runCatching { withContext(Dispatchers.IO) {
                EnginePipeClient.saveSettings(settings.copy(queueProfiles = profiles))
            } }.onSuccess { saved ->
                settingsEpoch++
                settings = saved
                if (selectedQueueId !in profiles.map { it.id }) selectedQueueId = null
                queueManagerDialog = false
                refreshKey++
                notice = UiSignal.Notice("success", "队列设置已保存")
            }.onFailure { notice = UiSignal.Notice("error", describeFailure(it, "队列设置保存失败")) }
            queueSaveBusy = false
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
            runCatching { withContext(Dispatchers.IO) { EnginePipeClient.assignQueue(taskIds, queueId) } }
                .onSuccess { refreshKey++; notice = UiSignal.Notice("success", "已移动 ${taskIds.size} 个任务") }
                .onFailure { notice = UiSignal.Notice("error", describeTaskActionFailure(it, "移动任务失败")) }
        }
    }
    LaunchedEffect(detailTaskId) {
        val id = detailTaskId
        // 列表里已有就什么都不做（那份本来就会随事件实时更新，也多一次 IPC）
        if (id == null || tasks.any { it.id == id }) {
            detailTaskFallback = null
            return@LaunchedEffect
        }
        val fetched = runCatching {
            withContext(Dispatchers.IO) { EnginePipeClient.getTask(id) }
        }.getOrNull()
        detailTaskFallback = if (fetched == null) null else {
            val scratch = mutableStateListOf<DownloadTask>()
            val scratchPositions = mutableMapOf<String, Int>()
            fetched.events.forEach { envelope ->
                applyEngineEvent(
                    scratch,
                    scratchPositions,
                    envelope.event,
                    setExtension = {},
                    offerHandoff = {},
                    resolveHandoff = {},
                    taskDeleted = {},
                )
            }
            scratch.firstOrNull()
        }
    }
    LaunchedEffect(engineText) { UiTestState.updateEngineText(engineText) }
    // notice 历史导给 /state：断言"某条提示真的出现过、就是这个字符串"。
    // 用最后一条的时间戳作 key —— 列表每次重组都会 toList() 出新对象，
    // 拿它当 key 会让 effect 每次重组都跑一遍，没有意义。
    val lastNoticeAt = noticeHistory.lastOrNull()?.at ?: 0L
    LaunchedEffect(lastNoticeAt) {
        UiTestState.updateNotices(noticeHistory.takeLast(12).map { Triple(it.level, it.message, it.at) })
    }
    detailTaskId?.let { taskId -> tasks.firstOrNull { it.id == taskId } ?: detailTaskFallback }?.let { task ->
        TaskDetailsDialog(
            task = task,
            logLines = taskLogs[task.id],
            onDismiss = { detailTaskId = null },
            onRefreshRequest = { url, cookie ->
                val requestId = UUID.randomUUID().toString()
                scope.launch {
                    runCatching { withContext(Dispatchers.IO) { EnginePipeClient.refreshTaskRequest(task.id, url, cookie) } }
                        .onSuccess { refreshKey++; notice = UiSignal.Notice("success", if (cookie.isBlank()) "下载地址已更新" else "下载地址和凭据已更新") }
                        .onFailure { error ->
                            UiDiagnostics.error("task_refresh_request", error, task.id, requestId)
                            notice = UiSignal.Notice("error", describeFailure(error, "更新下载请求失败"))
                        }
                }
            },
            onAction = { action -> performTaskAction(task.id, action) },
        )
    }
    if (extensionDialog) ExtensionDialog(extensionText) { extensionDialog = false }
    if (noticesDialog) NoticesDialog(noticeHistory.toList(), onClear = { noticeHistory.clear() }, onDismiss = { noticesDialog = false })
    if (aboutDialog) AboutDialog(
        engineText,
        extensionText,
        onOpenLogs = {
            scope.launch {
                val opened = withContext(Dispatchers.IO) {
                    runCatching { java.awt.Desktop.getDesktop().open(UiDiagnostics.logsDirectory().toFile()) }.isSuccess
                }
                if (!opened) notice = UiSignal.Notice("error", "打开日志文件夹失败")
            }
        },
        onOpenHomepage = {
            scope.launch {
                val opened = withContext(Dispatchers.IO) {
                    runCatching { java.awt.Desktop.getDesktop().browse(java.net.URI("https://github.com/ciaooo55/hls-downloader")) }.isSuccess
                }
                if (!opened) notice = UiSignal.Notice("error", "打开项目主页失败")
            }
        },
        onDismiss = { aboutDialog = false },
    )
    LaunchedEffect(notice) {
        notice?.let {
            recordNotice(it.level, it.message)
            delay(3_600)
            notice = null
        }
    }
    LaunchedEffect(fallbackCandidate?.handoffId, fallbackCandidate?.presentation) {
        val offer = fallbackCandidate ?: return@LaunchedEffect
        var failureReported = false
        while (!presenterReady.value && handoffQueue.any { it.handoffId == offer.handoffId && it.presentation != "fallback" }) {
            val claimed = runCatching {
                withContext(Dispatchers.IO) { EnginePipeClient.claimHandoffFallback(offer.handoffId) }
            }
            if (claimed.isSuccess) {
                val index = handoffQueue.indexOfFirst { it.handoffId == offer.handoffId }
                if (index >= 0) handoffQueue[index] = offer.copy(presentation = "fallback")
                return@LaunchedEffect
            }
            if (!failureReported) {
                UiDiagnostics.warning(
                    "handoff.claim_fallback",
                    describeFailure(claimed.exceptionOrNull(), "等待下载确认窗口释放请求"),
                    offer.handoffId,
                )
                failureReported = true
            }
            val status = runCatching {
                withContext(Dispatchers.IO) { EnginePipeClient.loadHandoffStatuses(offer.handoffId) }
            }.getOrNull()?.firstOrNull { it.id == offer.handoffId }?.status
            if (status != null && status != "pending") {
                handoffQueue.removeAll { it.handoffId == offer.handoffId }
                return@LaunchedEffect
            }
            delay(5_000)
        }
    }
    activeHandoff?.let { offer ->
        LaunchedEffect(offer.handoffId, handoffBusy) {
            if (handoffBusy) return@LaunchedEffect
            var statusReadFailed = false
            while (handoffQueue.any { it.handoffId == offer.handoffId }) {
                delay(1_500)
                runCatching { withContext(Dispatchers.IO) { EnginePipeClient.loadHandoffStatuses(offer.handoffId) } }
                    .onSuccess { statuses ->
                        statusReadFailed = false
                        val status = statuses.firstOrNull { it.id == offer.handoffId }?.status
                        if (status != null && status != "pending") {
                            handoffQueue.removeAll { it.handoffId == offer.handoffId }
                        }
                    }
                    .onFailure { error ->
                        if (!statusReadFailed) {
                            UiDiagnostics.error("handoff.status", error, offer.handoffId)
                            notice = UiSignal.Notice("error", describeFailure(error, "读取浏览器接管状态失败"))
                        }
                        statusReadFailed = true
                    }
            }
        }
        BrowserHandoffDialog(
            offer = offer,
            settings = settings,
            duplicate = tasks.firstOrNull { canonicalHandoffUrl(it.source.url) == canonicalHandoffUrl(offer.url) },
            pendingCount = if (presenterAvailable) {
                handoffQueue.count { it.presentation == "fallback" }
            } else {
                handoffQueue.size
            },
            busy = handoffBusy,
            onAccept = { decision ->
                handoffBusy = true
                scope.launch {
                    runCatching {
                        withContext(Dispatchers.IO) {
                            val client = EnginePipeClient
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
                        .onFailure { error ->
                            UiDiagnostics.error("handoff.accept", error, offer.handoffId)
                            notice = UiSignal.Notice("error", describeTaskActionFailure(error, "接受浏览器接管请求失败"))
                        }
                    handoffBusy = false
                }
            },
            onReject = { suppressSiteKind ->
                handoffBusy = true
                scope.launch {
                    runCatching { withContext(Dispatchers.IO) { EnginePipeClient.rejectHandoff(offer.handoffId, suppressSiteKind) } }
                        .onSuccess { handoffQueue.removeAll { it.handoffId == offer.handoffId } }
                        .onFailure { error ->
                            UiDiagnostics.error("handoff.reject", error, offer.handoffId)
                            notice = UiSignal.Notice("error", describeTaskActionFailure(error, "拒绝浏览器接管请求失败"))
                        }
                    handoffBusy = false
                }
            },
        )
    }
    probeResult?.let { signal -> ProbeResultDialog(signal, newTaskCreateBusy, { probeResult = null; probeDraft = null }) { variant ->
        if (newTaskCreateBusy) return@ProbeResultDialog
        val draft = (probeDraft ?: TaskDraft(url = signal.url)).copy(
            preferredBandwidth = variant?.bandwidth ?: 0,
            preferredHeight = variant?.height ?: 0,
            preferredAudio = variant?.name.orEmpty(),
            queueId = selectedQueueId ?: "default",
        )
        newTaskCreateBusy = true
        scope.launch {
            try {
                runCatching { withContext(Dispatchers.IO) { EnginePipeClient.createTask(draft) } }
                    .onSuccess { probeResult = null; probeDraft = null; refreshKey++ }
                    .onFailure { notice = UiSignal.Notice("error", describeTaskActionFailure(it, "创建媒体任务失败")) }
            } finally {
                newTaskCreateBusy = false
            }
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
                    withContext(Dispatchers.IO) { EnginePipeClient.probeHarvestSizes(signal.url, referer, urls) }
                }.onSuccess { sizes ->
                    harvestResult = harvestResult?.let { current ->
                        current.copy(links = mergeHarvestSizes(current.links, sizes))
                    }
                }.onFailure { notice = UiSignal.Notice("error", describeTaskActionFailure(it, "读取文件大小失败")) }
                harvestProbeBusy = false
            }
        },
    ) { request ->
        scope.launch {
            runCatching {
                withContext(Dispatchers.IO) {
                    request.urls.forEach { url ->
                        EnginePipeClient.createTask(TaskDraft(
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
                .onFailure { notice = UiSignal.Notice("error", describeTaskActionFailure(it, "创建抓取任务失败")) }
        }
    } }
    if (mediaSourceDialog.isNotEmpty()) MediaSourcePickerDialog(mediaSourceDialog, { mediaSourceDialog = "" }) { source ->
        resetCastState()
        settingsDeviceScanActive = false
        pendingMediaSource = source
        pendingCastTask = null
        pendingCastMode = mediaSourceDialog
        mediaSourceDialog = ""
        deviceResult = UiSignal.Devices(emptyList())
        discoverCastForPicker(pendingCastMode)
    }
    if (!settingsDeviceScanActive) deviceResult?.let { signal -> DevicePickerDialog(signal, pendingCastMode, pendingMediaSource, castDiscovering, castConnecting, settings.preferredCastDeviceId, castOperationEpoch, {
        val requestId = pendingPushRequestId
        val operationEpoch = castOperationEpoch
        if (requestId == null) {
            resetCastState()
        } else if (!castConnecting) {
            castConnecting = true
            scope.launch {
                resolveMediaPushTerminalReliably(requestId, "canceled", "已取消设备选择")
                if (castOperationEpoch == operationEpoch) {
                    resetCastState()
                }
            }
        }
    }, onRescan = {
        discoverCastForPicker(pendingCastMode)
    }, onPublish = {
        val taskId = pendingCastTask
        val media = pendingMediaSource
        val requestId = pendingPushRequestId
        val operationEpoch = castOperationEpoch
        if ((taskId != null || media != null) && !castConnecting) {
            castConnecting = true
            scope.launch {
                val outcome = runCatching { withContext(Dispatchers.IO) {
                    if (taskId != null) EnginePipeClient.castTask(taskId)
                    else EnginePipeClient.shareMedia(media!!.path, media.url, media.title, "", requestId.orEmpty())
                } }
                outcome.exceptionOrNull()?.let { if (it is CancellationException) throw it }
                if (outcome.isSuccess) {
                    if (requestId != null) resolveMediaPushTerminalReliably(requestId, "done", "已发布局域网播放地址")
                    if (castOperationEpoch == operationEpoch) {
                        resetCastState()
                    }
                } else {
                    val message = describeFailure(outcome.exceptionOrNull(), "局域网发布失败")
                    if (requestId != null) {
                        resolveMediaPushTerminalReliably(requestId, "failed", message)
                    }
                    if (castOperationEpoch != operationEpoch) return@launch
                    if (requestId != null) resetCastState()
                    notice = UiSignal.Notice("error", message)
                }
                if (castOperationEpoch == operationEpoch) castConnecting = false
            }
        }
    }) { device ->
        val taskId = pendingCastTask
        val media = pendingMediaSource
        val requestId = pendingPushRequestId
        val operationEpoch = castOperationEpoch
        if ((taskId != null || media != null) && !castConnecting) {
            castConnecting = true
            scope.launch {
                val outcome = runCatching { withContext(Dispatchers.IO) {
                    if (taskId != null) EnginePipeClient.castToDevice(taskId, device.id)
                    else EnginePipeClient.shareMedia(media!!.path, media.url, media.title, device.id, requestId.orEmpty())
                } }
                outcome.exceptionOrNull()?.let { if (it is CancellationException) throw it }
                if (outcome.isSuccess) {
                    settings = settings.copy(preferredCastDeviceId = device.id)
                    runCatching { withContext(Dispatchers.IO) { EnginePipeClient.storeSetting("preferred_cast_device_id", device.id) } }
                        .onFailure { error ->
                            if (error is CancellationException) throw error
                            UiDiagnostics.warning(
                                "media_push.preferred_device",
                                describeFailure(error, "首选投屏设备保存失败"),
                                requestId = requestId.orEmpty(),
                            )
                        }
                    if (requestId != null) resolveMediaPushTerminalReliably(requestId, "done", "已发送到 ${device.label}")
                    if (castOperationEpoch == operationEpoch) {
                        resetCastState()
                    }
                } else {
                    val message = describeFailure(outcome.exceptionOrNull(), "投屏连接失败")
                    if (requestId != null) {
                        resolveMediaPushTerminalReliably(requestId, "failed", message)
                    }
                    if (castOperationEpoch != operationEpoch) return@launch
                    if (requestId != null) resetCastState()
                    notice = UiSignal.Notice("error", message)
                }
                if (castOperationEpoch == operationEpoch) castConnecting = false
            }
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
            runCatching { withContext(Dispatchers.IO) { EnginePipeClient.downloadUpdate() } }
                .onSuccess { notice = UiSignal.Notice("success", "安装包已下载并完成身份校验") }
                .onFailure { notice = UiSignal.Notice("error", describeFailure(it, "下载或校验安装包失败")) }
            updateDownloadBusy = false
        }
    } }
    preparedUpdate?.let { signal -> UpdatePreparedDialog(signal, updateDownloadBusy, { if (!updateDownloadBusy) preparedUpdate = null }) {
        if (!updateDownloadBusy) scope.launch {
            updateDownloadBusy = true
            runCatching { withContext(Dispatchers.IO) { EnginePipeClient.installUpdate(ProcessHandle.current().pid()) } }
                .onSuccess {
                    notice = UiSignal.Notice("info", "任务断点已保存，正在关闭工作台并完成升级")
                    delay(100)
                    onExit()
                }
                .onFailure { notice = UiSignal.Notice("error", describeFailure(it, "无法开始覆盖升级")) }
            updateDownloadBusy = false
        }
    } }
    powerPending?.let { signal -> PowerActionDialog(signal, onCancel = {
        powerPending = null
        scope.launch {
            runCatching { withContext(Dispatchers.IO) { EnginePipeClient.cancelPowerAction() } }
                .onSuccess { notice = UiSignal.Notice("info", "已取消完成后电源动作") }
                .onFailure { notice = UiSignal.Notice("error", describeFailure(it, "取消电源动作失败")) }
        }
    }, onConfirm = {
        powerPending = null
        scope.launch {
            runCatching { withContext(Dispatchers.IO) { EnginePipeClient.confirmPowerAction() } }
                .onFailure { notice = UiSignal.Notice("error", describeFailure(it, "执行电源动作失败")) }
        }
    }) }
    if (castSession?.active == true) {
        Popup(alignment = Alignment.BottomEnd, offset = androidx.compose.ui.unit.IntOffset(-18, -46), properties = PopupProperties(focusable = false)) {
            Column(horizontalAlignment = Alignment.End, verticalArrangement = Arrangement.spacedBy(10.dp)) {
                castSession?.takeIf { it.active }?.let { signal -> CastSessionHud(signal, tasks.firstOrNull { it.id == signal.taskId }, castControlBusy, onCopy = { value ->
                    runCatching { Toolkit.getDefaultToolkit().systemClipboard.setContents(StringSelection(value), null) }
                        .onSuccess { notice = UiSignal.Notice("success", "播放地址已复制") }
                        .onFailure { notice = UiSignal.Notice("error", "复制播放地址失败") }
                }) { action, seconds ->
                    if (!castControlBusy) scope.launch {
                        castControlBusy = true
                        runCatching { withContext(Dispatchers.IO) { EnginePipeClient.controlCast(action, seconds) } }
                            .onSuccess { if (action == "stop") castSession = null }
                            .onFailure { notice = UiSignal.Notice("error", describeFailure(it, "投屏控制失败")) }
                        castControlBusy = false
                    }
                } }

            }
        }
    }
    destructiveRequest?.let { request -> DestructiveConfirmDialog(request, { destructiveRequest = null }) {
        destructiveRequest = null
        scope.launch {
            runCatching { withContext(Dispatchers.IO) { request.taskIds.map { taskId -> taskId to runCatching { EnginePipeClient.taskAction(taskId, request.action) } } } }
                .onSuccess { results ->
                    results.forEach { (taskId, result) ->
                        result.onSuccess { response ->
                            selected = selected - taskId
                            response.events.forEach { envelope ->
                                handleSignal(applyEngineEventHere(envelope.event)) { signal -> if (signal is UiSignal.Notice) notice = signal }
                            }
                        }
                    }
                    val failures = results.filter { it.second.isFailure }
                    if (failures.isNotEmpty()) {
                        val (taskId, failure) = failures.first()
                        val name = tasks.firstOrNull { it.id == taskId }?.filename ?: taskId
                        notice = UiSignal.Notice("error", "${failures.size} 个任务删除失败；首项 $name：${describeTaskActionFailure(failure.exceptionOrNull()!!)}")
                    }
                }
                .onFailure { notice = UiSignal.Notice("error", describeTaskActionFailure(it, "删除失败")) }
        }
    } }
    }
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
            // 第 1 行做成 BT 任务，好让"任务详情"里那段需要 torrentFiles 的面板（全选/反选/更新下载链接）
            // 能在本机量到。选 index==1 是因为它 status="queued"、availableActions 里有 start，
            // 这样 canRefresh 也为真，更新下载链接那个按钮才出现；而且它排在可见区第二行，
            // 不依赖滚动（滚轮在本机是废的）。
            // resourceKind 只影响展示标签（"BT"、"Piece"），不影响其它夹具。
            resourceKind = if (index == 1) "torrent" else if (extension == "m3u8") "hls" else "file",
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
private inline fun handleSignal(signal: UiSignal?, consume: (UiSignal) -> Unit) { if (signal != null) consume(signal) }
internal fun chooseDirectory(initialPath: String = "", title: String = "选择目录"): String? {
    val chooser = JFileChooser().apply {
        dialogTitle = title
        fileSelectionMode = JFileChooser.DIRECTORIES_ONLY
        isAcceptAllFileFilterUsed = false
        if (initialPath.isNotBlank()) currentDirectory = java.io.File(initialPath).let { if (it.isDirectory) it else it.parentFile }
    }
    return if (chooser.showOpenDialog(WorkbenchWindow.awtWindow) == JFileChooser.APPROVE_OPTION) chooser.selectedFile.absolutePath else null
}

private fun chooseImportPaths(): List<String>? {
    val chooser = JFileChooser().apply {
        dialogTitle = "导入任务 JSON、种子、Metalink 或 URL 列表"
        isMultiSelectionEnabled = true
        fileFilter = FileNameExtensionFilter("支持的导入文件", "json", "torrent", "metalink", "meta4", "txt", "urls")
    }
    return if (chooser.showOpenDialog(WorkbenchWindow.awtWindow) == JFileChooser.APPROVE_OPTION) chooser.selectedFiles.map { it.absolutePath } else null
}

private fun chooseExportPath(): java.nio.file.Path? {
    val chooser = JFileChooser().apply {
        dialogTitle = "导出任务列表"
        selectedFile = java.io.File("hls-downloader-tasks.json")
        fileFilter = FileNameExtensionFilter("JSON、CSV 或 URL 列表", "json", "csv", "txt")
    }
    return if (chooser.showSaveDialog(WorkbenchWindow.awtWindow) == JFileChooser.APPROVE_OPTION) chooser.selectedFile.toPath() else null
}

internal fun chooseMediaPath(): String? {
    val chooser = JFileChooser().apply {
        dialogTitle = "选择要投屏或推送的媒体文件"
        isMultiSelectionEnabled = false
        fileFilter = FileNameExtensionFilter("视频和音频文件", "mp4", "mkv", "webm", "mov", "avi", "m4v", "mp3", "flac", "wav", "m4a", "aac", "ts", "m3u8")
    }
    return if (chooser.showOpenDialog(WorkbenchWindow.awtWindow) == JFileChooser.APPROVE_OPTION) chooser.selectedFile.absolutePath else null
}

private fun exportTaskList(path: java.nio.file.Path, taskIds: List<String>): TaskExportResult {
    val extension = path.fileName.toString().substringAfterLast('.', "json").lowercase()
    val format = when (extension) {
        "csv" -> "csv"
        "txt", "urls" -> "urls"
        else -> "json"
    }
    val result = EnginePipeClient.exportTasks(taskIds, format)
    writeTaskExport(path, result.data)
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
            Text("HLS Downloader ${Product.version}", color = ink, fontSize = TypeScale.caption, fontWeight = FontWeight.Medium)
            Spacer(Modifier.weight(1f))
            IconButton(onClick = onMinimize, modifier = Modifier.width(45.dp).fillMaxHeight()) { Icon(Icons.Outlined.Minimize, "最小化", tint = muted, modifier = Modifier.size(16.dp)) }
            IconButton(onClick = onToggleMaximize, modifier = Modifier.width(45.dp).fillMaxHeight()) { Icon(if (maximized) Icons.Outlined.FilterNone else Icons.Outlined.CropSquare, if (maximized) "还原" else "最大化", tint = muted, modifier = Modifier.size(14.dp)) }
            IconButton(onClick = onClose, modifier = Modifier.width(45.dp).fillMaxHeight()) { Icon(Icons.Outlined.Close, "关闭", tint = muted, modifier = Modifier.size(17.dp)) }
        }
    }
}

@Composable private fun DesktopToolbar(query: String, onQuery: (String) -> Unit, onNew: () -> Unit, onPaste: () -> Unit, onBatch: () -> Unit, onHarvest: () -> Unit, onStart: () -> Unit, onPause: () -> Unit, onCastMedia: () -> Unit, onPushMedia: () -> Unit, onExtension: () -> Unit, onSettings: () -> Unit, dark: Boolean, onTheme: () -> Unit, canStart: Boolean, canPause: Boolean, selectedCount: Int, canDeleteSelected: Boolean, onDeleteSelected: () -> Unit, onSearchFocus: (Boolean) -> Unit, onZip: () -> Unit, onCrawl: () -> Unit) {
    var toolsOpen by remember { mutableStateOf(false) }
    BoxWithConstraints(Modifier.height(52.dp).fillMaxWidth().background(rail)) {
        val compact = maxWidth < 1050.dp
        val narrow = maxWidth < 980.dp
        @Composable fun Action(id: String) {
            val (icon, label, action) = when (id) {
                "new" -> Triple(Icons.Outlined.Add, "新建", onNew)
                "paste" -> Triple(Icons.Outlined.ContentPaste, "粘贴链接", onPaste)
                "start_all" -> Triple(Icons.Outlined.PlayArrow, if (selectedCount > 0) "开始 / 继续" else "全部开始", onStart)
                "pause_all" -> Triple(Icons.Outlined.Pause, if (selectedCount > 0) "暂停" else "全部暂停", onPause)
                else -> return
            }
            val enabled = when (id) {
                "start_all" -> canStart
                "pause_all" -> canPause
                else -> true
            }
            val hint = when {
                id == "start_all" && !enabled -> "没有可开始、继续或重试的任务"
                id == "pause_all" && !enabled -> "没有可暂停的任务"
                selectedCount > 0 && id in setOf("start_all", "pause_all") -> "$label（已选 $selectedCount 项）"
                else -> label
            }
            if (id == "new") ToolbarButton(icon, label, action, true)
            else if (compact && id != "paste") ToolbarIcon(icon, label, enabled, hint, action)
            else WorkbenchTooltip(hint) { ToolbarButton(icon, label, action, enabled = enabled) }
        }
        Row(Modifier.fillMaxSize().padding(horizontal = if (narrow) 8.dp else 14.dp), verticalAlignment = Alignment.CenterVertically) {
            Action("new")
            Spacer(Modifier.width(6.dp))
            Action("paste")
            Spacer(Modifier.width(12.dp))
            Action("start_all")
            Action("pause_all")
            if (selectedCount > 0) ToolbarIcon(Icons.Outlined.DeleteOutline, "删除所选任务", enabled = canDeleteSelected, hint = "删除所选任务记录；保留已下载文件", action = onDeleteSelected)
            Box {
                ToolbarButton(Icons.Outlined.MoreHoriz, "工具", { toolsOpen = true })
                DropdownMenu(toolsOpen, { toolsOpen = false }, shape = RoundedCornerShape(Radius.lg), containerColor = dialogSurface, shadowElevation = Elevation.e2) {
                    listOf(
                        Triple(Icons.AutoMirrored.Outlined.PlaylistAdd, "批量添加", onBatch),
                        Triple(Icons.Outlined.TravelExplore, "页面抓取", onHarvest),
                        Triple(Icons.Outlined.TravelExplore, "抓站与同步项目", onCrawl),
                        Triple(Icons.Outlined.FolderOpen, "ZIP 内容预览", onZip),
                        Triple(Icons.Outlined.Cast, "投屏到电视", onCastMedia),
                        Triple(Icons.Outlined.Tv, "TVBox 推送", onPushMedia),
                        Triple(Icons.Outlined.Extension, "浏览器插件", onExtension),
                    ).forEach { (icon, label, action) ->
                        DropdownMenuItem(
                            text = {
                                Row(verticalAlignment = Alignment.CenterVertically) {
                                    Icon(icon, null, modifier = Modifier.size(18.dp))
                                    Spacer(Modifier.width(10.dp))
                                    Text(label, fontSize = TypeScale.body)
                                }
                            },
                            onClick = { toolsOpen = false; action() },
                            modifier = Modifier.reportControlBounds("toolbar.$label"),
                        )
                    }
                }
            }
            Spacer(Modifier.weight(1f))
            ToolbarSearchField(query, onQuery, narrow, onSearchFocus)
            Spacer(Modifier.width(5.dp))
            if (compact) ToolbarIcon(Icons.Outlined.Settings, "设置", action = onSettings) else ToolbarButton(Icons.Outlined.Settings, "设置", onSettings)
            ToolbarIcon(if (dark) Icons.Outlined.LightMode else Icons.Outlined.DarkMode, if (dark) "浅色模式" else "深色模式", action = onTheme)
        }
        HorizontalDivider(Modifier.align(Alignment.BottomCenter), color = border)
    }
}
@Composable private fun ToolbarSearchField(value: String, onValue: (String) -> Unit, narrow: Boolean, onFocus: (Boolean) -> Unit) {
    var focused by remember { mutableStateOf(false) }
    val focusRequester = remember { FocusRequester() }
    Row(
        Modifier.width(if (narrow) 145.dp else 190.dp).height(36.dp)
            .background(if (focused) rail else surface2, RoundedCornerShape(Radius.md)).border(1.dp, if (focused) blue else border, RoundedCornerShape(Radius.md)).clip(RoundedCornerShape(Radius.md)).padding(start = 10.dp, end = if (value.isEmpty()) 10.dp else 0.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(Icons.Outlined.Search, "搜索", modifier = Modifier.size(17.dp), tint = muted); Spacer(Modifier.width(8.dp))
        BasicTextField(
            value = value,
            onValueChange = onValue,
            modifier = Modifier.weight(1f).focusRequester(focusRequester).onFocusChanged { focused = it.isFocused; onFocus(it.isFocused) }
                .semantics { contentDescription = "搜索任务" },
            singleLine = true,
            textStyle = TextStyle(color = ink, fontSize = TypeScale.caption),
            cursorBrush = SolidColor(blue),
            decorationBox = { field -> if (value.isEmpty()) Text(if (narrow) "搜索" else "搜索任务", color = faint, fontSize = TypeScale.caption, maxLines = 1); field() },
        )
        if (value.isNotEmpty()) WorkbenchTooltip("清除搜索（Esc）") {
            IconButton(onClick = { onValue(""); focusRequester.requestFocus() }, modifier = Modifier.size(34.dp).reportControlBounds("toolbar.clear_search")) {
                Icon(Icons.Outlined.Close, "清除搜索", modifier = Modifier.size(15.dp), tint = muted)
            }
        }
    }
}
@Composable private fun ToolbarButton(icon: ImageVector, label: String, action: () -> Unit, primary: Boolean = false, enabled: Boolean = true) { Button(onClick = action, enabled = enabled, colors = ButtonDefaults.buttonColors(containerColor = if (primary) blue else Color.Transparent, contentColor = if (primary) onBlue else ink), border = if (primary) null else BorderStroke(1.dp, Color.Transparent), shape = RoundedCornerShape(Radius.md), contentPadding = PaddingValues(horizontal = 8.dp), modifier = Modifier.height(36.dp).padding(horizontal = 1.dp).reportControlBounds("toolbar.$label"), hoverColor = if (primary) null else surface3, pressedColor = if (primary) null else pressedSurface) { Icon(icon, null, Modifier.size(16.dp)); Spacer(Modifier.width(5.dp)); Text(label, fontSize = TypeScale.caption, fontWeight = if (primary) FontWeight.SemiBold else FontWeight.Medium) } }
@Composable private fun ToolbarIcon(icon: ImageVector, text: String, enabled: Boolean = true, hint: String = text, action: () -> Unit) = WorkbenchTooltip(hint) { IconButton(onClick = action, enabled = enabled, modifier = Modifier.size(36.dp).reportControlBounds("toolbar.$text")) { Icon(icon, text, tint = if (enabled) ink else muted, modifier = Modifier.size(18.dp)) } }

/**
 * 侧栏导航行的外壳：悬停换底色、按压缩一点、选中给选中底。
 *
 * 三个分组（任务状态 / 下载队列 / 分类）的选中语义、计数和回调完全共用，行外壳也共用，
 * 只有中间的内容不同，所以外壳收成一个组件 —— 三份手写的 hover 一定会各自漂移。
 *
 * 两处刻意的取舍：
 * 1. **选中态在悬停时不改底色**。选中底 `selectedSurface` 上压着 `blue` 的计数文字，
 *    把底色往 `blue` 混会同时压低两种主题下的比值（浅色实测 4.51 → 4.18:1，跌破 4.5:1）。
 *    这与 `TaskRow`「选中压过悬停」的既有约定一致。
 * 2. **悬停时把图标/文字从 `muted` 提到 `ink`**。只靠 `surface3` 底色的话，浅色主题下
 *    它压在白底上只有 1.18:1，几乎看不见；而 `muted → ink` 是纯粹的对比度提升
 *    （浅色 6.41 → 15.1:1、深色 8.0 → 更高），不引入任何新令牌。
 */
@Composable
private fun NavRow(
    active: Boolean,
    onSelect: () -> Unit,
    modifier: Modifier = Modifier,
    content: @Composable RowScope.(highlighted: Boolean) -> Unit,
) {
    val feedback = rememberPressFeedback(
        restColor = if (active) selectedSurface else Color.Transparent,
        hoverColor = if (active) selectedSurface else surface3,
        pressScale = .99f,
    )
    Row(
        modifier
            .fillMaxWidth()
            .height(36.dp)
            .pressFeedback(feedback, RoundedCornerShape(Radius.md))
            .selectable(
                selected = active,
                interactionSource = feedback.interaction,
                indication = null,
                role = Role.Tab,
                onClick = onSelect,
            )
            .padding(horizontal = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        // 选中态保留固定宽度的左侧色条：即使不看颜色，形状也能标出当前入口。
        Box(
            Modifier.width(3.dp).height(18.dp)
                .clip(RoundedCornerShape(Radius.sm))
                .background(if (active) blue else Color.Transparent),
        )
        Spacer(Modifier.width(7.dp))
        content(active || feedback.hovered)
    }
}

@Composable private fun Sidebar(selected: TaskFilter, selectedCategory: TaskCategory?, selectedQueueId: String?, profiles: List<QueueProfileDto>, tasks: List<DownloadTask>, compact: Boolean, onSelected: (TaskFilter) -> Unit, onCategory: (TaskCategory) -> Unit, onQueue: (String) -> Unit, onManageQueues: () -> Unit) {
    // 进度和速度变化会替换任务快照；计数相同就不让它们触发侧栏刷新。
    val counts by remember(tasks) { taskNavigationCounts(tasks) }
    // 折叠态只换外壳，三个分组的选中语义、计数和回调完全共用，避免两套导航状态不同步。
    if (compact) {
        SidebarRail(selected, selectedCategory, selectedQueueId, profiles, counts, onSelected, onCategory, onQueue, onManageQueues)
        return
    }
    val scrollState = rememberScrollState()
    var queuesExpanded by remember { mutableStateOf(selectedQueueId != null || profiles.size > 1) }
    var categoriesExpanded by remember { mutableStateOf(true) }
    Column(Modifier.width(SIDEBAR_WIDTH).fillMaxHeight().background(rail).padding(horizontal = 9.dp, vertical = 11.dp)) {
        Text("任务状态", color = muted, fontSize = TypeScale.caption, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(horizontal = 10.dp, vertical = 5.dp))
        Box(Modifier.weight(1f).fillMaxWidth()) {
        Column(Modifier.fillMaxSize().verticalScroll(scrollState).padding(end = 5.dp)) {
        TaskFilter.entries.forEach { item ->
            val active = item == selected
            NavRow(active, { onSelected(item) }, Modifier.reportControlBounds("sidebar.status.${item.label}")) { highlighted ->
                Icon(categoryIcon(item), null, tint = if (active) blue else if (highlighted) ink else muted, modifier = Modifier.size(17.dp)); Spacer(Modifier.width(8.dp)); Text(item.label, color = if (highlighted) ink else muted, fontSize = TypeScale.body, fontWeight = if (active) FontWeight.SemiBold else FontWeight.Normal); Spacer(Modifier.weight(1f)); Text(counts.count(item).toString(), color = if (active) blue else muted, fontSize = TypeScale.caption, fontWeight = if (active) FontWeight.SemiBold else FontWeight.Normal)
            }
        }
        Spacer(Modifier.height(12.dp))
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            TextButton(onClick = { queuesExpanded = !queuesExpanded }, contentPadding = PaddingValues(horizontal = 10.dp), modifier = Modifier.weight(1f).reportControlBounds("sidebar.toggle_queues")) {
                Text("下载队列", color = muted, fontSize = TypeScale.caption, fontWeight = FontWeight.SemiBold)
                Spacer(Modifier.weight(1f))
                Icon(if (queuesExpanded) Icons.Outlined.ExpandLess else Icons.Outlined.ExpandMore, if (queuesExpanded) "收起队列" else "展开队列", tint = muted, modifier = Modifier.size(16.dp))
            }
            WorkbenchTooltip("管理队列") { IconButton(onClick = onManageQueues, modifier = Modifier.size(26.dp)) { Icon(Icons.Outlined.SettingsSuggest, "管理队列", tint = muted, modifier = Modifier.size(15.dp)) } }
        }
        if (queuesExpanded) profiles.sortedByDescending { it.priority }.forEach { profile ->
            val active = profile.id == selectedQueueId
            NavRow(active, { onQueue(profile.id) }, Modifier.reportControlBounds("sidebar.queue.${profile.name}")) { highlighted ->
                Icon(if (profile.enabled) Icons.AutoMirrored.Outlined.PlaylistPlay else Icons.Outlined.PauseCircleOutline, null, tint = if (active) blue else if (highlighted) ink else muted, modifier = Modifier.size(17.dp)); Spacer(Modifier.width(8.dp)); Text(profile.name, color = if (highlighted) ink else muted, fontSize = TypeScale.body, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f)); Text((counts.queues[profile.id] ?: 0).toString(), color = if (active) blue else faint, fontSize = TypeScale.caption)
            }
        }
        Spacer(Modifier.height(12.dp))
        TextButton(onClick = { categoriesExpanded = !categoriesExpanded }, contentPadding = PaddingValues(horizontal = 10.dp), modifier = Modifier.fillMaxWidth().reportControlBounds("sidebar.toggle_categories")) {
            Text(selectedCategory?.let { "分类 · ${it.label}" } ?: "分类", color = muted, fontSize = TypeScale.caption, fontWeight = FontWeight.SemiBold)
            Spacer(Modifier.weight(1f))
            Icon(if (categoriesExpanded) Icons.Outlined.ExpandLess else Icons.Outlined.ExpandMore, if (categoriesExpanded) "收起分类" else "展开分类", tint = muted, modifier = Modifier.size(16.dp))
        }
        if (categoriesExpanded) TaskCategory.entries.forEach { item ->
            val active = item == selectedCategory
            NavRow(active, { onCategory(item) }, Modifier.reportControlBounds("sidebar.category.${item.label}")) { highlighted ->
                Icon(categoryIcon(item), null, tint = if (active) blue else if (highlighted) ink else muted, modifier = Modifier.size(17.dp)); Spacer(Modifier.width(8.dp)); Text(item.label, color = if (highlighted) ink else muted, fontSize = TypeScale.body, fontWeight = if (active) FontWeight.SemiBold else FontWeight.Normal); Spacer(Modifier.weight(1f)); Text((counts.categories[item] ?: 0).toString(), color = if (active) blue else muted, fontSize = TypeScale.caption, fontWeight = if (active) FontWeight.SemiBold else FontWeight.Normal)
            }
        }
        }
        VerticalScrollbar(rememberScrollbarAdapter(scrollState), Modifier.align(Alignment.CenterEnd).fillMaxHeight().width(6.dp), style = workbenchScrollbarStyle())
        }
    }
}

/**
 * 窄屏（窗口内容区 < [SIDEBAR_COLLAPSE_WIDTH]）下的图标栏。
 *
 * 导航能力与展开态完全一致：三个分组的每一项都可点，计数改成图标右上角角标，
 * 分组标题换成细分隔线，文字标签由悬停 tooltip 补回。
 */
@Composable private fun SidebarRail(selected: TaskFilter, selectedCategory: TaskCategory?, selectedQueueId: String?, profiles: List<QueueProfileDto>, counts: TaskNavigationCounts, onSelected: (TaskFilter) -> Unit, onCategory: (TaskCategory) -> Unit, onQueue: (String) -> Unit, onManageQueues: () -> Unit) {
    val scrollState = rememberScrollState()
    Column(Modifier.width(SIDEBAR_RAIL_WIDTH).fillMaxHeight().background(rail).border(BorderStroke(1.dp, border)).padding(horizontal = 8.dp, vertical = 11.dp)) {
        Box(Modifier.weight(1f).fillMaxWidth()) {
            Column(Modifier.fillMaxSize().verticalScroll(scrollState).padding(end = 2.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                TaskFilter.entries.forEach { item ->
                    RailItem(categoryIcon(item), item.label, counts.count(item), item == selected, Modifier.reportControlBounds("rail.status.${item.label}")) { onSelected(item) }
                }
                RailGroupDivider()
                profiles.sortedByDescending { it.priority }.forEach { profile ->
                    RailItem(
                        if (profile.enabled) Icons.AutoMirrored.Outlined.PlaylistPlay else Icons.Outlined.PauseCircleOutline,
                        profile.name,
                        counts.queues[profile.id] ?: 0,
                        profile.id == selectedQueueId,
                        Modifier.reportControlBounds("rail.queue.${profile.name}"),
                    ) { onQueue(profile.id) }
                }
                RailGroupDivider()
                TaskCategory.entries.forEach { item ->
                    RailItem(categoryIcon(item), item.label, counts.categories[item] ?: 0, item == selectedCategory, Modifier.reportControlBounds("rail.category.${item.label}")) { onCategory(item) }
                }
            }
            VerticalScrollbar(rememberScrollbarAdapter(scrollState), Modifier.align(Alignment.CenterEnd).fillMaxHeight().width(6.dp), style = workbenchScrollbarStyle())
        }
        // 展开态里"管理队列"是分组标题右侧的小图标；折叠后没有标题行，
        // 固定到底部并加一条分隔线，免得和分类图标连成一片看不出边界。
        HorizontalDivider(color = border)
        Spacer(Modifier.height(6.dp))
        RailItem(Icons.Outlined.SettingsSuggest, "管理队列", null, false, Modifier.reportControlBounds("rail.manage"), onManageQueues)
    }
}

@Composable private fun RailGroupDivider() {
    HorizontalDivider(Modifier.padding(vertical = 7.dp), color = border)
}

/**
 * 图标栏的一项：40dp 方形点击区，图标居中，计数做右上角角标。
 *
 * 选中态同时给底色、图标色和左侧色条三条线索——底色在色觉障碍下不可靠，
 * 色条是形状线索，对应 WCAG 1.4.1"不能只靠颜色传达信息"。
 */
@Composable private fun RailItem(icon: ImageVector, label: String, count: Int?, active: Boolean, modifier: Modifier = Modifier, onClick: () -> Unit) {
    WorkbenchTooltip(if (count != null && count > 0) "$label · $count" else label) {
        // 与展开态 NavRow 同一套规则：悬停换底色 + 图标从 muted 提到 ink，选中态压过悬停。
        // 按压只缩到 .96f —— 40dp 的方框缩太多会让图标在格子间"跳"。
        val feedback = rememberPressFeedback(
            restColor = if (active) selectedSurface else Color.Transparent,
            hoverColor = if (active) selectedSurface else surface3,
            pressScale = .96f,
        )
        Box(Modifier.fillMaxWidth().height(40.dp), contentAlignment = Alignment.Center) {
            Box(
                modifier.fillMaxSize().pressFeedback(feedback, RoundedCornerShape(Radius.md))
                    .selectable(selected = active, interactionSource = feedback.interaction, indication = null, role = Role.Tab, onClick = onClick),
                contentAlignment = Alignment.Center,
            ) {
                Icon(icon, label, tint = if (active) blue else if (feedback.hovered) ink else muted, modifier = Modifier.size(17.dp))
            }
            if (active) Box(Modifier.align(Alignment.CenterStart).width(3.dp).height(18.dp).clip(RoundedCornerShape(Radius.sm)).background(blue))
            // 角标最多三位：四位数字会把 40dp 方框撑破，超过 99 一律显示 99+。
            // 描边取角标背后的底色（选中态是 selectedSurface，否则是 rail），
            // 等于把角标从图标上"挖"出来——没有这圈描边，角标会和图标线条糊在一起。
            if (count != null && count > 0) Box(
                Modifier.align(Alignment.TopEnd).padding(top = 2.dp)
                    .clip(RoundedCornerShape(Radius.sm))
                    .background(if (active) blue else surface3)
                    .border(1.5.dp, if (active) selectedSurface else rail, RoundedCornerShape(Radius.sm))
                    .padding(horizontal = 2.dp),
            ) { Text(if (count > 99) "99+" else count.toString(), color = if (active) onBlue else muted, fontSize = TypeScale.micro, fontWeight = FontWeight.SemiBold) }
        }
    }
}
private fun categoryIcon(filter: TaskFilter): ImageVector = when (filter) { TaskFilter.RUNNING -> Icons.Outlined.Downloading; TaskFilter.QUEUED -> Icons.Outlined.Schedule; TaskFilter.PAUSED -> Icons.Outlined.PauseCircle; TaskFilter.COMPLETED -> Icons.Outlined.CheckCircle; TaskFilter.FAILED -> Icons.Outlined.ErrorOutline; else -> Icons.Outlined.Folder }
private fun categoryIcon(category: TaskCategory): ImageVector = when (category) { TaskCategory.MEDIA -> Icons.Outlined.Image; TaskCategory.PROGRAM -> Icons.Outlined.Apps; TaskCategory.ARCHIVE -> Icons.Outlined.FolderZip; TaskCategory.OTHER -> Icons.AutoMirrored.Outlined.InsertDriveFile }

@Composable private fun ContentHeader(title: String, count: Int, selection: List<DownloadTask>, completedCount: Int, onClearSelection: () -> Unit, onRefresh: () -> Unit, onClearCompleted: () -> Unit, onMore: (String) -> Unit, onSelectedAction: (String) -> Unit) {
    var menuOpen by remember { mutableStateOf(false) }
    val selectedSources = selection.map { it.source }
    val clearHint = "清理当前列表中 $completedCount 个已完成任务；保留已下载文件"
    BoxWithConstraints(Modifier.height(42.dp).fillMaxWidth().background(if (selection.isNotEmpty()) selectedSurface else rail)) {
        val compact = maxWidth < 940.dp
        @Composable fun SelectedAction(icon: ImageVector, label: String, action: String) {
            val eligibleCount = selectedTaskActionTargets(selectedSources, action).size
            val hint = when (eligibleCount) {
                0 -> "所选任务当前不支持$label"
                selection.size -> "$label（${selection.size} 项）"
                else -> "$label（${selection.size} 项中有 $eligibleCount 项可执行）"
            }
            ToolbarIcon(icon, label, enabled = eligibleCount > 0, hint = hint) { onSelectedAction(action) }
        }
        Row(Modifier.fillMaxSize().padding(horizontal = if (compact) 8.dp else 14.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(title, color = ink, fontSize = TypeScale.body, fontWeight = FontWeight.SemiBold)
            Spacer(Modifier.width(8.dp)); Text("$count 项", color = faint, fontSize = TypeScale.caption)
            if (selection.isNotEmpty()) {
                Spacer(Modifier.width(if (compact) 8.dp else 16.dp))
                Text("已选 ${selection.size} 项", color = blue, fontSize = TypeScale.caption, fontWeight = FontWeight.SemiBold)
                ToolbarIcon(Icons.Outlined.Close, "取消选择", action = onClearSelection)
                SelectedAction(Icons.AutoMirrored.Outlined.DriveFileMove, "移动队列", "move_queue")
                selection.singleOrNull()?.takeIf { taskSupportsMediaActions(it.source) }?.let { task ->
                    SelectedAction(Icons.Outlined.PlayCircle, taskPlayLabel(task.source), "play")
                    SelectedAction(Icons.Outlined.Cast, "投屏", "cast")
                    SelectedAction(Icons.Outlined.Tv, "TVBox 推送", "push_tvbox")
                }
            } else if (!compact) {
                Spacer(Modifier.width(14.dp)); Text("选择任务后可进行批量操作", color = faint, fontSize = TypeScale.caption)
            }
            Spacer(Modifier.weight(1f))
            if (compact) {
                WorkbenchTooltip(clearHint) {
                    IconButton(onClick = onClearCompleted, enabled = completedCount > 0, modifier = Modifier.size(36.dp).reportControlBounds("content_header.clear_completed")) {
                        Icon(Icons.Outlined.DeleteSweep, clearHint, tint = muted, modifier = Modifier.size(18.dp))
                    }
                }
                ToolbarIcon(Icons.Outlined.Refresh, "刷新", action = onRefresh)
            } else {
                WorkbenchTooltip(clearHint) {
                    TextButton(onClick = onClearCompleted, enabled = completedCount > 0, contentPadding = PaddingValues(horizontal = 8.dp), hoverColor = surface3, pressedColor = pressedSurface, modifier = Modifier.reportControlBounds("content_header.clear_completed")) { Icon(Icons.Outlined.DeleteSweep, null, Modifier.size(15.dp)); Spacer(Modifier.width(4.dp)); Text("清理已完成", fontSize = TypeScale.caption) }
                }
                TextButton(onClick = onRefresh, contentPadding = PaddingValues(horizontal = 8.dp), hoverColor = surface3, pressedColor = pressedSurface, modifier = Modifier.reportControlBounds("content_header.refresh")) { Icon(Icons.Outlined.Refresh, null, Modifier.size(15.dp)); Spacer(Modifier.width(4.dp)); Text("刷新", fontSize = TypeScale.caption) }
            }
            Box { ToolbarIcon(Icons.Outlined.MoreHoriz, "更多操作") { menuOpen = true }; DropdownMenu(menuOpen, { menuOpen = false }, shape = RoundedCornerShape(Radius.lg), containerColor = dialogSurface, shadowElevation = Elevation.e2) { listOf("import" to "导入任务或种子", "export" to "导出任务列表", "reset_columns" to "恢复默认列宽", "update" to "检查更新", "cancel_power" to "取消完成后电源动作", "notices" to "通知中心", "about" to "关于 HLS Downloader", "exit" to "退出程序").forEach { (action, label) -> DropdownMenuItem(text = { Text(label, fontSize = TypeScale.body) }, onClick = { menuOpen = false; onMore(action) }) } } }
        }
    }
}
private data class TaskContextMenuRequest(
    val task: DownloadTask,
    val targets: List<DownloadTask>,
    val position: IntOffset,
)
@OptIn(ExperimentalComposeUiApi::class)
@Composable private fun TaskTable(tasks: List<DownloadTask>, selected: Set<String>, appIcon: ImageBitmap?, taskSort: String, onSelection: (Set<String>) -> Unit, onDetails: (String) -> Unit, onDeleteSelection: () -> Unit, onQueueMove: (String, Int) -> Unit, modifier: Modifier = Modifier, onBatchAction: (Set<String>, String) -> Unit, hasTasks: Boolean, onClearFilters: () -> Unit, onSort: (String) -> Unit, columnLayout: String, onColumnLayoutChange: (String) -> Unit, onAction: (String, String) -> Unit) {
    var anchorId by remember { mutableStateOf<String?>(null) }
    var tableFocused by remember { mutableStateOf(false) }
    var contextMenu by remember { mutableStateOf<TaskContextMenuRequest?>(null) }
    val focusRequester = remember { FocusRequester() }
    var columnDraft by remember(columnLayout) { mutableStateOf(columnLayout) }
    val savedColumnLayout by rememberUpdatedState(columnLayout)
    val commitColumnLayout by rememberUpdatedState(onColumnLayoutChange)
    val density = LocalDensity.current
    val dragTouchSlop = LocalViewConfiguration.current.touchSlop
    fun activate(task: DownloadTask) {
        val action = taskActivationAction(task.source)
        if (action == null) onDetails(task.id) else onAction(task.id, action)
    }
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
        // 详情弹窗的真实入口（行双击、行内菜单"详情与日志"）在本机夹具里都点不中，
        // 这里给测试 API 一个按行索引直接打开的钩子，让详情相关改动能拿到运行时证据。
        // 不靠坐标直接触发上下文菜单动作：菜单项 onClick 调的就是
        // performTaskActions(targets, action)，这里走同一条路，
        // 于是断线时批量操作的人话提示可以被一条调用验到（不需要点中菜单项）。
        UiTestState.installTaskContextActionInvoker { index, action ->
            val task = index?.let { tasks.getOrNull(it) } ?: return@installTaskContextActionInvoker
            val targets = if (task.id in selected) tasks.filter { it.id in selected } else listOf(task)
            onBatchAction(targets.mapTo(mutableSetOf()) { it.id }, action)
        }
        UiTestState.installTaskDetailsOpener { index, taskId, _, _ ->
            // 按 task_id 直接打开（目标任务可以不在列表里，用来触达"按 id 单独取"的兜底分支），
            // 否则按行索引解析出 id。与 installTaskSelector 的解析方式一致。
            val id = taskId ?: index?.let { tasks.getOrNull(it)?.id } ?: return@installTaskDetailsOpener
            onDetails(id)
        }
    }
    DisposableEffect(Unit) { onDispose { UiTestState.installTaskSelector(null); UiTestState.installTaskContextOpener(null); UiTestState.installTaskContextActionInvoker(null); UiTestState.installTaskDetailsOpener(null) } }
    BoxWithConstraints(
        modifier.fillMaxWidth().background(rail)
            .semantics { contentDescription = "下载任务列表，共 ${tasks.size} 项，已选择 ${selected.size} 项" }
            .focusRequester(focusRequester).onFocusChanged { tableFocused = it.isFocused }.focusable()
            .onPreviewKeyEvent { event ->
                if (event.type != KeyEventType.KeyDown) return@onPreviewKeyEvent false
                // Popup 的按键仍可能先经过列表的 preview 阶段，菜单必须优先于列表快捷键。
                if (contextMenu != null) {
                    if (event.key != Key.Escape) return@onPreviewKeyEvent false
                    contextMenu = null
                    UiTestState.closeContextMenu()
                    focusRequester.requestFocus()
                    return@onPreviewKeyEvent true
                }
                if (!tableFocused) return@onPreviewKeyEvent false
                when (resolveTaskTableKeyCommand(event.key, event.isCtrlPressed, selected.size, tasks.size)) {
                    TaskTableKeyCommand.SelectAll -> {
                        onSelection(tasks.mapTo(mutableSetOf()) { it.id }); anchorId = tasks.lastOrNull()?.id; true
                    }
                    TaskTableKeyCommand.ClearSelection -> { onSelection(emptySet()); anchorId = null; true }
                    TaskTableKeyCommand.DeleteSelection -> { onDeleteSelection(); true }
                    TaskTableKeyCommand.ActivateSelection -> { tasks.firstOrNull { it.id in selected }?.let(::activate); true }
                    TaskTableKeyCommand.MoveSelection -> {
                        val current = tasks.indexOfFirst { it.id == anchorId }.let { if (it < 0) 0 else it }
                        val next = (current + if (event.key == Key.DirectionDown) 1 else -1).coerceIn(0, tasks.lastIndex)
                        selectTask(tasks[next].id, event.isShiftPressed, false); true
                    }
                    null -> false
                }
            }.border(1.dp, if (tableFocused) blue.copy(alpha = .7f) else Color.Transparent)
    ) {
        val viewportWidth = maxWidth
        val columns = resolveTaskColumns(viewportWidth, columnDraft)
        val tableWidth = maxOf(maxWidth, columns.requiredWidth)
        val horizontalState = rememberScrollState()
        Box(Modifier.fillMaxSize().padding(bottom = if (tableWidth > maxWidth) 9.dp else 0.dp).horizontalScroll(horizontalState)) {
            Column(Modifier.width(tableWidth).fillMaxHeight()) {
                Row(Modifier.fillMaxWidth().height(36.dp).background(surface3).border(BorderStroke(1.dp, border)).padding(horizontal = 15.dp), verticalAlignment = Alignment.CenterVertically) {
                    columns.items.forEach { column ->
                        key(column.id) {
                            val currentColumn by rememberUpdatedState(column)
                            var resizeFocused by remember { mutableStateOf(false) }
                            Box(Modifier.width(column.width).height(36.dp)) {
                            if (column.id == "name") {
                                Row(Modifier.fillMaxSize().padding(end = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                                    Checkbox(tasks.isNotEmpty() && selected.size == tasks.size, { checked -> onSelection(if (checked) tasks.mapTo(mutableSetOf()) { it.id } else emptySet()) }, Modifier.size(18.dp), accessibilityLabel = "选择全部任务")
                                    Spacer(Modifier.width(8.dp))
                                    TaskHeader(column.label, Modifier.weight(1f), column.id, taskSort, onSort)
                                }
                            } else {
                                TaskHeader(column.label, Modifier.fillMaxSize().padding(end = 8.dp), column.id, taskSort, onSort)
                            }
                            Box(
                                Modifier.align(Alignment.CenterEnd).width(8.dp).fillMaxHeight()
                                    .pointerHoverIcon(PointerIcon(java.awt.Cursor.getPredefinedCursor(java.awt.Cursor.E_RESIZE_CURSOR)))
                                    .semantics { contentDescription = "${column.label}列宽；拖动或用左右方向键调整" }
                                    .onPreviewKeyEvent { event ->
                                        if (event.type != KeyEventType.KeyDown || event.key !in setOf(Key.DirectionLeft, Key.DirectionRight)) false
                                        else {
                                            val delta = if (event.key == Key.DirectionRight) 8.dp else (-8).dp
                                            columnDraft = resizeTaskColumnLayout(columnDraft, viewportWidth, column.id, currentColumn.width + delta)
                                            commitColumnLayout(columnDraft)
                                            true
                                        }
                                    }.onFocusChanged { resizeFocused = it.isFocused }.focusable()
                                    .pointerInput(column.id, density, viewportWidth) {
                                        var dragWidth = 0f
                                        detectHorizontalDragGestures(
                                            onDragStart = { dragWidth = currentColumn.width.value },
                                            onDragCancel = { columnDraft = savedColumnLayout },
                                            onDragEnd = { if (columnDraft != savedColumnLayout) commitColumnLayout(columnDraft) },
                                        ) { change, delta ->
                                            change.consume()
                                            dragWidth = (dragWidth + delta / density.density).coerceIn(if (column.id == "name") 160f else 48f, 800f)
                                            columnDraft = resizeTaskColumnLayout(columnDraft, viewportWidth, column.id, dragWidth.dp)
                                        }
                                    }, contentAlignment = Alignment.Center,
                            ) { Box(Modifier.width(if (resizeFocused) 2.dp else 1.dp).height(16.dp).background(if (resizeFocused) blue else border)) }
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
                            Text(if (hasTasks) "当前筛选下没有任务" else "这里还没有下载任务", color = ink, fontSize = TypeScale.title, fontWeight = FontWeight.SemiBold)
                            Spacer(Modifier.height(4.dp))
                            Text(if (hasTasks) "可调整搜索、状态、分类或队列筛选" else "点击工具栏“新建”或粘贴链接开始下载", color = muted, fontSize = TypeScale.body)
                            if (hasTasks) {
                                Spacer(Modifier.height(12.dp))
                                TextButton(onClick = onClearFilters, modifier = Modifier.reportControlBounds("tasks.reset_filters")) { Text("重置筛选", color = blue) }
                            }
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
                            .onPointerEvent(PointerEventType.Move, PointerEventPass.Main) { event ->
                                val start = dragStart ?: return@onPointerEvent
                                // 子控件（列宽、队列排序等）优先拥有已经接管的拖动。
                                if (event.changes.any { it.isConsumed }) {
                                    dragStart = null
                                    dragCurrent = null
                                    dragSelecting = false
                                    return@onPointerEvent
                                }
                                if (!event.buttons.isPrimaryPressed) return@onPointerEvent
                                val current = event.changes.firstOrNull()?.position ?: return@onPointerEvent
                                dragCurrent = current
                                if (!dragSelecting && (current - start).getDistance() > dragTouchSlop) dragSelecting = true
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
                                { activate(task) },
                                { onDetails(task.id) },
                                { delta -> onQueueMove(task.id, delta) },
                                { position ->
                                    val actions = batchTaskMenuActions(contextTargets.map { it.source })
                                    contextMenu = TaskContextMenuRequest(task, contextTargets, position)
                                    UiTestState.updateContextMenu(position, contextTargets.mapTo(mutableSetOf()) { it.id }, actions)
                                },
                            ) { action -> onAction(task.id, action) }
                        } }
                        // 不再用 tasks.size > 8 当"会不会溢出"的代理：窗口矮的时候 8 行就装不下，
                        // 实测 1440x500 下第 7 行起被切掉，而 8 条任务恰好不满足 > 8，于是一点滚动提示都没有。
                        // 是否溢出由 listState 自己说——VerticalScrollbar 在没有可滚动内容时本来就不绘制。
                        // LazyColumn 的滚动条走 rememberScrollbarAdapter(LazyListState) 这条路，
                        // 实测在这台机器上画不出来（1000 行、确定溢出，预期位置整段都是 rail 底色，
                        // 悬停也没有任何变化）。而 ScrollState 那条路是画得出来的（设置内容区实测有 389px 竖条）。
                        // 任务表是主界面、行数最多，不能没有滚动提示，所以这里用 listState 自己画。
                        // layoutInfo 每次测量都会更新；组合期读取会让列表重新组合、再测量，
                        // 即使任务静止也形成逐帧循环。滚动条只在绘制期订阅布局结果。
                        run {
                            val taskThumbColor = muted.copy(alpha = 0.42f)   // muted 是 @Composable get()，必须在 Composable 上下文里取
                            val taskThumbMin = 28.dp
                            Box(
                                Modifier.align(Alignment.CenterEnd).fillMaxHeight().width(7.dp).padding(vertical = 3.dp)
                                    .drawBehind {
                                        val layout = listState.layoutInfo
                                        val taskTotal = layout.totalItemsCount
                                        val taskVisible = layout.visibleItemsInfo.size
                                        if (taskTotal <= taskVisible || taskVisible == 0) return@drawBehind
                                        val track = size.height
                                        if (track <= 0f) return@drawBehind
                                        val ratio = taskVisible.toFloat() / taskTotal.toFloat()
                                        val thumbH = maxOf(track * ratio, taskThumbMin.toPx())
                                        val maxScroll = (taskTotal - taskVisible).coerceAtLeast(1).toFloat()
                                        val frac = (listState.firstVisibleItemIndex.toFloat() / maxScroll).coerceIn(0f, 1f)
                                        val y = (track - thumbH) * frac
                                        drawRoundRect(
                                            color = taskThumbColor,
                                            topLeft = Offset(0f, y),
                                            size = Size(size.width, thumbH),
                                            cornerRadius = CornerRadius(size.width / 2f),
                                        )
                                    },
                            )
                        }
                    }
                }
            }
        }
        if (tableWidth > maxWidth) HorizontalScrollbar(rememberScrollbarAdapter(horizontalState), Modifier.align(Alignment.BottomCenter).fillMaxWidth().height(9.dp))
        contextMenu?.let { request ->
            val menuFocus = remember { FocusRequester() }
            val closeMenu = {
                contextMenu = null
                UiTestState.closeContextMenu()
            }
            Popup(
                popupPositionProvider = ContextMenuPositionProvider(request.position),
                onDismissRequest = { closeMenu() },
                properties = PopupProperties(focusable = true),
            ) {
                Surface(
                    color = dialogSurface,
                    shape = RoundedCornerShape(Radius.lg),
                    shadowElevation = Elevation.e2,
                    border = BorderStroke(1.dp, border),
                    modifier = Modifier.width(220.dp),
                ) {
                    Column(
                        Modifier.padding(6.dp)
                            .focusRequester(menuFocus)
                            .onPreviewKeyEvent { event ->
                                if (event.type == KeyEventType.KeyDown && event.key == Key.Escape) {
                                    closeMenu()
                                    focusRequester.requestFocus()
                                    true
                                } else false
                            }
                            .focusable(),
                    ) {
                        TaskMenuEntries(
                            request.task,
                            { closeMenu() },
                            { onDetails(request.task.id) },
                            { action -> onBatchAction(request.targets.mapTo(mutableSetOf()) { it.id }, action) },
                            actions = batchTaskMenuActions(request.targets.map { it.source }),
                            includeDetails = request.targets.size == 1,
                        )
                    }
                    // 桌面 Popup 不会自动将键盘焦点交给无输入框的菜单。
                    LaunchedEffect(request) { menuFocus.requestFocus() }
                }
            }
        }
    }
}
@Composable
private fun TaskHeader(value: String, modifier: Modifier, field: String, taskSort: String, onSort: (String) -> Unit) {
    val sortable = field != "actions"
    val active = taskSort.substringBefore(':') == field
    // 只有可排序的列挂悬停反馈：不可排序的列（actions）挂上 hover 会让人以为点得动。
    // 表头不缩放 —— 文字在点击时缩一下会像"表格在抖"，反馈只走底色。
    // 但"只走底色"原先只走通了 hover：没给 pressedColor，按下时底色停在 hover 档不动，
    // 用 press（按住不抬）实测按住表头与悬停态像素级相同（差 0）。补上按下档，
    // 与 NavRow / 媒体轨带同一套"hover 一档、press 再深一档"的底色阶梯。
    val feedback = rememberPressFeedback(
        enabled = sortable,
        pressScale = 1f,
        pressedColor = if (active) selectedSurface else pressedSurface,
        hoverMillis = 120,
    )
    Row(
        modifier
            .height(36.dp)
            .then(
                if (sortable) {
                    Modifier
                        .background(feedback.background)
                        .hoverable(feedback.interaction)
                        .clickable(interactionSource = feedback.interaction, indication = null) { onSort(field) }
                } else {
                    Modifier
                },
            ),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(value, color = muted, fontSize = TypeScale.body, fontWeight = FontWeight.SemiBold, maxLines = 1)
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
@Composable private fun TaskRow(task: DownloadTask, columns: ResolvedTaskColumns, queueOrder: Boolean, isSelected: Boolean, select: (Boolean, Boolean) -> Unit, onActivate: () -> Unit, onDetails: () -> Unit, onQueueMove: (Int) -> Unit, onContextMenu: (IntOffset) -> Unit, onAction: (String) -> Unit) {
    var overflowOpen by remember { mutableStateOf(false) }
    var rowWindowOrigin by remember { mutableStateOf(Offset.Zero) }
    var rowSize by remember { mutableStateOf(IntSize.Zero) }
    var clickShift by remember { mutableStateOf(false) }
    var clickCtrl by remember { mutableStateOf(false) }
    val rowInteraction = remember { MutableInteractionSource() }
    val hovered by rowInteraction.collectIsHoveredAsState()
    val focused by rowInteraction.collectIsFocusedAsState()
    val pressed by rowInteraction.collectIsPressedAsState()
    // 行是自己手写 interaction 的（要同时吃 hover / 双击 / 右键 / 拖动），所以没有走
    // rememberPressFeedback——于是"按下"这一态原先整条漏掉了：hover 有反馈（rail→surface2），
    // 按下和抬起之间肉眼完全一样。按住任务行时没有"我按住了"的信号，而这是表格里最高频的手势。
    // 档位与 NavRow / 轨带选择一致：未选中按下压到 surface3（比 hover 深一档），
    // 已选中按下用 selectedSurface 朝 ink 混 5%——两种选中态下都留得下一格按压痕迹。
    val rowColorTarget = when {
        isSelected && pressed -> selectedSurface.blendToward(ink, .05f)
        isSelected -> selectedSurface
        pressed -> surface3
        hovered -> surface2
        else -> rail
    }
    val reduceMotion by MotionPreferences.reduceMotion
    val rowColor by animateColorAsState(
        targetValue = rowColorTarget,
        animationSpec = tween(motionDurationMillis(reduceMotion, 180)),
        label = "task-row-background",
    )
    val rowShape = RoundedCornerShape(Radius.sm)
    Row(Modifier.fillMaxWidth().height(52.dp).reportControlBounds("taskrow.${task.id}").padding(vertical = 1.dp, horizontal = 4.dp).onGloballyPositioned { rowWindowOrigin = it.positionInWindow(); rowSize = it.size }.background(rowColor, rowShape).border(1.dp, if (focused) ink else if (isSelected) blue.copy(alpha = .4f) else Color.Transparent, rowShape).clip(rowShape).hoverable(rowInteraction).semantics { selected = isSelected; contentDescription = taskAccessibilityLabel(task) }.onPointerEvent(PointerEventType.Press, PointerEventPass.Initial) {
        clickShift = it.keyboardModifiers.isPointerShiftPressed
        clickCtrl = it.keyboardModifiers.isPointerCtrlPressed
        if (it.button == PointerButton.Secondary || it.buttons.isSecondaryPressed) {
            if (!isSelected) select(false, false)
            onContextMenu(contextMenuWindowPosition(rowWindowOrigin, rowSize, it.changes.firstOrNull()?.position ?: Offset.Zero))
        }
    }.combinedClickable(interactionSource = rowInteraction, indication = null, onDoubleClick = { select(false, false); onActivate() }, onClick = { select(clickShift, clickCtrl) }).padding(horizontal = 11.dp), verticalAlignment = Alignment.CenterVertically) {
        columns.items.forEach { column ->
            key(column.id) {
                when (column.id) {
                    "name" -> Row(Modifier.width(column.width), verticalAlignment = Alignment.CenterVertically) { Checkbox(isSelected, { select(false, true) }, Modifier.size(18.dp), accessibilityLabel = "选择 ${task.filename}"); Spacer(Modifier.width(8.dp)); Icon(categoryIcon(taskCategory(task)), null, tint = muted, modifier = Modifier.size(18.dp)); Spacer(Modifier.width(8.dp)); WorkbenchTooltip("${task.filename}\n${safeResourceLocation(task.source.url)}") { Column(Modifier.weight(1f).padding(end = 8.dp)) { Text(task.filename, color = ink, maxLines = 1, overflow = TextOverflow.Ellipsis, fontSize = TypeScale.body, fontWeight = FontWeight.Medium); Text(listOf(taskProtocolLabel(task.source), taskExtensionLabel(task.source)).filter { it.isNotBlank() }.joinToString(" · "), color = faint, fontSize = TypeScale.micro, fontWeight = FontWeight.SemiBold) } } }
                    "progress" -> if (task.status == "已完成") {
                        Text("100%", color = successColor, fontSize = TypeScale.caption, modifier = Modifier.width(column.width))
                    } else Column(Modifier.width(column.width).padding(end = 12.dp)) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            val progressColor = when (task.status) {
                                "已暂停" -> warningColor
                                "失败", "下载失败" -> errorStrong
                                else -> blue
                            }
                            if (!taskProgressKnown(task.source) && isActiveTaskStatus(task.status)) LinearProgressIndicator(modifier = Modifier.weight(1f).height(5.dp).clip(RoundedCornerShape(Radius.sm)), color = progressColor, trackColor = surface3)
                            else LinearProgressIndicator(progress = { task.progress.coerceIn(0f, 1f) }, modifier = Modifier.weight(1f).height(5.dp).clip(RoundedCornerShape(Radius.sm)), color = progressColor, trackColor = surface3)
                            Spacer(Modifier.width(6.dp)); Text(taskProgressLabel(task), color = progressColor, fontSize = TypeScale.micro, fontWeight = FontWeight.SemiBold, modifier = Modifier.width(48.dp))
                        }
                    }
                    "status" -> Column(Modifier.width(column.width)) {
                        StatusBadge(task.status)
                        if (isActiveTaskStatus(task.status) && task.remaining.isNotBlank()) {
                            Text(task.remaining, color = muted, fontSize = TypeScale.micro, modifier = Modifier.padding(top = 1.dp))
                        }
                    }
                    "speed" -> Text(if (isActiveTaskStatus(task.status)) task.speed else "—", Modifier.width(column.width), color = ink, fontSize = TypeScale.caption)
                    "size" -> Text(task.source.totalBytes?.takeIf { it > 0 }?.let(::formatBytes) ?: "已下载 ${formatBytes(task.source.downloadedBytes)}", Modifier.width(column.width), color = ink, fontSize = TypeScale.caption, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    "actions" -> Row(Modifier.width(column.width), verticalAlignment = Alignment.CenterVertically) {
                        if (!columns.compact && queueOrder && task.status == "排队中") {
                            var dragY by remember { mutableFloatStateOf(0f) }
                            val rowHeightPx = with(LocalDensity.current) { 52.dp.toPx() }
                            Icon(Icons.Outlined.DragHandle, "拖动排序", tint = faint,
                                modifier = Modifier.size(34.dp).padding(8.dp).pointerInput(task.id, rowHeightPx) {
                                    detectDragGestures(
                                        onDragStart = { dragY = 0f },
                                        onDragCancel = { dragY = 0f },
                                        onDragEnd = {
                                            val delta = (dragY / rowHeightPx).roundToInt()
                                            if (delta != 0) onQueueMove(delta)
                                            dragY = 0f
                                        },
                                    ) { change, amount ->
                                        change.consume()
                                        dragY += amount.y
                                    }
                                })
                        } else {
                            val startAction = taskStartAction(task.source)
                            val primaryAction = when {
                                "pause" in task.source.availableActions -> "pause"
                                startAction != null -> startAction
                                taskSupportsMediaActions(task.source) && task.source.playbackReady -> "play"
                                "open_folder" in task.source.availableActions -> "open_folder"
                                else -> null
                            }
                            Box(Modifier.width(34.dp)) {
                                if (primaryAction != null) {
                                    val label = if (primaryAction == "play") taskPlayLabel(task.source) else actionLabel(primaryAction)
                                    val icon = when (primaryAction) {
                                        "pause" -> Icons.Outlined.Pause
                                        "retry" -> Icons.Outlined.Refresh
                                        "open_folder" -> Icons.Outlined.FolderOpen
                                        else -> Icons.Outlined.PlayArrow
                                    }
                                    WorkbenchTooltip(label) {
                                        IconButton(onClick = { onAction(primaryAction) }, modifier = Modifier.size(34.dp).reportControlBounds("task.action.${task.id}.$primaryAction")) {
                                            Icon(icon, "$label：${task.filename}", tint = blue, modifier = Modifier.size(18.dp))
                                        }
                                    }
                                }
                            }
                        }
                        Box(Modifier.width(36.dp)) { IconButton(onClick = { if (!isSelected) select(false, false); overflowOpen = true }, modifier = Modifier.size(34.dp)) { Icon(Icons.Outlined.MoreVert, "更多操作", tint = muted) }; DropdownMenu(expanded = overflowOpen, onDismissRequest = { overflowOpen = false }, shape = RoundedCornerShape(Radius.lg), containerColor = dialogSurface, shadowElevation = Elevation.e2) { TaskMenuEntries(task, { overflowOpen = false }, onDetails, onAction) } }
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
    val (destructiveActions, regularActions) = actions.partition { it == "delete" || it == "delete_files" }
    if (includeDetails) DropdownMenuItem(text = { Text("详情与日志", fontSize = TypeScale.body, color = ink) }, onClick = { dismiss(); onDetails() })
    regularActions.forEach { action ->
        DropdownMenuItem(text = { Text(if (action == "play") taskPlayLabel(task.source) else actionLabel(action), fontSize = TypeScale.body, color = ink) }, onClick = { dismiss(); onAction(action) })
    }
    if (includeDetails && task.status == "排队中") {
        HorizontalDivider(color = border)
        listOf("queue_up", "queue_down", "queue_top", "queue_bottom").forEach { action ->
            DropdownMenuItem(text = { Text(actionLabel(action), fontSize = TypeScale.body, color = ink) }, onClick = { dismiss(); onAction(action) })
        }
    }
    if (destructiveActions.isNotEmpty()) {
        if (includeDetails || regularActions.isNotEmpty()) HorizontalDivider(color = border)
        destructiveActions.forEach { action ->
            DropdownMenuItem(text = { Text(actionLabel(action), fontSize = TypeScale.body, color = errorStrong) }, onClick = { dismiss(); onAction(action) })
        }
    }
}
internal fun taskPlayLabel(task: TaskDto) = if (task.status.lowercase() in setOf("completed", "done")) "播放" else "边播边下"
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
    "进度 ${taskProgressLabel(task)}",
    if (isActiveTaskStatus(task.status)) "速度 ${task.speed}" else null,
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
internal fun formatBytes(bytes: Long) = when { bytes >= 1024L * 1024L * 1024L -> "%.2f GB".format(java.util.Locale.ROOT, bytes / 1024.0 / 1024.0 / 1024.0); bytes >= 1024L * 1024L -> "%.2f MB".format(java.util.Locale.ROOT, bytes / 1024.0 / 1024.0); bytes >= 1024L -> "%.1f KB".format(java.util.Locale.ROOT, bytes / 1024.0); else -> "${bytes.coerceAtLeast(0)} B" }
internal fun actionLabel(action: String) = mapOf("details" to "详情与日志", "start" to "开始", "pause" to "暂停", "resume" to "继续", "retry" to "重试", "cancel" to "取消", "open" to "打开文件", "open_folder" to "打开所在位置", "launch" to "运行文件", "copy_file" to "复制文件", "drag_file" to "拖出文件", "delete" to "删除任务", "delete_files" to "删除任务和文件", "play" to "播放", "cast" to "投屏", "push_tvbox" to "TVBox 推送", "move_queue" to "移动到队列", "queue_up" to "上移", "queue_down" to "下移", "queue_top" to "置顶", "queue_bottom" to "置底")[action] ?: action
@Composable internal fun StatusBadge(status: String, modifier: Modifier = Modifier) {
    val color = when(status) { "已完成" -> successColor; "失败", "下载失败" -> errorStrong; "已暂停" -> warningColor; "排队中" -> muted; else -> blue }
    val live = isActiveTaskStatus(status) || status == "排队中"
    Surface(modifier, color = color.copy(alpha = .08f), shape = RoundedCornerShape(Radius.sm)) {
        Row(Modifier.padding(horizontal = 7.dp, vertical = 3.dp), verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.size(6.dp).clip(RoundedCornerShape(Radius.pill)).background(color.copy(alpha = if (live) .95f else .7f)))
            Spacer(Modifier.width(5.dp))
            Text(status, color = color, fontSize = TypeScale.caption, fontWeight = FontWeight.Medium)
        }
    }
}
@Composable private fun ConnectionStatus(tasks: List<DownloadTask>, engine: String, extension: String, modifier: Modifier = Modifier) { val activeTasks = tasks.filter { isActiveTaskStatus(it.status) }; val active = activeTasks.size; val bytes = activeTasks.sumOf { it.speedBytes }; Row(modifier.height(28.dp).fillMaxWidth().background(rail).border(BorderStroke(1.dp, border)).padding(horizontal = 12.dp), verticalAlignment = Alignment.CenterVertically) { Text("活动任务 $active", color = muted, fontSize = TypeScale.caption); Spacer(Modifier.width(14.dp)); Text("队列 ${tasks.count { it.status == "排队中" }}", color = muted, fontSize = TypeScale.caption); Spacer(Modifier.width(14.dp)); Text("总速度 ${formatRate(bytes)}", color = blue, fontSize = TypeScale.caption, fontWeight = FontWeight.SemiBold); Spacer(Modifier.weight(1f)); Text(engine, color = if (engine == Product.engineConnected) successColor else warningColor, fontSize = TypeScale.caption, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis); Spacer(Modifier.width(14.dp)); Text(extension, color = if (connectionStateOf(extension)) successColor else faint, fontSize = TypeScale.caption, maxLines = 1, overflow = TextOverflow.Ellipsis) } }

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
                // Esc 关闭：`Popup` 的 `onDismissRequest` 只覆盖"点遮罩/失焦"，
                // 桌面端按键不会自动映射到它（实测：无此处理时 Esc 对
                // WorkbenchDialog 完全无效）。浏览器下载确认弹窗此前单独挂了
                // Enter/Esc 预览键处理，这里把 Esc 收进共享组件，其余所有
                // WorkbenchDialog（新建/批量/嗅探/设置/队列/详情/关于/通知中心/
                // 危险操作确认/设备选择）都获得同样的键盘出口。
                // 挂在根节点而不是内容列：内容可能有输入框（Enter 属于输入框），
                // 而 Esc 在模态对话框里永远只表示"关闭"。`dismissible=false`
                // 的忙碌态弹窗不注册，保持原先的防误触语义。
                .onPreviewKeyEvent { event ->
                    if (dismissible && event.type == KeyEventType.KeyDown && event.key == Key.Escape) {
                        onDismiss()
                        true
                    } else {
                        false
                    }
                }
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
            // 原先写的是 AnimatedVisibility(visible = true, …)：那是**死代码**。
            // 内部走 updateTransition(visible)，首帧的初始状态就等于目标状态，入场不会播；
            // 退出更不会播，因为父级 `if (flag) WorkbenchDialog(…)` 直接把整棵子树卸载了。
            // 改成状态从 false 起步，入场才真的会播（退出仍需父级配合保留挂载，本轮不做）。
            val reduceMotion by MotionPreferences.reduceMotion
            val enterState = remember { MutableTransitionState(false).apply { targetState = true } }
            AnimatedVisibility(
                visibleState = enterState,
                enter = fadeIn(tween(motionDurationMillis(reduceMotion, 130))) + scaleIn(tween(motionDurationMillis(reduceMotion, 130)), initialScale = .97f),
                exit = fadeOut(tween(motionDurationMillis(reduceMotion, 110))) + scaleOut(tween(motionDurationMillis(reduceMotion, 110)), targetScale = .97f),
            ) {
            Surface(
                modifier = Modifier
                    .width(bounds.width)
                    .heightIn(max = bounds.maxHeight)
                    .semantics { paneTitle = title },
                shape = RoundedCornerShape(Radius.lg),
                color = dialogSurface,
                shadowElevation = Elevation.e3,
                border = BorderStroke(1.dp, border),
            ) {
                Column(Modifier.fillMaxWidth().padding(18.dp)) {
                    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.Top) {
                        Column(Modifier.weight(1f)) {
                            Text(title, color = ink, fontSize = TypeScale.display, fontWeight = FontWeight.SemiBold)
                            Text(description, color = muted, fontSize = TypeScale.caption, modifier = Modifier.padding(top = 4.dp))
                        }
                        if (dismissible) IconButton(onClick = onDismiss, modifier = Modifier.size(32.dp)) {
                            Icon(Icons.Outlined.Close, "关闭", tint = muted, modifier = Modifier.size(18.dp))
                        }
                    }
                    Spacer(Modifier.height(13.dp))
                    HorizontalDivider(color = border)
                    Spacer(Modifier.height(13.dp))
                    // 内容区高度由弹窗上限（dialogBounds 统一压到 680dp 以内）决定，这里不再写死上限。
                    // 写死 420dp 时，字号刻度调整后设置页内容会超出，最后一行被滚动视口切掉半行。
                    // 滚动条：scrollState 提到 val，滚动条才能拿到同一个 state；内联 rememberScrollState() 时
                    // 任何滚动条都拿不到它，于是内容超出后用户看不到"下面还有"，只有半行被切掉的暗示。
                    Box(Modifier.fillMaxWidth().weight(1f, fill = false)) {
                        val dialogContentScroll = rememberScrollState()
                        Column(
                            Modifier
                                .fillMaxWidth()
                                .then(if (scrollable) Modifier.verticalScroll(dialogContentScroll).padding(end = 12.dp) else Modifier),
                            content = content,
                        )
                        if (scrollable) {
                            // 覆盖层不参与内容高度测量，短对话框按内容收缩。
                            Box(Modifier.matchParentSize()) {
                                VerticalScrollbar(
                                    rememberScrollbarAdapter(dialogContentScroll),
                                    Modifier.align(Alignment.CenterEnd).fillMaxHeight().width(6.dp),
                                    style = workbenchScrollbarStyle(),
                                )
                            }
                        }
                    }
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
@Composable internal fun DialogPrimary(label: String, enabled: Boolean = true, onClick: () -> Unit) = Button(onClick = onClick, enabled = enabled, shape = RoundedCornerShape(Radius.md), contentPadding = PaddingValues(horizontal = 15.dp), colors = ButtonDefaults.buttonColors(containerColor = blue, contentColor = onBlue), modifier = Modifier.reportControlBounds("dialog.primary.$label")) { Text(label, fontSize = TypeScale.body, fontWeight = FontWeight.SemiBold) }
@Composable internal fun DialogSecondary(label: String, enabled: Boolean = true, onClick: () -> Unit) = TextButton(onClick = onClick, enabled = enabled, contentPadding = PaddingValues(horizontal = 12.dp), hoverColor = surface3, pressedColor = pressedSurface, modifier = Modifier.reportControlBounds("dialog.secondary.$label")) { Text(label, fontSize = TypeScale.body, color = if (enabled) muted else faint) }
@Composable internal fun DialogLabel(value: String) = Text(value, color = muted, fontSize = TypeScale.caption, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(bottom = 5.dp))

/**
 * 分段按钮的按压缩放。`Button` 默认 .985f 是为了整行/整块目标不失真，
 * 但分段按钮只有约 24dp 高，1.5% 不足一个像素——用 `press` 动作量出来差分是 0。
 * 窄目标的规则沿用 [IconButton] / [RadioButton] 的既有约定：缩到 .92f。
 */
internal const val SEGMENT_SCALE = .92f

/**
 * 完成后动作的选项表。此前这份 `listOf(...)` 在 Main.kt 抄了两遍、SettingsV7.kt 一遍，
 * 共三份，加一个动作要改三处。
 */
internal val POWER_ACTION_OPTIONS: List<Pair<String, String>> = listOf(
    "none" to "无", "sleep" to "睡眠", "hibernate" to "休眠", "shutdown" to "关机",
)

/**
 * 确认弹窗里"X 秒后将…"用的动词。
 *
 * 刻意与 [POWER_ACTION_OPTIONS] 的标签不同措辞，这不是分叉：
 * "秒后将进入睡眠"读得通，"秒后将睡眠"不通；而"进入关机"反而是错的，
 * 所以关机不带前缀。两处是同一个枚举的两种渲染。
 */
internal fun powerActionVerb(kind: String): String = when (kind) {
    "shutdown" -> "关机"
    "sleep" -> "进入睡眠"
    "hibernate" -> "进入休眠"
    else -> "执行系统操作"
}
/**
 * 把任务操作失败翻译成用户能照着做的提示。
 *
 * 为什么需要：断线时 raw message 往往是 "Connection refused"、"system error: 2" 之类，
 * 用户看到也不知道该怎么办。断线是这台机器上最常见的一种失败
 * （引擎没起来、被杀了、正在重连），所以值得单独给一句话：
 * 操作没发出去、正在自动重连、稍后重试即可。
 */


internal fun describeTaskActionFailure(error: Throwable, fallback: String = "任务操作失败"): String {
    // 正常协程取消不是失败：runCatching 会把 CancellationException 也吞进来，
    // 这里必须原样抛回，否则一次正常取消会变成一条 error 通知。
    if (error is CancellationException) throw error
    val raw = error.message ?: return fallback
    val lowered = raw.lowercase()
    val connectionish =
        lowered.contains("refused") || lowered.contains("timed out") ||
            lowered.contains("timeout") || lowered.contains("closed") ||
            lowered.contains("reset") || lowered.contains("unreachable") ||
            lowered.contains("no such file") || lowered.contains("cannot find") ||
            lowered.contains("pipe") || lowered.contains("not connected")
    return if (connectionish) "下载引擎当前未连接，操作未能发送；正在自动重连，请稍后重试" else raw
}

/**
 * 非任务类操作的失败文案（导入/导出/设置/更新/投屏…）。
 *
 * 与 [describeTaskActionFailure] 唯一的区别：这里**不掺引擎连接语义**——
 * 读一个文件失败跟下载引擎连没连上是两件事，不能都回同一句话。
 * 但"正常取消必须抛回、不能被 runCatching 吞成一条 error 通知"同样成立。
 */
internal fun describeFailure(error: Throwable?, fallback: String): String {
    if (error is CancellationException) throw error
    return error?.message ?: fallback
}


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

@Composable private fun ExtensionDialog(status: String, onDismiss: () -> Unit) {
    val scope = rememberCoroutineScope()
    var message by remember { mutableStateOf("") }
    var working by remember { mutableStateOf(false) }
    WorkbenchDialog(onDismiss, "浏览器插件", "安装、重载并连接 Chromium 或 Firefox 插件", 620.dp, content = {
        Surface(Modifier.fillMaxWidth(), color = if (connectionStateOf(status)) successSurface else surface2, shape = RoundedCornerShape(Radius.md)) {
            Row(Modifier.padding(13.dp), verticalAlignment = Alignment.CenterVertically) {
                Icon(if (connectionStateOf(status)) Icons.Outlined.CheckCircle else Icons.Outlined.Extension, null, tint = if (connectionStateOf(status)) successColor else blue)
                Spacer(Modifier.width(10.dp))
                Column {
                    Text(status, color = if (connectionStateOf(status)) successColor else ink, fontSize = TypeScale.body, fontWeight = FontWeight.SemiBold)
                    Text(if (connectionStateOf(status)) "插件已可发送资源和接管下载" else "未连接时浏览器仍使用自己的下载器，不会静默丢失文件", color = muted, fontSize = TypeScale.caption)
                }
            }
        }
        Spacer(Modifier.height(14.dp))
        Text("插件功能", color = ink, fontSize = TypeScale.body, fontWeight = FontWeight.SemiBold)
        Text("识别 HLS、DASH、音视频轨道和网页播放器；接管普通下载；向桌面端发送 Cookie 授权后的请求身份；发起投屏和 TVBox 推送。", color = muted, fontSize = TypeScale.body, lineHeight = TypeScale.lineTitle, modifier = Modifier.padding(top = 5.dp))
        Spacer(Modifier.height(15.dp))
        Surface(Modifier.fillMaxWidth(), color = surface2, shape = RoundedCornerShape(Radius.md)) {
            Column(Modifier.padding(13.dp)) {
                Text("Chrome / Edge / Chromium", color = ink, fontSize = TypeScale.body, fontWeight = FontWeight.SemiBold)
                Text("打开扩展管理页和已解压的 ${Product.version} 插件目录；首次安装选择“加载已解压的扩展程序”，升级后点击“重新加载”。", color = muted, fontSize = TypeScale.caption, lineHeight = TypeScale.lineBody, modifier = Modifier.padding(top = 4.dp))
                Button(
                    onClick = {
                        working = true
                        message = ""
                        scope.launch {
                            val result = withContext(Dispatchers.IO) { openChromiumExtensionInstaller() }
                            working = false
                            message = if (result.ok) {
                                "${if (result.browserOpened) "已打开扩展管理页和插件目录。" else "已打开插件目录，请手动打开浏览器扩展管理页。"} 目录：${result.path}"
                            } else result.error
                        }
                    },
                    enabled = !working,
                    modifier = Modifier.padding(top = 10.dp),
                    shape = RoundedCornerShape(Radius.md),
                    colors = ButtonDefaults.buttonColors(containerColor = blue, contentColor = onBlue),
                ) {
                    Icon(Icons.Outlined.FolderOpen, null, Modifier.size(16.dp))
                    Spacer(Modifier.width(6.dp))
                    Text(if (working) "正在准备插件…" else "安装或重载 Chromium 插件", fontSize = TypeScale.body, fontWeight = FontWeight.SemiBold)
                }
            }
        }
        Spacer(Modifier.height(10.dp))
        Surface(Modifier.fillMaxWidth(), color = surface2, shape = RoundedCornerShape(Radius.md)) {
            Column(Modifier.padding(13.dp)) {
                Text("Firefox", color = ink, fontSize = TypeScale.body, fontWeight = FontWeight.SemiBold)
                Text("使用 Mozilla Add-ons 签名版，商店会自动更新；插件身份与 Native Messaging 注册保持一致。", color = muted, fontSize = TypeScale.caption, lineHeight = TypeScale.lineBody, modifier = Modifier.padding(top = 4.dp))
                TextButton(
                    onClick = {
                        working = true
                        message = ""
                        scope.launch {
                            val opened = withContext(Dispatchers.IO) { openFirefoxAddonPage() }
                            working = false
                            message = if (opened) "已用默认浏览器打开 Firefox Add-ons 安装页。" else "无法打开 Firefox Add-ons 安装页。"
                        }
                    },
                    enabled = !working,
                    contentPadding = PaddingValues(horizontal = 0.dp),
                    hoverColor = surface3,
                    pressedColor = pressedSurface,
                    modifier = Modifier.reportControlBounds("extension.firefox_install").padding(top = 5.dp),
                ) {
                    Icon(Icons.AutoMirrored.Outlined.OpenInNew, null, tint = blue, modifier = Modifier.size(16.dp))
                    Spacer(Modifier.width(6.dp))
                    Text("在 Firefox Add-ons 安装", color = blue, fontSize = TypeScale.body, fontWeight = FontWeight.SemiBold)
                }
            }
        }
        if (message.isNotBlank()) {
            Spacer(Modifier.height(10.dp))
            Surface(Modifier.fillMaxWidth(), color = blue.copy(alpha = .09f), shape = RoundedCornerShape(Radius.md)) {
                Text(message, color = ink, fontSize = TypeScale.caption, lineHeight = TypeScale.lineBody, modifier = Modifier.padding(10.dp))
            }
        }
        Text("安装或重载后请刷新正在播放的网页。Cookie 只在你对具体站点明确授权后读取。", color = faint, fontSize = TypeScale.caption, lineHeight = TypeScale.lineBody, modifier = Modifier.padding(top = 12.dp))
    }, actions = { DialogPrimary("完成", onClick = onDismiss) })
}

@Composable private fun AboutDialog(engine: String, extension: String, onOpenLogs: () -> Unit, onOpenHomepage: () -> Unit, onDismiss: () -> Unit) = WorkbenchDialog(onDismiss, "关于", "HLS Downloader", 470.dp, content = {
    Text("HLS Downloader ${Product.version}", color = ink, fontSize = TypeScale.display, fontWeight = FontWeight.SemiBold)
    Spacer(Modifier.height(6.dp))
    Text("Windows 桌面下载管理器：HLS/DASH 直播与点播、BT 磁力、HTTP、FTP/SFTP 与浏览器下载接管。", color = muted, fontSize = TypeScale.body, lineHeight = TypeScale.lineTitle)
    Spacer(Modifier.height(14.dp))
    AboutRow("核心协议", "hls-downloader-v7-core · v1")
    AboutRow("下载引擎", engine.substringAfter("·").trim())
    AboutRow("浏览器插件", extension.substringAfter("·").trim())
    Spacer(Modifier.height(6.dp))
    Text("日志与诊断数据仅保存在本机。", color = faint, fontSize = TypeScale.caption)
}, actions = {
    DialogSecondary("打开日志文件夹", onClick = onOpenLogs)
    DialogSecondary("项目主页", onClick = onOpenHomepage)
    DialogPrimary("关闭", onClick = onDismiss)
})

@Composable private fun AboutRow(label: String, value: String) = Row(Modifier.fillMaxWidth().padding(vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
    Text(label, color = muted, fontSize = TypeScale.body, modifier = Modifier.width(96.dp))
    Text(value, color = ink, fontSize = TypeScale.body, fontWeight = FontWeight.Medium)
}

@Composable private fun NoticesDialog(entries: List<NoticeHistoryEntry>, onClear: () -> Unit, onDismiss: () -> Unit) = WorkbenchDialog(onDismiss, "通知中心", "最近的提示与错误记录（最多保留 100 条）", 560.dp, content = {
    if (entries.isEmpty()) {
        Text("暂无通知。任务失败、引擎警告等记录会出现在这里。", color = muted, fontSize = TypeScale.body, modifier = Modifier.padding(vertical = 18.dp))
    } else {
        Box(Modifier.fillMaxWidth().heightIn(max = 380.dp)) {
            val noticesScroll = rememberScrollState()
            Column(Modifier.fillMaxWidth().verticalScroll(noticesScroll), verticalArrangement = Arrangement.spacedBy(7.dp)) {
                entries.asReversed().forEach { entry ->
                    val tone = if (entry.level == "error") errorStrong else if (entry.level == "success") successColor else blue
                    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.Top) {
                        Box(Modifier.padding(top = 5.dp).size(6.dp).clip(RoundedCornerShape(Radius.pill)).background(tone))
                        Spacer(Modifier.width(9.dp))
                        Text(java.time.Instant.ofEpochMilli(entry.at).atZone(java.time.ZoneId.systemDefault()).format(java.time.format.DateTimeFormatter.ofPattern("HH:mm:ss")), color = faint, fontSize = TypeScale.caption)
                        Spacer(Modifier.width(9.dp))
                        Text(entry.message, color = ink, fontSize = TypeScale.body, lineHeight = TypeScale.lineTitle, modifier = Modifier.weight(1f))
                    }
                }
            }
            VerticalScrollbar(
        rememberScrollbarAdapter(noticesScroll),
        Modifier.align(Alignment.CenterEnd).fillMaxHeight().width(6.dp),
        style = workbenchScrollbarStyle(),
    )
        }
    }
}, actions = { if (entries.isNotEmpty()) DialogSecondary("清空记录", onClick = onClear); DialogPrimary("关闭", onClick = onDismiss) })

@Composable private fun NoticeToast(signal: UiSignal.Notice, modifier: Modifier = Modifier, onDismiss: () -> Unit) {
        val tone = if (signal.level == "error") errorStrong else if (signal.level == "success") successColor else blue
        val reduceMotion by MotionPreferences.reduceMotion
        // 从上方 8dp 滑入 + 淡入：通知条贴在右上角，向下滑的动势和它的落点方向一致。
        val enterState = remember { MutableTransitionState(false).apply { targetState = true } }
        AnimatedVisibility(
            visibleState = enterState,
            modifier = modifier,
            enter = fadeIn(tween(motionDurationMillis(reduceMotion, 150))) + slideInVertically(tween(motionDurationMillis(reduceMotion, 150))) { -8 },
        ) {
        Surface(color = dialogSurface, shape = RoundedCornerShape(Radius.md), shadowElevation = Elevation.e2, border = BorderStroke(1.dp, border), modifier = Modifier.widthIn(min = 300.dp, max = 440.dp)) {
            Row(Modifier.padding(horizontal = 14.dp, vertical = 12.dp), verticalAlignment = Alignment.CenterVertically) {
                Icon(if (signal.level == "error") Icons.Outlined.ErrorOutline else Icons.Outlined.Info, null, tint = tone, modifier = Modifier.size(19.dp))
                Spacer(Modifier.width(10.dp)); Text(signal.message, color = ink, fontSize = TypeScale.body, modifier = Modifier.weight(1f), maxLines = 3, overflow = TextOverflow.Ellipsis)
                IconButton(onClick = onDismiss, modifier = Modifier.size(28.dp)) { Icon(Icons.Outlined.Close, "关闭", tint = muted, modifier = Modifier.size(16.dp)) }
            }
        }
        }
}

@Composable private fun TorrentSelectionDialog(data: TorrentProbeDto, busy: Boolean, onDismiss: () -> Unit, onCreate: (List<TorrentFileDto>) -> Unit) {
    var files by remember(data) { mutableStateOf(data.files.map { it.copy(selected = true) }) }
    val selectedBytes = files.filter { it.selected }.sumOf { it.size }
    WorkbenchDialog(onDismiss, "选择种子文件", "${data.name} · ${files.size} 个文件", 680.dp, dismissible = !busy, content = {
        Row(Modifier.fillMaxWidth().padding(bottom = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            Checkbox(files.all { it.selected }, { checked -> files = files.map { it.copy(selected = checked) } }, accessibilityLabel = "选择全部种子文件", enabled = !busy)
            Spacer(Modifier.width(6.dp)); Text("全选", color = ink, fontSize = TypeScale.body)
            Spacer(Modifier.weight(1f)); Text("已选 ${files.count { it.selected }} / ${files.size} · ${formatBytes(selectedBytes)}", color = muted, fontSize = TypeScale.caption)
        }
        HorizontalDivider(color = border)
        if (files.isEmpty()) {
            Column(Modifier.fillMaxWidth().padding(vertical = 34.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                Text("未取得种子文件清单", color = ink, fontSize = TypeScale.body, fontWeight = FontWeight.SemiBold)
                Text("磁力链接还没有返回完整元数据", color = muted, fontSize = TypeScale.caption, modifier = Modifier.padding(top = 6.dp))
            }
        } else {
            Box(Modifier.fillMaxWidth().heightIn(max = 390.dp)) {
                val torrentFilesScroll = rememberScrollState()
                Column(Modifier.fillMaxWidth().verticalScroll(torrentFilesScroll)) {
                    files.forEachIndexed { index, file ->
                        Row(Modifier.fillMaxWidth().padding(vertical = 5.dp), verticalAlignment = Alignment.CenterVertically) {
                            Checkbox(file.selected, { checked -> files = files.toMutableList().also { it[index] = file.copy(selected = checked) } }, accessibilityLabel = "选择 ${file.path}", enabled = !busy)
                            Spacer(Modifier.width(6.dp)); Column(Modifier.weight(1f)) {
                                Text(file.path, color = ink, fontSize = TypeScale.body, maxLines = 2, overflow = TextOverflow.Ellipsis)
                                Text(formatBytes(file.size), color = muted, fontSize = TypeScale.micro)
                            }
                        }
                    }
                }
                VerticalScrollbar(rememberScrollbarAdapter(torrentFilesScroll), Modifier.align(Alignment.CenterEnd).fillMaxHeight().width(6.dp), style = workbenchScrollbarStyle())
            }
        }
    }, actions = {
        DialogSecondary("取消", onClick = onDismiss, enabled = !busy)
        if (files.isEmpty()) {
            DialogPrimary(if (busy) "创建中…" else "直接创建磁力任务", !busy) { onCreate(emptyList()) }
        } else {
            DialogPrimary(if (busy) "创建中…" else "创建已选任务", !busy && files.any { it.selected }) { onCreate(files) }
        }
    })
}

@Composable private fun ProbeResultDialog(signal: UiSignal.Probe, busy: Boolean, onDismiss: () -> Unit, onCreate: (StreamVariantDto?) -> Unit) {
    var selected by remember(signal) { mutableStateOf<StreamVariantDto?>(signal.variants.maxByOrNull { it.bandwidth }) }
    WorkbenchDialog(onDismiss, "媒体资源", "选择清晰度、音轨或直接使用自动选择", 660.dp, dismissible = !busy, content = {
        DetailLine("资源链接", safeResourceLocation(signal.url))
        Spacer(Modifier.height(12.dp))
        if (signal.variants.isEmpty()) Text("下载引擎未发现可选择的媒体轨道，将使用自动识别结果。", color = muted, fontSize = TypeScale.body)
        else signal.variants.forEach { variant ->
            val active = selected == variant
            val feedback = rememberPressFeedback(
                restColor = if (active) selectedSurface else surface2,
                hoverColor = if (active) selectedSurface else surface3,
                pressedColor = if (active) selectedSurface else pressedSurface,
                enabled = !busy,
                pressScale = 1f,
            )
            Row(Modifier.fillMaxWidth().padding(vertical = 3.dp).pressFeedback(feedback, RoundedCornerShape(Radius.md)).clickable(enabled = !busy, interactionSource = feedback.interaction, indication = null) { selected = variant }.padding(12.dp), verticalAlignment = Alignment.CenterVertically) {
                RadioButton(active, { selected = variant }, accessibilityLabel = "选择 ${variant.label}", enabled = !busy); Spacer(Modifier.width(8.dp))
                Column(Modifier.weight(1f)) { Text(variant.label.ifBlank { variant.name.ifBlank { "媒体轨道" } }, color = ink, fontSize = TypeScale.body, fontWeight = FontWeight.SemiBold); Text(listOfNotNull(variant.height.takeIf { it > 0 }?.let { "${it}p" }, variant.bandwidth.takeIf { it > 0 }?.let { formatRate(it / 8) }, variant.kind.takeIf(String::isNotBlank)).joinToString(" · "), color = muted, fontSize = TypeScale.micro) }
            }
        }
    }, actions = { DialogSecondary("取消", onClick = onDismiss, enabled = !busy); DialogPrimary(if (busy) "创建中…" else "创建下载", !busy) { onCreate(selected) } })
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
    // 分类筛选条：悬停把底色从 surface2 提到 surface3，按下再缩一点（28dp 的小胶囊缩 3% 已足够可感）。
    val feedback = rememberPressFeedback(
        restColor = if (active) selectedSurface else surface2,
        hoverColor = if (active) selectedSurface else surface3,
        enabled = enabled,
        pressScale = .97f,
        hoverMillis = 120,
    )
    Surface(
        modifier = Modifier
            .height(28.dp)
            .graphicsLayer { scaleX = feedback.scale; scaleY = feedback.scale }
            .hoverable(feedback.interaction)
            .clickable(interactionSource = feedback.interaction, indication = null, enabled = enabled, onClick = onClick),
        color = feedback.background,
        shape = RoundedCornerShape(Radius.md),
        border = BorderStroke(1.dp, if (active) blue.copy(alpha = .55f) else border),
    ) {
        Box(Modifier.padding(horizontal = 9.dp), contentAlignment = Alignment.Center) {
            Text(label, color = foreground, fontSize = TypeScale.micro, fontWeight = if (active) FontWeight.SemiBold else FontWeight.Normal)
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
                Text("页面未发现可下载的静态文件链接。", color = muted, fontSize = TypeScale.body)
                Text("这里只读取当前页面 HTML，不执行脚本，也不继续打开子页面。", color = faint, fontSize = TypeScale.caption, modifier = Modifier.padding(top = 6.dp))
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
                    Text("已选 ${selectedVisible.size} / ${visible.size}", color = muted, fontSize = TypeScale.micro)
                }
                Spacer(Modifier.height(9.dp))
                Column(Modifier.fillMaxWidth().border(1.dp, border, RoundedCornerShape(Radius.md)).clip(RoundedCornerShape(Radius.md))) {
                    visible.forEachIndexed { index, item ->
                        val isPicked = item.url in selected
                        // 整行通宽（约 760dp），不做缩放：横向缩 1% 就是 7px 的内容位移，
                        // 看起来像布局在抖。宽行一律用底色档位表达按压（pressScale = 1f）。
                        val feedback = rememberPressFeedback(
                            restColor = if (isPicked) selectedSurface.copy(alpha = .55f) else Color.Transparent,
                            hoverColor = if (isPicked) selectedSurface.copy(alpha = .55f) else surface3,
                            pressedColor = if (isPicked) selectedSurface.copy(alpha = .55f) else pressedSurface,
                            pressScale = 1f,
                            hoverMillis = 120,
                        )
                        Row(
                            Modifier.fillMaxWidth()
                                .pressFeedback(feedback)
                                .clickable(interactionSource = feedback.interaction, indication = null) {
                                    selected = if (item.url in selected) selected - item.url else selected + item.url
                                }
                                .padding(horizontal = 9.dp, vertical = 7.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Checkbox(
                                item.url in selected,
                                { checked -> selected = if (checked) selected + item.url else selected - item.url },
                                accessibilityLabel = "选择 ${item.filename.ifBlank { item.url }}",
                            )
                            Spacer(Modifier.width(7.dp))
                            Column(Modifier.weight(1f)) {
                                Text(item.filename.ifBlank { item.url.substringAfterLast('/') }, color = ink, fontSize = TypeScale.body, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                val details = buildList {
                                    add(harvestCategories.firstOrNull { it.first == item.category }?.second ?: "文件")
                                    item.extension.trimStart('.').takeIf(String::isNotBlank)?.let { add(".$it") }
                                    if (item.size > 0) add(formatBytes(item.size)) else if (probed) add("大小未知")
                                }
                                Text(details.joinToString(" · "), color = muted, fontSize = TypeScale.micro, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            }
                            WorkbenchTooltip(item.url) {
                                Icon(Icons.Outlined.Link, null, tint = faint, modifier = Modifier.size(16.dp))
                            }
                        }
                        if (index < visible.lastIndex) HorizontalDivider(color = border)
                    }
                }
                if (visible.isEmpty()) Text("当前筛选条件下没有资源。", color = muted, fontSize = TypeScale.caption, modifier = Modifier.padding(top = 9.dp))
            }
            Spacer(Modifier.height(12.dp))
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Column(Modifier.weight(1f)) {
                    DialogLabel("Referer（可选）")
                    OutlinedTextField(referer, { referer = it }, Modifier.fillMaxWidth(), singleLine = true, shape = RoundedCornerShape(Radius.md), isError = !refererValid)
                }
                Column(Modifier.width(120.dp)) {
                    DialogLabel("并发")
                    OutlinedTextField(concurrency, { concurrency = it.filter(Char::isDigit) }, Modifier.fillMaxWidth(), singleLine = true, shape = RoundedCornerShape(Radius.md), isError = parsedConcurrency == 0L)
                }
            }
        },
        actions = {
            DialogSecondary("取消", onClick = onDismiss)
            DialogPrimary("创建 ${selectedVisible.size} 个下载", selectedVisible.isNotEmpty() && refererValid && parsedConcurrency > 0L && !probing) {
                onCreate(HarvestCreateRequest(selectedVisible.map { it.url }, effectiveReferer, parsedConcurrency))
            }
        },
    )
}

@Composable private fun DuplicateDialog(signal: UiSignal.Duplicate, onDismiss: () -> Unit, onConfirm: () -> Unit) = WorkbenchDialog(onDismiss, "发现重复任务", signal.message, 520.dp, content = {
    Text("已有任务可以${actionLabel(signal.action)}，继续将对现有任务执行该操作。", color = muted, fontSize = TypeScale.body)
}, actions = { DialogSecondary("取消", onClick = onDismiss); DialogPrimary(actionLabel(signal.action), onClick = onConfirm) })

@Composable private fun UpdateDialog(signal: UiSignal.Update, busy: Boolean, onDismiss: () -> Unit, onRelease: () -> Unit, onDownload: () -> Unit) = WorkbenchDialog(onDismiss, "发现新版本", "安全覆盖升级", 590.dp, dismissible = !busy, content = {
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        UpdateVersionCell("当前版本", signal.current, Modifier.weight(1f))
        Icon(Icons.AutoMirrored.Outlined.ArrowForward, null, tint = muted, modifier = Modifier.align(Alignment.CenterVertically).size(18.dp))
        UpdateVersionCell("可用版本", signal.latest, Modifier.weight(1f), emphasized = true)
    }
    Spacer(Modifier.height(12.dp))
    if (signal.notes.isNotBlank()) {
        Column(verticalArrangement = Arrangement.spacedBy(5.dp)) {
            signal.notes.lineSequence().filter { it.isNotBlank() }.take(7).forEach { line ->
                val heading = line.trimStart().startsWith("#")
                val text = line.trim().trimStart('#').trim().removePrefix("- ").removePrefix("* ")
                    .replace(Regex("\\[([^]]+)\\]\\([^)]+\\)"), "$1").replace("**", "").replace("`", "")
                Text(text, color = if (heading) ink else muted, fontSize = TypeScale.body,
                    fontWeight = if (heading) FontWeight.SemiBold else FontWeight.Normal,
                    maxLines = 2, overflow = TextOverflow.Ellipsis)
            }
        }
    }
    Spacer(Modifier.height(12.dp))
    Surface(Modifier.fillMaxWidth(), color = surface2, shape = RoundedCornerShape(Radius.md)) {
        Column(Modifier.padding(horizontal = 13.dp, vertical = 11.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Text(signal.installerName.ifBlank { "未找到可自动安装的 Windows x64 MSI" }, color = ink, fontSize = TypeScale.caption, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(if (signal.installerSize > 0) "${formatBytes(signal.installerSize)} · SHA-256 ${if (signal.sha256Verified) "发布方摘要已确认" else "摘要缺失"}" else "自动升级已停用，请从发布页人工核验", color = if (signal.sha256Verified) successColor else warningColor, fontSize = TypeScale.micro)
        }
    }
    Spacer(Modifier.height(12.dp)); Text("安装程序将覆盖当前版本，配置、任务数据库和下载文件会保留。开始安装前会安全暂停活动任务并保存断点；升级不会自动重启 Windows。", color = muted, fontSize = TypeScale.caption, lineHeight = TypeScale.lineBody)
}, actions = {
    DialogSecondary("稍后", onClick = onDismiss)
    if (signal.releaseUrl.isNotBlank()) DialogSecondary("查看发布页", onClick = onRelease)
    DialogPrimary(if (busy) "下载并校验中…" else "下载更新", !busy && signal.installerSize > 0 && signal.sha256Verified && signal.installerName.endsWith(".msi", true), onDownload)
})

@Composable private fun UpdatePreparedDialog(signal: UiSignal.UpdatePrepared, busy: Boolean, onDismiss: () -> Unit, onInstall: () -> Unit) = WorkbenchDialog(
    onDismiss = onDismiss,
    title = "更新已准备",
    description = "版本 ${signal.latest} 已完成完整性与产品身份校验",
    width = 590.dp,
    dismissible = !busy,
    content = {
        Surface(Modifier.fillMaxWidth(), color = surface2, shape = RoundedCornerShape(Radius.md)) {
            Column(Modifier.padding(horizontal = 13.dp, vertical = 11.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                Row(Modifier.fillMaxWidth()) {
                    Text("产品", color = muted, fontSize = TypeScale.micro, modifier = Modifier.width(82.dp))
                    Text("${signal.productName} ${signal.productVersion}", color = ink, fontSize = TypeScale.caption, fontWeight = FontWeight.Medium)
                }
                Row(Modifier.fillMaxWidth()) {
                    Text("安装包", color = muted, fontSize = TypeScale.micro, modifier = Modifier.width(82.dp))
                    Text(signal.installerPath.substringAfterLast('/').substringAfterLast('\\'), color = ink, fontSize = TypeScale.caption, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
                Row(Modifier.fillMaxWidth()) {
                    Text("SHA-256", color = muted, fontSize = TypeScale.micro, modifier = Modifier.width(82.dp))
                    Text(signal.sha256.take(16) + "…" + signal.sha256.takeLast(8), color = successColor, fontSize = TypeScale.micro)
                }
            }
        }
        Spacer(Modifier.height(12.dp))
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.Top) {
            Icon(Icons.Outlined.RestartAlt, null, tint = blue, modifier = Modifier.size(18.dp))
            Spacer(Modifier.width(9.dp))
            Text("开始后，下载引擎会暂停活动任务并保存断点，工作台和临时窗口随后关闭。独立更新助手会等待文件释放、执行覆盖安装并重新打开 HLS Downloader。", color = muted, fontSize = TypeScale.caption, lineHeight = TypeScale.lineBody)
        }
    },
    actions = {
        DialogSecondary("稍后安装", onClick = onDismiss)
        DialogPrimary(if (busy) "正在保存断点…" else "立即安装并重启", !busy, onInstall)
    },
)

@Composable private fun UpdateVersionCell(label: String, value: String, modifier: Modifier = Modifier, emphasized: Boolean = false) {
    Column(modifier.clip(RoundedCornerShape(Radius.md)).background(if (emphasized) selectedSurface else surface2).padding(horizontal = 13.dp, vertical = 11.dp)) {
        Text(label, color = muted, fontSize = TypeScale.micro)
        Text(value.ifBlank { "-" }, color = if (emphasized) blue else ink, fontSize = TypeScale.display, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(top = 2.dp))
    }
}

@Composable private fun DestructiveConfirmDialog(request: DestructiveRequest, onDismiss: () -> Unit, onConfirm: () -> Unit) = WorkbenchDialog(
    onDismiss, if (request.action == "delete_files") "删除任务和文件" else "删除任务", "此操作将影响 ${request.taskIds.size} 个任务", 520.dp,
    content = {
        Surface(color = errorSurface, shape = RoundedCornerShape(Radius.md), modifier = Modifier.fillMaxWidth()) {
            Row(Modifier.padding(13.dp), verticalAlignment = Alignment.Top) { Icon(Icons.Outlined.WarningAmber, null, tint = errorStrong); Spacer(Modifier.width(10.dp)); Text(if (request.action == "delete_files") "任务记录、已下载文件和过程文件都会删除。" else "只删除任务记录，已完成文件将保留。", color = errorBody, fontSize = TypeScale.body, lineHeight = TypeScale.lineTitle) }
        }
    }, actions = { DialogSecondary("取消", onClick = onDismiss); Button(onClick = onConfirm, colors = ButtonDefaults.buttonColors(containerColor = destructiveFill, contentColor = Color.White), shape = RoundedCornerShape(Radius.md), modifier = Modifier.reportControlBounds("dialog.primary.确认删除")) { Text("确认删除", fontSize = TypeScale.body, fontWeight = FontWeight.SemiBold) } },
)

@Composable private fun PowerActionDialog(signal: UiSignal.PowerPending, onCancel: () -> Unit, onConfirm: () -> Unit) = WorkbenchDialog(
    onDismiss = onCancel,
    title = "下载完成",
    description = "完成后操作确认",
    width = 480.dp,
    content = {
        val action = powerActionVerb(signal.action)
        Text("${signal.title.ifBlank { "下载任务" }} 已完成。${signal.delaySeconds} 秒后将$action。", color = ink, fontSize = TypeScale.body, lineHeight = TypeScale.lineTitle)
        Text("可以立即执行，或取消本次操作。", color = muted, fontSize = TypeScale.caption, modifier = Modifier.padding(top = 8.dp))
    },
    actions = { DialogSecondary("取消操作", onClick = onCancel); DialogPrimary("立即执行", onClick = onConfirm) },
)

@Composable private fun CastSessionHud(signal: UiSignal.Cast, task: DownloadTask?, busy: Boolean, onCopy: (String) -> Unit, onAction: (String, Long) -> Unit) {
    var scrubPosition by remember(signal.positionSeconds) { mutableFloatStateOf(signal.positionSeconds.toFloat()) }
    val offline = signal.status.equals("OFFLINE", true)
    val controllable = signal.supportedActions.contains("play") && !offline
        Surface(color = dialogSurface, shape = RoundedCornerShape(Radius.lg), shadowElevation = Elevation.e3, border = BorderStroke(1.dp, border), modifier = Modifier.width(420.dp)) {
            Column(Modifier.padding(14.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Surface(Modifier.size(34.dp), color = selectedSurface, shape = RoundedCornerShape(Radius.md)) { Box(contentAlignment = Alignment.Center) { Icon(Icons.Outlined.CastConnected, null, tint = blue, modifier = Modifier.size(19.dp)) } }
                    Spacer(Modifier.width(10.dp)); Column(Modifier.weight(1f)) {
                        Text(signal.title.ifBlank { task?.filename ?: "正在投屏" }, color = ink, fontSize = TypeScale.body, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        val protocol = castProtocolLabel(signal.deviceKind)
                        Text(listOf(signal.device, protocol.takeUnless { signal.device.contains(it, ignoreCase = true) }.orEmpty(), castStatusLabel(signal.status)).filter(String::isNotBlank).joinToString(" · "), color = muted, fontSize = TypeScale.micro, maxLines = 1, overflow = TextOverflow.Ellipsis)
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
                        Slider(value = scrubPosition.coerceIn(0f, signal.durationSeconds.toFloat()), onValueChange = { scrubPosition = it }, onValueChangeFinished = { onAction("seek_to", scrubPosition.toLong()) }, valueRange = 0f..signal.durationSeconds.toFloat(), modifier = Modifier.fillMaxWidth().height(28.dp), colors = SliderDefaults.colors(thumbColor = blue, activeTrackColor = blue, inactiveTrackColor = surface3), accessibilityLabel = "投屏播放位置")
                        Row(Modifier.fillMaxWidth()) { Text(formatClock(scrubPosition.toLong()), color = muted, fontSize = TypeScale.micro); Spacer(Modifier.weight(1f)); Text(formatClock(signal.durationSeconds), color = muted, fontSize = TypeScale.micro) }
                    }
                } else {
                    Spacer(Modifier.height(9.dp)); Text(if (offline) "与接收设备的连接已中断。检查电视和局域网后点击重新连接。" else if (signal.deviceKind == "tvbox") "已推送到 TVBox。此类设备没有统一的远程控制协议，请在电视端操作。" else "局域网播放地址已发布，请在接收设备中控制播放。", color = if (offline) errorStrong else muted, fontSize = TypeScale.micro, lineHeight = TypeScale.lineMicro)
                }
                if (signal.deviceKind == "lan" && signal.mediaUrl.isNotBlank()) {
                    Spacer(Modifier.height(10.dp))
                    Row(Modifier.fillMaxWidth().background(surface2, RoundedCornerShape(Radius.md)).border(1.dp, border, RoundedCornerShape(Radius.md)).clip(RoundedCornerShape(Radius.md)).padding(start = 10.dp, end = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                        Text(signal.mediaUrl, color = muted, fontSize = TypeScale.micro, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                        IconButton(onClick = { onCopy(signal.mediaUrl) }, modifier = Modifier.size(32.dp)) { Icon(Icons.Outlined.ContentCopy, "复制播放地址", tint = blue, modifier = Modifier.size(16.dp)) }
                    }
                }
                task?.let {
                    Spacer(Modifier.height(10.dp)); HorizontalDivider(color = border); Spacer(Modifier.height(9.dp))
                    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) { Text("下载进度", color = muted, fontSize = TypeScale.micro); Spacer(Modifier.weight(1f)); Text("${(it.progress * 100).toInt()}% · ${it.speed}", color = muted, fontSize = TypeScale.micro) }
                    Spacer(Modifier.height(5.dp)); LinearProgressIndicator(progress = { it.progress }, modifier = Modifier.fillMaxWidth().height(4.dp).clip(RoundedCornerShape(Radius.tiny)), color = blue, trackColor = surface3)
                }
            }
        }
}

private fun formatClock(seconds: Long): String {
    val value = seconds.coerceAtLeast(0)
    return if (value >= 3600) "%d:%02d:%02d".format(java.util.Locale.ROOT, value / 3600, (value / 60) % 60, value % 60) else "%02d:%02d".format(java.util.Locale.ROOT, value / 60, value % 60)
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
