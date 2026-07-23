package com.ciaooo55.hlsdownloader

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Archive
import androidx.compose.material.icons.filled.CheckCircle
import androidx.compose.material.icons.filled.ClearAll
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material.icons.filled.Description
import androidx.compose.material.icons.filled.Downloading
import androidx.compose.material.icons.filled.FolderOpen
import androidx.compose.material.icons.filled.InsertDriveFile
import androidx.compose.material.icons.filled.Movie
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.filled.Pause
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material.icons.filled.RestartAlt
import androidx.compose.material.icons.filled.Search
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material.icons.filled.Terminal
import androidx.compose.material.icons.filled.ViewList
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Checkbox
import androidx.compose.material3.Divider
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.FilledTonalIconButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.isCtrlPressed
import androidx.compose.ui.input.key.isShiftPressed
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.type
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp

data class KeyboardSelection(val ctrl: Boolean = false, val shift: Boolean = false)

@Composable
fun MainContent(
    state: AppState,
    dark: Boolean,
    onToggleTheme: () -> Unit,
    keys: KeyboardSelection,
) {
    var showAdd by remember { mutableStateOf(false) }
    var addInitial by remember { mutableStateOf("") }
    var showBatch by remember { mutableStateOf(false) }
    var showSettings by remember { mutableStateOf(false) }
    var showIntegration by remember { mutableStateOf(false) }
    var detailTask by remember { mutableStateOf<TaskItem?>(null) }
    var logTask by remember { mutableStateOf<TaskItem?>(null) }
    var confirmDelete by remember { mutableStateOf<Pair<List<TaskItem>, Boolean>?>(null) }

    Box(Modifier.fillMaxSize()) {
        Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
            Column(Modifier.fillMaxSize()) {
            TopToolbar(
                state = state,
                dark = dark,
                onToggleTheme = onToggleTheme,
                onNew = { addInitial = ""; showAdd = true },
                onPaste = { addInitial = state.pasteUrl(); showAdd = true },
                onBatch = { showBatch = true },
                onSettings = { showSettings = true },
                onIntegration = { showIntegration = true },
                onLog = { logTask = state.selectedTasks.firstOrNull() },
                onDelete = { if (state.selectedTasks.isNotEmpty()) confirmDelete = state.selectedTasks to false },
            )
            if (state.error.isNotBlank()) ErrorBar(state.error, state::dismissError)
            Row(Modifier.weight(1f).fillMaxWidth()) {
                Sidebar(state)
                Column(Modifier.weight(1f).fillMaxHeight()) {
                    ContentHeader(state)
                    TaskTable(
                        state,
                        keys,
                        onDetails = { detailTask = it },
                        onLog = { logTask = it },
                        onDelete = { task, files -> confirmDelete = listOf(task) to files },
                    )
                }
            }
            StatusBar(state)
            }
        }

        if (showAdd) AddDownloadDialog(state, addInitial, onClose = { showAdd = false })
        if (showBatch) BatchDownloadDialog(state, onClose = { showBatch = false })
        if (showSettings) SettingsDialog(state, onClose = { showSettings = false })
        if (showIntegration) IntegrationDialog(state, onClose = { showIntegration = false })
        detailTask?.let { task ->
            state.tasks.firstOrNull { it.id == task.id }?.let { current ->
                TaskDetailsDialog(state, current, onClose = { detailTask = null }, onLog = { logTask = current })
            } ?: run { detailTask = null }
        }
        logTask?.let { LogDialog(state, it, onClose = { logTask = null }) }
        confirmDelete?.let { (targets, files) ->
            AlertDialog(
                onDismissRequest = { confirmDelete = null },
                title = { Text(if (files) "删除任务及文件" else "删除任务记录") },
                text = { Text(if (files) "将停止所选任务并删除已经写入的文件，此操作无法撤销。" else "只删除任务记录，已完成文件会保留。") },
                confirmButton = {
                    Button(onClick = { state.perform("delete", targets, files); confirmDelete = null }, shape = DesktopControlShape) { Text("删除") }
                },
                dismissButton = { TextButton(onClick = { confirmDelete = null }) { Text("取消") } },
            )
        }
    }
}

