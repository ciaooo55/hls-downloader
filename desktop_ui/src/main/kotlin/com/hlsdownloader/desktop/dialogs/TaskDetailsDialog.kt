package com.hlsdownloader.desktop

import androidx.compose.foundation.background
import androidx.compose.foundation.Image
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.border
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.VerticalScrollbar
import androidx.compose.foundation.rememberScrollbarAdapter
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import com.hlsdownloader.desktop.icons.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.toComposeImageBitmap
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.coroutines.launch
import java.awt.Toolkit
import java.awt.datatransfer.StringSelection
import java.io.File
import java.net.URI
import java.nio.file.Files
import org.jetbrains.skia.Image as SkiaImage

@Composable internal fun TaskDetailsDialog(
    task: DownloadTask,
    logLines: List<String>?,
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
    LaunchedEffect(task.id, canPreview, task.source.outputPath, tab) {
        if (!canPreview) {
            preview = null
        } else if (tab == "预览" && preview == null) {
            preview = withContext(Dispatchers.IO) { loadLocalImagePreview(task.source.outputPath) }
        }
    }
    LaunchedEffect(task.id, torrentTask) {
        if (!torrentTask) return@LaunchedEffect
        torrentLoading = true
        runCatching { withContext(Dispatchers.IO) { EnginePipeClient.getTaskTorrentFiles(task.id) } }
            .onSuccess { torrentFiles = it.files }
            .onFailure { torrentNotice = describeTaskActionFailure(it, "读取 BT 文件清单失败") }
        torrentLoading = false
    }
    WorkbenchDialog(onDismiss, "任务详情", "进度、连接、速度和运行日志", 780.dp, content = {
        Text(task.filename, color = ink, fontSize = TypeScale.title, fontWeight = FontWeight.SemiBold, maxLines = 2, overflow = TextOverflow.Ellipsis)
        Spacer(Modifier.height(12.dp))
        if (!taskProgressKnown(task.source) && isActiveTaskStatus(task.status)) LinearProgressIndicator(modifier = Modifier.fillMaxWidth().height(7.dp), color = blue, trackColor = surface3)
        else LinearProgressIndicator(progress = { task.progress }, modifier = Modifier.fillMaxWidth().height(7.dp).clip(RoundedCornerShape(Radius.sm)), color = blue, trackColor = surface3)
        Row(Modifier.padding(top = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            StatusBadge(task.status); Spacer(Modifier.width(9.dp))
            Text(
                if (torrentTask) "${taskProgressLabel(task)} · ${task.speed} · ${task.segments}"
                else "${taskProgressLabel(task)} · ${task.speed} · ${task.segments} · ${task.source.activeWorkers} 个连接",
                color = muted,
                fontSize = TypeScale.caption,
            )
        }
        Spacer(Modifier.height(14.dp))
        Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(Radius.md)).background(surface2).padding(3.dp)) {
            (listOf("概览", "连接", "速度", "日志") + if (canPreview) listOf("预览") else emptyList()).forEach { item ->
                TextButton(onClick = { tab = item; if (item == "日志") onAction("log") }, Modifier.weight(1f), containerColor = segmentBackground(tab == item, rail, Color.Transparent), hoverColor = if (tab == item) rail else surface3, pressedColor = if (tab == item) rail else pressedSurface, pressScale = SEGMENT_SCALE) { Text(item, color = if (tab == item) blue else muted, fontSize = TypeScale.caption, fontWeight = if (tab == item) FontWeight.SemiBold else FontWeight.Normal) }
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
                if (task.source.outputMissing) DetailLine("输出", "文件已删除，可重新下载", errorStrong)
                TaskVerificationDetails(task.source)
                taskFailureDetails(task.source)?.let { failure ->
                    Spacer(Modifier.height(13.dp))
                    Surface(
                        Modifier.fillMaxWidth(),
                        color = errorSurface,
                        shape = RoundedCornerShape(Radius.md),
                        border = BorderStroke(1.dp, errorBorder),
                    ) {
                        Column(Modifier.padding(12.dp)) {
                            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                                Icon(Icons.Outlined.ErrorOutline, null, tint = errorStrong, modifier = Modifier.size(17.dp))
                                Spacer(Modifier.width(7.dp))
                                Text(failure.title, Modifier.weight(1f), color = errorStrong, fontSize = TypeScale.body, fontWeight = FontWeight.SemiBold)
                                TextButton(onClick = {
                                    runCatching { Toolkit.getDefaultToolkit().systemClipboard.setContents(StringSelection(taskFailureDiagnostic(task)), null) }
                                    diagnosticsCopied = true
                                }, contentPadding = PaddingValues(horizontal = 7.dp, vertical = 2.dp)) { Text(if (diagnosticsCopied) "已复制" else "复制诊断", color = blue, fontSize = TypeScale.micro) }
                            }
                            failure.items.forEach { (label, value) -> DetailLine(label, value, errorBody) }
                            task.source.errorMessage?.takeIf(String::isNotBlank)?.let { message ->
                                Spacer(Modifier.height(8.dp)); Text("失败原因", color = errorStrong, fontSize = TypeScale.micro, fontWeight = FontWeight.SemiBold)
                                Text(redactDiagnosticText(message), color = errorBody, fontSize = TypeScale.caption, lineHeight = TypeScale.lineBody, modifier = Modifier.padding(top = 3.dp))
                            }
                            if (failure.steps.isNotEmpty()) {
                                Spacer(Modifier.height(8.dp)); Text("建议步骤", color = errorStrong, fontSize = TypeScale.micro, fontWeight = FontWeight.SemiBold)
                                failure.steps.forEachIndexed { index, step -> Text("${index + 1}. $step", color = errorBody, fontSize = TypeScale.caption, lineHeight = TypeScale.lineBody, modifier = Modifier.padding(top = 2.dp)) }
                            }
                        }
                    }
                }
                if (torrentTask) {
                    Spacer(Modifier.height(13.dp)); HorizontalDivider(color = border); Spacer(Modifier.height(11.dp))
                    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                        Text("BT 文件选择", Modifier.weight(1f), color = ink, fontSize = TypeScale.body, fontWeight = FontWeight.SemiBold)
                        Text("已选 ${torrentFiles.count { it.selected }} / ${torrentFiles.size} · ${formatBytes(torrentFiles.filter { it.selected }.sumOf { it.size })}", color = muted, fontSize = TypeScale.micro)
                    }
                    Row(Modifier.padding(top = 5.dp), horizontalArrangement = Arrangement.spacedBy(5.dp)) {
                        TextButton(onClick = { torrentFiles = torrentFiles.map { it.copy(selected = true) } }, enabled = torrentFiles.isNotEmpty(), contentPadding = PaddingValues(horizontal = 7.dp), hoverColor = surface3, pressedColor = pressedSurface, modifier = Modifier.reportControlBounds("task_details.select_all")) { Text("全选", fontSize = TypeScale.micro) }
                        TextButton(onClick = { torrentFiles = torrentFiles.map { it.copy(selected = !it.selected) } }, enabled = torrentFiles.isNotEmpty(), contentPadding = PaddingValues(horizontal = 7.dp), hoverColor = surface3, pressedColor = pressedSurface, modifier = Modifier.reportControlBounds("task_details.select_none")) { Text("反选", fontSize = TypeScale.micro) }
                    }
                    when {
                        torrentLoading -> Text("正在读取种子元数据…", color = muted, fontSize = TypeScale.caption, modifier = Modifier.padding(vertical = 18.dp))
                        torrentFiles.isEmpty() -> Text("尚未取得文件清单。磁力任务需要先取得元数据。", color = muted, fontSize = TypeScale.caption, modifier = Modifier.padding(vertical = 14.dp))
                        else -> Box(Modifier.fillMaxWidth().heightIn(max = 220.dp)) {
                            val torrentFilesScroll = rememberScrollState()
                            Column(Modifier.fillMaxWidth().border(BorderStroke(1.dp, border), RoundedCornerShape(Radius.md)).clip(RoundedCornerShape(Radius.md)).verticalScroll(torrentFilesScroll).padding(horizontal = 9.dp, vertical = 5.dp)) {
                                torrentFiles.forEachIndexed { index, file ->
                                    Row(Modifier.fillMaxWidth().padding(vertical = 3.dp), verticalAlignment = Alignment.CenterVertically) {
                                        Checkbox(file.selected, { checked -> torrentFiles = torrentFiles.toMutableList().also { it[index] = file.copy(selected = checked) } }, accessibilityLabel = "选择 ${file.path}")
                                        Spacer(Modifier.width(6.dp)); Text(file.path, Modifier.weight(1f), color = ink, fontSize = TypeScale.caption, maxLines = 2, overflow = TextOverflow.Ellipsis)
                                        Spacer(Modifier.width(8.dp)); Text(formatBytes(file.size), color = muted, fontSize = TypeScale.micro)
                                    }
                                }
                            }
                            VerticalScrollbar(
        rememberScrollbarAdapter(torrentFilesScroll),
        Modifier.align(Alignment.CenterEnd).fillMaxHeight().width(6.dp),
        style = workbenchScrollbarStyle(),
    )
                        }
                    }
                    if (task.status == "进行中") Text("当前内置 BT 引擎需先暂停任务，才能安全调整文件选择。", color = warningColor, fontSize = TypeScale.micro, modifier = Modifier.padding(top = 6.dp))
                    if (torrentNotice.isNotBlank()) Text(torrentNotice, color = if (torrentNotice.contains("已保存")) successColor else errorStrong, fontSize = TypeScale.micro, modifier = Modifier.padding(top = 6.dp))
                    DialogPrimary(if (torrentBusy) "正在保存…" else "保存文件选择", !torrentBusy && task.status != "进行中" && torrentFiles.any { it.selected }) {
                        torrentBusy = true; torrentNotice = ""
                        detailScope.launch {
                            runCatching { withContext(Dispatchers.IO) { EnginePipeClient.setTaskTorrentFiles(task.id, torrentFiles) } }
                                .onSuccess { torrentFiles = it.files; torrentNotice = "文件选择已保存，将在开始或恢复时生效" }
                                .onFailure { torrentNotice = describeTaskActionFailure(it, "保存文件选择失败") }
                            torrentBusy = false
                        }
                    }
                }
                if (task.status != "已完成") {
                    Spacer(Modifier.height(13.dp)); HorizontalDivider(color = border); Spacer(Modifier.height(11.dp)); DialogLabel("任务限速 KiB/s（0 不限制）")
                    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) { OutlinedTextField(speedLimit, { speedLimit = it.filter(Char::isDigit) }, Modifier.weight(1f), singleLine = true, shape = RoundedCornerShape(Radius.md)); Spacer(Modifier.width(8.dp)); DialogSecondary("应用") { onAction("speed:${speedLimit.toLongOrNull()?.coerceIn(0, 1_048_576) ?: 0}") } }
                    if (canRefresh) { Spacer(Modifier.height(8.dp)); TextButton(onClick = { showRefresh = !showRefresh }, contentPadding = PaddingValues(0.dp), hoverColor = surface3, pressedColor = pressedSurface, modifier = Modifier.reportControlBounds("task_details.refresh_link")) { Text(if (showRefresh) "收起链接更新" else "更新下载链接", color = blue, fontSize = TypeScale.caption) } }
                    if (showRefresh) {
                        OutlinedTextField(refreshUrl, { refreshUrl = it }, Modifier.fillMaxWidth(), label = { Text("新的资源地址") }, minLines = 2, maxLines = 3, shape = RoundedCornerShape(Radius.md))
                        Spacer(Modifier.height(7.dp))
                        OutlinedTextField(refreshCookie, { refreshCookie = it }, Modifier.fillMaxWidth(), singleLine = true, visualTransformation = PasswordVisualTransformation(), label = { Text("新的 Cookie（可选）") }, shape = RoundedCornerShape(Radius.md))
                        Text("留空保留同站点原凭据；跨站地址会自动丢弃旧凭据。适合 401、403 和短效签名过期。", color = muted, fontSize = TypeScale.micro, lineHeight = TypeScale.lineMicro, modifier = Modifier.padding(top = 5.dp))
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
            else -> Surface(Modifier.fillMaxWidth().heightIn(min = 210.dp), color = surface2, shape = RoundedCornerShape(Radius.md), border = BorderStroke(1.dp, border)) { Text((logLines ?: task.source.logTail).ifEmpty { listOf("暂无日志记录") }.joinToString("\n"), Modifier.padding(12.dp), color = muted, fontSize = TypeScale.caption, lineHeight = TypeScale.lineBody) }
        }
    }, actions = {
        val mediaCapable = taskSupportsMediaActions(task.source)
        if (mediaCapable && task.source.playbackReady) DialogSecondary(taskPlayLabel(task.source)) { onAction("play") }
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
    Surface(Modifier.fillMaxWidth().heightIn(min = 220.dp, max = 380.dp), color = surface2, shape = RoundedCornerShape(Radius.md), border = BorderStroke(1.dp, border)) {
        Box(Modifier.fillMaxSize().padding(10.dp), contentAlignment = Alignment.Center) {
            when {
                preview == null -> Text("正在读取图片…", color = muted, fontSize = TypeScale.caption)
                preview.isFailure -> Text(describeFailure(preview.exceptionOrNull(), "图片预览失败"), color = errorStrong, fontSize = TypeScale.caption)
                else -> Image(preview.getOrThrow(), "已下载图片预览", Modifier.fillMaxSize(), contentScale = ContentScale.Fit)
            }
        }
    }
}

@Composable private fun ConnectionMap(parts: List<ConnectionPartDto>, hint: String) {
    Column {
        Text(hint.ifBlank { if (parts.isEmpty()) "当前任务没有活动分段" else "${parts.size} 个连接分段" }, color = muted, fontSize = TypeScale.caption)
        Spacer(Modifier.height(10.dp))
        if (parts.isEmpty()) Surface(Modifier.fillMaxWidth().height(84.dp), color = surface2, shape = RoundedCornerShape(Radius.md)) { Box(contentAlignment = Alignment.Center) { Text("下载开始后显示各连接覆盖范围", color = faint, fontSize = TypeScale.caption) } }
        else Column(verticalArrangement = Arrangement.spacedBy(6.dp)) { parts.chunked(8).forEach { row -> Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(6.dp)) { row.forEach { part -> val length = (part.end - part.start + 1).coerceAtLeast(1); val progress = (part.done.toFloat() / length).coerceIn(0f, 1f); Column(Modifier.weight(1f)) { LinearProgressIndicator(progress = { progress }, modifier = Modifier.fillMaxWidth().height(7.dp).clip(RoundedCornerShape(Radius.tiny)), color = if (part.state == "failed") errorStrong else blue, trackColor = surface3); Text("${(progress * 100).toInt()}%", color = faint, fontSize = TypeScale.micro) } }; repeat(8 - row.size) { Spacer(Modifier.weight(1f)) } } } }
    }
}