@Composable
private fun TopToolbar(
    state: AppState,
    dark: Boolean,
    onToggleTheme: () -> Unit,
    onNew: () -> Unit,
    onPaste: () -> Unit,
    onBatch: () -> Unit,
    onSettings: () -> Unit,
    onIntegration: () -> Unit,
    onLog: () -> Unit,
    onDelete: () -> Unit,
) {
    Surface(tonalElevation = 1.dp) {
        BoxWithConstraints(Modifier.fillMaxWidth()) {
            val compact = maxWidth < 1320.dp
            val veryCompact = maxWidth < 930.dp
            Row(
                Modifier.fillMaxWidth().height(52.dp).padding(horizontal = 10.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(5.dp),
            ) {
                Row(
                    Modifier.weight(1f),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(5.dp),
                ) {
                    if (!veryCompact) {
                        Text("HLS Downloader", style = MaterialTheme.typography.titleSmall, modifier = Modifier.padding(horizontal = 8.dp))
                    }
                    Button(
                        onClick = onNew,
                        shape = DesktopControlShape,
                        contentPadding = androidx.compose.foundation.layout.PaddingValues(horizontal = if (compact) 10.dp else 12.dp),
                        modifier = Modifier.height(34.dp),
                    ) {
                        Icon(Icons.Default.Add, "新建", Modifier.size(17.dp))
                        if (!veryCompact) { Spacer(Modifier.width(6.dp)); Text("新建") }
                    }
                    ToolbarButton(Icons.Default.Downloading, "粘贴", onPaste, compact = compact)
                    ToolbarButton(Icons.Default.ViewList, "批量", onBatch, compact = compact)
                    VerticalRule()
                    ToolbarButton(Icons.Default.PlayArrow, "开始", { state.perform("resume") }, state.selectedTasks.isNotEmpty(), compact)
                    ToolbarButton(Icons.Default.Pause, "暂停", { state.perform("pause") }, state.selectedTasks.isNotEmpty(), compact)
                    ToolbarButton(Icons.Default.Close, "取消", { state.perform("cancel") }, state.selectedTasks.isNotEmpty(), compact)
                    ToolbarButton(Icons.Default.Delete, "删除", onDelete, state.selectedTasks.isNotEmpty(), compact)
                    ToolbarButton(Icons.Default.FolderOpen, "位置", {
                        state.selectedTasks.firstOrNull()?.outputPath?.takeIf { it.isNotBlank() }?.let { path -> state.launch { state.api.openExplorer(path) } }
                    }, state.selectedTasks.firstOrNull()?.outputPath?.isNotBlank() == true, compact)
                }
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(5.dp)) {
                    ToolbarButton(Icons.Default.Terminal, "日志", onLog, state.selectedTasks.size == 1, compact)
                    ToolbarButton(Icons.Default.Refresh, "刷新", { state.launch { state.refreshAll() } }, compact = compact)
                    ToolbarButton(Icons.Default.Description, "浏览器", onIntegration, compact = compact)
                    ToolbarButton(if (dark) Icons.Default.CheckCircle else Icons.Default.Movie, if (dark) "浅色" else "深色", onToggleTheme, compact = compact)
                    ToolbarButton(Icons.Default.Settings, "设置", onSettings, compact = compact)
                }
            }
        }
    }
}

@Composable
private fun ToolbarButton(
    icon: ImageVector,
    label: String,
    onClick: () -> Unit,
    enabled: Boolean = true,
    compact: Boolean = false,
) {
    if (compact) {
        IconButton(
            onClick = onClick,
            enabled = enabled,
            modifier = Modifier.size(36.dp),
        ) {
            Icon(icon, label, Modifier.size(17.dp))
        }
        return
    }
    TextButton(
        onClick = onClick,
        enabled = enabled,
        shape = DesktopControlShape,
        contentPadding = androidx.compose.foundation.layout.PaddingValues(horizontal = 9.dp),
        modifier = Modifier.height(32.dp),
    ) {
        Icon(icon, label, Modifier.size(16.dp))
        Spacer(Modifier.width(5.dp)); Text(label, style = MaterialTheme.typography.labelMedium)
    }
}

@Composable private fun VerticalRule() = Box(Modifier.padding(horizontal = 3.dp).width(1.dp).height(24.dp).background(MaterialTheme.colorScheme.outlineVariant))

@Composable
private fun Sidebar(state: AppState) {
    Surface(Modifier.width(176.dp).fillMaxHeight(), color = MaterialTheme.colorScheme.surface) {
        Column(Modifier.padding(8.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            TaskFilter.entries.forEach { item ->
                val icon = when (item) {
                    TaskFilter.ALL -> Icons.Default.ViewList
                    TaskFilter.ACTIVE -> Icons.Default.Downloading
                    TaskFilter.DONE -> Icons.Default.CheckCircle
                    TaskFilter.MEDIA -> Icons.Default.Movie
                    TaskFilter.PROGRAM -> Icons.Default.Terminal
                    TaskFilter.ARCHIVE -> Icons.Default.Archive
                    TaskFilter.OTHER -> Icons.Default.InsertDriveFile
                }
                val count = state.tasks.count { task ->
                    when (item) {
                        TaskFilter.ALL -> true
                        TaskFilter.ACTIVE -> task.status !in setOf("done", "failed", "canceled", "unsupported")
                        TaskFilter.DONE -> task.status == "done"
                        TaskFilter.MEDIA -> downloadCategory(task.displayName, task.mimeType) == "media"
                        TaskFilter.PROGRAM -> downloadCategory(task.displayName, task.mimeType) == "program"
                        TaskFilter.ARCHIVE -> downloadCategory(task.displayName, task.mimeType) == "archive"
                        TaskFilter.OTHER -> downloadCategory(task.displayName, task.mimeType) == "other"
                    }
                }
                val selected = state.filter == item
                Surface(
                    onClick = { state.filter = item },
                    color = if (selected) MaterialTheme.colorScheme.surfaceVariant else Color.Transparent,
                    shape = RoundedCornerShape(7.dp),
                ) {
                    Row(Modifier.fillMaxWidth().height(36.dp).padding(horizontal = 9.dp), verticalAlignment = Alignment.CenterVertically) {
                        Icon(icon, null, Modifier.size(17.dp), tint = if (selected) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant)
                        Spacer(Modifier.width(9.dp)); Text(item.label, Modifier.weight(1f), style = MaterialTheme.typography.bodyMedium)
                        Text(count.toString(), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                }
            }
            Spacer(Modifier.weight(1f))
            HorizontalDivider()
            Column(Modifier.padding(9.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Text("浏览器接管", style = MaterialTheme.typography.labelMedium)
                Text(
                    if (state.browserStatus.detected) "已连接 · ${state.browserStatus.version}" else "未连接",
                    style = MaterialTheme.typography.bodySmall,
                    color = if (state.browserStatus.detected) Color(0xFF53B987) else MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
    }
}

@Composable
private fun ContentHeader(state: AppState) {
    BoxWithConstraints(Modifier.fillMaxWidth()) {
        val compact = maxWidth < 820.dp
        Row(Modifier.fillMaxWidth().height(46.dp).padding(horizontal = 12.dp), verticalAlignment = Alignment.CenterVertically) {
            Row(Modifier.weight(1f), verticalAlignment = Alignment.CenterVertically) {
                Text(state.filter.label, style = MaterialTheme.typography.titleSmall)
                Spacer(Modifier.width(8.dp)); Text("${state.filteredTasks.size} 项", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(7.dp)) {
                CompactSearchField(
                    value = state.query,
                    onValueChange = { state.query = it },
                    placeholder = if (compact) "搜索" else "搜索文件、状态或网址",
                    modifier = Modifier.width(if (compact) 156.dp else 270.dp),
                )
                if (compact) {
                    FilledTonalIconButton(onClick = { state.clearCompleted() }, shape = DesktopControlShape, modifier = Modifier.size(34.dp)) {
                        Icon(Icons.Default.ClearAll, "清理完成", Modifier.size(16.dp))
                    }
                } else {
                    OutlinedButton(onClick = { state.clearCompleted() }, shape = DesktopControlShape, modifier = Modifier.height(32.dp)) {
                        Icon(Icons.Default.ClearAll, null, Modifier.size(15.dp)); Spacer(Modifier.width(5.dp)); Text("清理完成")
                    }
                }
            }
        }
    }
}

@Composable
private fun CompactSearchField(
    value: String,
    onValueChange: (String) -> Unit,
    placeholder: String,
    modifier: Modifier = Modifier,
) {
    var focused by remember { mutableStateOf(false) }
    Surface(
        modifier = modifier.height(36.dp),
        color = MaterialTheme.colorScheme.surface,
        shape = DesktopControlShape,
        border = BorderStroke(1.dp, if (focused) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.outline),
    ) {
        Row(
            Modifier.fillMaxSize().padding(start = 10.dp, end = if (value.isBlank()) 10.dp else 2.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(Icons.Default.Search, null, Modifier.size(16.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
            Spacer(Modifier.width(7.dp))
            Box(Modifier.weight(1f), contentAlignment = Alignment.CenterStart) {
                if (value.isBlank()) {
                    Text(placeholder, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                BasicTextField(
                    value = value,
                    onValueChange = onValueChange,
                    singleLine = true,
                    textStyle = MaterialTheme.typography.bodySmall.copy(color = MaterialTheme.colorScheme.onSurface),
                    cursorBrush = SolidColor(MaterialTheme.colorScheme.primary),
                    modifier = Modifier.fillMaxWidth().onFocusChanged { focused = it.isFocused },
                )
            }
            if (value.isNotBlank()) {
                IconButton(onClick = { onValueChange("") }, modifier = Modifier.size(30.dp)) {
                    Icon(Icons.Default.Close, "清空搜索", Modifier.size(14.dp))
                }
            }
        }
    }
}

@Composable
private fun TaskTable(
    state: AppState,
    keys: KeyboardSelection,
    onDetails: (TaskItem) -> Unit,
    onLog: (TaskItem) -> Unit,
    onDelete: (TaskItem, Boolean) -> Unit,
) {
    val listState = rememberLazyListState()
    var dragAnchor by remember { mutableStateOf<Int?>(null) }
    var dragBase by remember { mutableStateOf<Set<String>>(emptySet()) }
    fun itemAt(y: Float): Int? = listState.layoutInfo.visibleItemsInfo
        .firstOrNull { y.toInt() in it.offset until (it.offset + it.size) }
        ?.index
    Column(Modifier.fillMaxSize().padding(horizontal = 8.dp, vertical = 3.dp)) {
        TaskHeader(state)
        if (!state.loading && state.filteredTasks.isEmpty()) {
            Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    Icon(Icons.Default.Downloading, null, Modifier.size(38.dp), tint = MaterialTheme.colorScheme.outline)
                    Text(if (state.query.isNotBlank()) "没有匹配的任务" else "还没有下载任务", style = MaterialTheme.typography.titleMedium)
                    Text("点击顶部“新建”或从浏览器发送下载", color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
        } else {
            LazyColumn(
                state = listState,
                modifier = Modifier.fillMaxSize().pointerInput(state.filteredTasks, keys.ctrl) {
                    detectDragGestures(
                        onDragStart = { position ->
                            dragAnchor = itemAt(position.y)
                            dragBase = if (keys.ctrl) state.selected.toSet() else emptySet()
                            dragAnchor?.let { state.selectVisibleRange(it, it, dragBase) }
                        },
                        onDragEnd = { dragAnchor = null; dragBase = emptySet() },
                        onDragCancel = { dragAnchor = null; dragBase = emptySet() },
                        onDrag = { change, _ ->
                            val anchor = dragAnchor ?: return@detectDragGestures
                            itemAt(change.position.y)?.let { state.selectVisibleRange(anchor, it, dragBase) }
                            change.consume()
                        },
                    )
                },
                verticalArrangement = Arrangement.spacedBy(2.dp),
            ) {
                items(state.filteredTasks, key = { it.id }) { task ->
                    TaskRow(
                        task = task,
                        selected = task.id in state.selected,
                        onSelect = { state.select(task.id, additive = keys.ctrl, range = keys.shift) },
                        onCheckbox = { state.select(task.id, additive = true) },
                        onDetails = { onDetails(task) },
                        onLog = { onLog(task) },
                        onAction = { state.perform(it, listOf(task)) },
                        onDelete = { files -> onDelete(task, files) },
                        onOpen = { state.launch { state.api.openExplorer(task.outputPath) } },
                        onLaunch = { state.launch { state.api.launchFile(task.outputPath) } },
                        onPreview = { NativeDialogs.openUri(state.api.playerUrl(task.id)) },
                    )
                }
            }
        }
    }
}

@Composable
private fun TaskHeader(state: AppState) {
    Row(Modifier.fillMaxWidth().height(32.dp).padding(horizontal = 5.dp), verticalAlignment = Alignment.CenterVertically) {
        Checkbox(
            checked = state.filteredTasks.isNotEmpty() && state.filteredTasks.all { it.id in state.selected },
            onCheckedChange = { state.selectAllVisible() },
            modifier = Modifier.size(32.dp),
        )
        Text("文件名", Modifier.weight(1f), style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Text("状态", Modifier.width(88.dp), style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Text("进度", Modifier.width(180.dp), style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Text("速度 / 剩余", Modifier.width(122.dp), style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Text("更新时间", Modifier.width(92.dp), style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Spacer(Modifier.width(38.dp))
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun TaskRow(
    task: TaskItem,
    selected: Boolean,
    onSelect: () -> Unit,
    onCheckbox: () -> Unit,
    onDetails: () -> Unit,
    onLog: () -> Unit,
    onAction: (String) -> Unit,
    onDelete: (Boolean) -> Unit,
    onOpen: () -> Unit,
    onLaunch: () -> Unit,
    onPreview: () -> Unit,
) {
    var menu by remember { mutableStateOf(false) }
    Surface(
        color = if (selected) MaterialTheme.colorScheme.primaryContainer.copy(alpha = .72f) else MaterialTheme.colorScheme.surface,
        shape = RoundedCornerShape(7.dp),
        modifier = Modifier.fillMaxWidth().height(58.dp).combinedClickable(onClick = onSelect, onDoubleClick = onDetails),
    ) {
        Row(Modifier.fillMaxSize().padding(horizontal = 5.dp), verticalAlignment = Alignment.CenterVertically) {
            Checkbox(checked = selected, onCheckedChange = { onCheckbox() }, modifier = Modifier.size(32.dp))
            Column(Modifier.weight(1f).padding(horizontal = 6.dp)) {
                Text(task.displayName, maxLines = 1, overflow = TextOverflow.Ellipsis, fontWeight = FontWeight.Medium)
                Text(hostOf(task.url), maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            Text(statusLabel(task.status), Modifier.width(88.dp), color = MaterialTheme.colorScheme.statusColor(task.status), style = MaterialTheme.typography.labelLarge)
            Column(Modifier.width(180.dp).padding(end = 16.dp), verticalArrangement = Arrangement.spacedBy(5.dp)) {
                LinearProgressIndicator(progress = { task.progress }, modifier = Modifier.fillMaxWidth().height(5.dp).clip(RoundedCornerShape(3.dp)))
                Text("%.1f%%  ·  %s / %s".format(task.progressPercent, formatBytes(task.downloadedBytes), formatBytes(task.totalBytes)), style = MaterialTheme.typography.bodySmall, maxLines = 1)
            }
            Column(Modifier.width(122.dp)) {
                Text(formatSpeed(task.speedBytesPerSec), style = MaterialTheme.typography.labelMedium, color = if (task.speedBytesPerSec > 0) Color(0xFF53B987) else MaterialTheme.colorScheme.onSurfaceVariant)
                Text(formatEta(task.etaSeconds), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            Text(formatDate(task.updatedAt), Modifier.width(92.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Box(Modifier.width(38.dp)) {
                IconButton(onClick = { menu = true }) { Icon(Icons.Default.MoreVert, "更多操作") }
                DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                    DropdownMenuItem(text = { Text("任务详情") }, onClick = { menu = false; onDetails() })
                    DropdownMenuItem(text = { Text("查看日志") }, onClick = { menu = false; onLog() })
                    if ("pause" in task.availableActions) DropdownMenuItem(text = { Text("暂停") }, onClick = { menu = false; onAction("pause") })
                    if ("resume" in task.availableActions) DropdownMenuItem(text = { Text("恢复") }, onClick = { menu = false; onAction("resume") })
                    if ("retry" in task.availableActions) DropdownMenuItem(text = { Text("重试") }, onClick = { menu = false; onAction("retry") })
                    if ("cancel" in task.availableActions) DropdownMenuItem(text = { Text("取消") }, onClick = { menu = false; onAction("cancel") })
                    if ("preview" in task.availableActions) DropdownMenuItem(text = { Text(if (task.status == "done") "内置播放" else "边下边播") }, onClick = { menu = false; onPreview() })
                    if (task.outputPath.isNotBlank()) DropdownMenuItem(text = { Text("打开所在位置") }, onClick = { menu = false; onOpen() })
                    if (task.outputIsFile && task.status == "done") DropdownMenuItem(text = { Text("系统打开") }, onClick = { menu = false; onLaunch() })
                    HorizontalDivider()
                    DropdownMenuItem(text = { Text("删除记录") }, onClick = { menu = false; onDelete(false) })
                    DropdownMenuItem(text = { Text("删除任务及文件", color = MaterialTheme.colorScheme.error) }, onClick = { menu = false; onDelete(true) })
                }
            }
        }
    }
}

@Composable
private fun ErrorBar(message: String, dismiss: () -> Unit) {
    Surface(color = MaterialTheme.colorScheme.error.copy(alpha = .12f)) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(message, Modifier.weight(1f), color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
            TextButton(onClick = dismiss) { Text("关闭") }
        }
    }
}

@Composable
private fun StatusBar(state: AppState) {
    val speed = state.tasks.sumOf { it.speedBytesPerSec }
    val active = state.tasks.count { it.status in setOf("downloading", "downloading_m3u8", "downloading_segments", "merging", "remuxing") }
    Surface(color = MaterialTheme.colorScheme.surface) {
        Row(Modifier.fillMaxWidth().height(27.dp).padding(horizontal = 11.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(18.dp)) {
            Text("任务 ${state.tasks.size}", style = MaterialTheme.typography.bodySmall)
            Text("进行中 $active", style = MaterialTheme.typography.bodySmall)
            Text("总速度 ${formatSpeed(speed)}", style = MaterialTheme.typography.bodySmall)
            Spacer(Modifier.weight(1f))
            Text("v${state.version}", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}