@Composable private fun SpeedHistory(history: List<Long>) {
    val values = history.takeLast(48)
    val maximum = values.maxOrNull()?.coerceAtLeast(1) ?: 1
    Column {
        Text(if (values.isEmpty()) "暂无速度采样" else "峰值 ${formatRate(maximum)} · 最近 ${values.size} 个采样", color = muted, fontSize = TypeScale.caption)
        Spacer(Modifier.height(10.dp))
        Row(Modifier.fillMaxWidth().height(150.dp).clip(RoundedCornerShape(Radius.md)).background(surface2).padding(horizontal = 10.dp, vertical = 12.dp), horizontalArrangement = Arrangement.spacedBy(3.dp), verticalAlignment = Alignment.Bottom) {
            if (values.isEmpty()) Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { Text("任务运行后显示实时速度", color = faint, fontSize = TypeScale.caption) }
            else values.forEach { value -> Box(Modifier.weight(1f).fillMaxHeight((value.toFloat() / maximum).coerceIn(.03f, 1f)).clip(RoundedCornerShape(Radius.tiny)).background(blue.copy(alpha = .78f))) }
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
    Text("完成检查", color = ink, fontSize = TypeScale.body, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(top = 11.dp))
    if (task.expectedChecksum.isNotBlank()) {
        val result = when (task.checksumVerified) {
            true -> "已通过"
            false -> "不匹配"
            null -> "等待下载完成"
        }
        val resultColor = when (task.checksumVerified) {
            true -> successColor
            false -> errorStrong
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
        DetailLine("病毒扫描", result, if (scan.state == "clean") successColor else if (scan.state == "threat") errorStrong else muted)
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
        DetailLine(label, detail, if (mirror.state == "failed") errorStrong else null)
    }
}

@Composable internal fun DetailLine(label: String, value: String, color: Color? = null) = Row(Modifier.fillMaxWidth().padding(top = 9.dp)) { Text(label, Modifier.width(72.dp), color = faint, fontSize = TypeScale.caption); Text(value, Modifier.weight(1f), color = color ?: muted, fontSize = TypeScale.caption, maxLines = 2, overflow = TextOverflow.Ellipsis) }

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
