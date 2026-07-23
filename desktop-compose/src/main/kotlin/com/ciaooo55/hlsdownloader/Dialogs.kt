package com.ciaooo55.hlsdownloader

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Check
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.ContentCopy
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material.icons.filled.Download
import androidx.compose.material.icons.filled.FileOpen
import androidx.compose.material.icons.filled.FolderOpen
import androidx.compose.material.icons.filled.Info
import androidx.compose.material.icons.filled.Link
import androidx.compose.material.icons.filled.OpenInBrowser
import androidx.compose.material.icons.filled.Pause
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material.icons.filled.RestartAlt
import androidx.compose.material.icons.filled.Save
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material3.Button
import androidx.compose.material3.Checkbox
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import java.awt.Toolkit
import java.awt.datatransfer.StringSelection
import java.net.URI

@Composable
private fun Modal(
    title: String,
    subtitle: String = "",
    width: Dp = 640.dp,
    height: Dp? = null,
    onClose: () -> Unit,
    footer: @Composable RowScope.() -> Unit,
    content: @Composable () -> Unit,
) {
    Box(
        Modifier.fillMaxSize()
            .background(Color.Black.copy(alpha = 0.62f))
            .clickable(onClick = onClose),
        contentAlignment = Alignment.Center,
    ) {
        Surface(
            modifier = Modifier.width(width)
                .let { if (height != null) it.height(height) else it }
                .clickable(onClick = {}),
            shape = MaterialTheme.shapes.large,
            color = MaterialTheme.colorScheme.surface,
            tonalElevation = 4.dp,
            shadowElevation = 8.dp,
        ) {
            Column(Modifier.fillMaxSize()) {
                Row(Modifier.fillMaxWidth().padding(18.dp, 15.dp, 12.dp, 13.dp), verticalAlignment = Alignment.Top) {
                    Column(Modifier.weight(1f)) {
                        Text(title, style = MaterialTheme.typography.titleLarge)
                        if (subtitle.isNotBlank()) Text(subtitle, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                    IconButton(onClick = onClose) { Icon(Icons.Default.Close, "关闭") }
                }
                HorizontalDivider()
                Box(Modifier.weight(1f).fillMaxWidth()) { content() }
                HorizontalDivider()
                Row(
                    Modifier.fillMaxWidth().padding(14.dp, 10.dp),
                    horizontalArrangement = Arrangement.End,
                    verticalAlignment = Alignment.CenterVertically,
                ) { footer() }
            }
        }
    }
}

@Composable
fun AddDownloadDialog(state: AppState, initialUrl: String, onClose: () -> Unit) {
    var url by remember(initialUrl) { mutableStateOf(initialUrl.trim()) }
    var filename by remember { mutableStateOf("") }
    var directory by remember { mutableStateOf(state.settings.downloadDir) }
    var concurrency by remember { mutableStateOf(state.settings.defaultConcurrency.toString()) }
    var referer by remember { mutableStateOf(state.settings.defaultReferer) }
    var origin by remember { mutableStateOf(state.settings.defaultOrigin) }
    var userAgent by remember { mutableStateOf(state.settings.defaultUserAgent) }
    var cookie by remember { mutableStateOf(state.settings.defaultCookie) }
    var advanced by remember { mutableStateOf(false) }
    var recognizing by remember { mutableStateOf(false) }
    var candidates by remember { mutableStateOf<List<RecognitionCandidate>>(emptyList()) }
    var localError by remember { mutableStateOf("") }

    fun payload(target: String) = TaskCreate(
        url = target.trim(), filename = filename.trim(), downloadDir = directory.trim(),
        concurrency = concurrency.toIntOrNull()?.coerceIn(1, 256) ?: state.settings.defaultConcurrency,
        referer = referer.trim(), origin = origin.trim(), userAgent = userAgent.trim(), cookie = cookie.trim(),
    )
    fun directType(value: String): Boolean {
        if (value.startsWith("magnet:", true)) return true
        val path = runCatching { URI.create(value).path.lowercase() }.getOrDefault("")
        return listOf(".m3u8", ".mpd", ".torrent", ".mp4", ".mkv", ".webm", ".mov", ".mp3", ".zip", ".7z", ".rar", ".exe", ".msi", ".iso", ".pdf").any(path::endsWith)
    }
    fun submit(target: String = url) {
        if (target.isBlank()) return
        if (directType(target) || candidates.any { it.url == target }) {
            state.createTask(payload(target), onClose)
            return
        }
        state.launch {
            recognizing = true; localError = ""
            try {
                val result = state.api.recognize(target.trim(), referer, origin, userAgent, cookie)
                candidates = result.candidates
                when (result.candidates.size) {
                    1 -> state.createTask(payload(result.candidates.first().url), onClose)
                    0 -> localError = result.message.ifBlank { "页面中没有识别到可下载媒体，请使用浏览器扩展嗅探后再发送。" }
                }
            } catch (reason: Throwable) { localError = reason.message ?: "识别失败" }
            finally { recognizing = false }
        }
    }

    Modal(
        title = "新建下载",
        subtitle = "普通文件、HLS、DASH、magnet 和 .torrent",
        width = 700.dp,
        height = if (advanced || candidates.isNotEmpty()) 680.dp else 470.dp,
        onClose = onClose,
        footer = {
            OutlinedButton(onClick = onClose, shape = DesktopControlShape) { Text("取消") }
            Spacer(Modifier.width(8.dp))
            Button(onClick = { submit() }, enabled = url.isNotBlank() && !recognizing && !state.busy, shape = DesktopControlShape) {
                Icon(Icons.Default.Download, null, Modifier.size(17.dp)); Spacer(Modifier.width(6.dp))
                Text(if (recognizing) "正在识别…" else if (directType(url)) "开始下载" else "识别并下载")
            }
        },
    ) {
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(18.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            LabeledField("链接", url, { url = it; candidates = emptyList() }, "粘贴文件、m3u8、mpd、网页或 magnet 链接", leading = Icons.Default.Link)
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                OutlinedButton(onClick = {
                    NativeDialogs.chooseTorrent()?.let { path -> state.uploadTorrent(path, onClose) }
                }, enabled = !state.busy, shape = DesktopControlShape) { Icon(Icons.Default.FileOpen, null, Modifier.size(16.dp)); Spacer(Modifier.width(6.dp)); Text("导入 .torrent") }
                Text("磁力链接可直接粘贴", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Box(Modifier.weight(1f)) { LabeledField("输出文件名（可选）", filename, { filename = it }, "自动识别") }
                Box(Modifier.width(112.dp)) { LabeledField("并发", concurrency, { concurrency = it.filter(Char::isDigit) }, "12") }
            }
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.Bottom) {
                Box(Modifier.weight(1f)) { LabeledField("保存到", directory, { directory = it }) }
                IconButton(onClick = { NativeDialogs.chooseFolder(directory)?.let { directory = it } }) { Icon(Icons.Default.FolderOpen, "选择目录") }
            }
            TextButton(onClick = { advanced = !advanced }, contentPadding = PaddingValues(0.dp)) {
                Text(if (advanced) "收起请求选项" else "请求选项（Referer / Origin / Cookie）")
            }
            if (advanced) {
                Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    Box(Modifier.weight(1f)) { LabeledField("Referer", referer, { referer = it }, "https://example.com/watch") }
                    Box(Modifier.weight(1f)) { LabeledField("Origin", origin, { origin = it }, "https://example.com") }
                }
                LabeledField("User-Agent", userAgent, { userAgent = it })
                LabeledField("Cookie", cookie, { cookie = it }, "sessionid=abc; token=xyz")
            }
            if (candidates.isNotEmpty()) {
                Text("发现 ${candidates.size} 个播放清单", style = MaterialTheme.typography.titleSmall)
                candidates.forEach { candidate ->
                    Surface(onClick = { submit(candidate.url) }, color = MaterialTheme.colorScheme.surfaceVariant, shape = MaterialTheme.shapes.small) {
                        Column(Modifier.fillMaxWidth().padding(10.dp)) {
                            Text(candidate.title.ifBlank { candidate.quality.ifBlank { "媒体清单" } }, fontWeight = FontWeight.Medium)
                            Text(candidate.url, maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        }
                    }
                }
            }
            if (localError.isNotBlank()) InlineError(localError)
        }
    }
}

@Composable
fun BatchDownloadDialog(state: AppState, onClose: () -> Unit) {
    var text by remember { mutableStateOf("") }
    var referer by remember { mutableStateOf(state.settings.defaultReferer) }
    var directory by remember { mutableStateOf(state.settings.downloadDir) }
    var concurrency by remember { mutableStateOf(state.settings.defaultConcurrency.toString()) }
    val urls = text.lines().map(String::trim).filter(String::isNotBlank)
    Modal("批量添加", "每行输入一个下载链接，最多 100 项", 720.dp, 560.dp, onClose, footer = {
        OutlinedButton(onClick = onClose, shape = DesktopControlShape) { Text("取消") }; Spacer(Modifier.width(8.dp))
        Button(onClick = {
            state.createBatch(urls.take(100).map { TaskCreate(it, referer = referer, downloadDir = directory, concurrency = concurrency.toIntOrNull() ?: state.settings.defaultConcurrency) }, onClose)
        }, enabled = urls.isNotEmpty() && urls.size <= 100 && !state.busy, shape = DesktopControlShape) {
            Icon(Icons.Default.Add, null, Modifier.size(17.dp)); Spacer(Modifier.width(6.dp)); Text("添加 ${urls.size} 项")
        }
    }) {
        Column(Modifier.fillMaxSize().padding(18.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            OutlinedTextField(text, { text = it }, Modifier.fillMaxWidth().weight(1f), label = { Text("下载链接") }, placeholder = { Text("https://example.com/video.m3u8") })
            Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Box(Modifier.weight(1f)) { LabeledField("Referer（可选）", referer, { referer = it }) }
                Box(Modifier.width(112.dp)) { LabeledField("并发", concurrency, { concurrency = it.filter(Char::isDigit) }) }
            }
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.Bottom) {
                Box(Modifier.weight(1f)) { LabeledField("保存到", directory, { directory = it }) }
                IconButton(onClick = { NativeDialogs.chooseFolder(directory)?.let { directory = it } }) { Icon(Icons.Default.FolderOpen, "选择目录") }
            }
            if (urls.size > 100) InlineError("一次最多添加 100 项")
        }
    }
}

@Composable
fun SettingsDialog(state: AppState, onClose: () -> Unit) {
    var value by remember { mutableStateOf(state.settings) }
    var advanced by remember { mutableStateOf(false) }
    var message by remember { mutableStateOf("") }
    var update by remember { mutableStateOf(state.updateInfo) }
    val dirty = value != state.settings
    Modal("设置${if (dirty) " *" else ""}", "下载位置、并发、浏览器接管和请求参数", 760.dp, 720.dp, onClose, footer = {
        OutlinedButton(onClick = onClose, shape = DesktopControlShape) { Text("关闭") }; Spacer(Modifier.width(8.dp))
        Button(onClick = { state.saveSettings(value) { message = "设置已保存" } }, enabled = dirty && !state.busy, shape = DesktopControlShape) {
            Icon(Icons.Default.Save, null, Modifier.size(17.dp)); Spacer(Modifier.width(6.dp)); Text(if (state.busy) "保存中…" else "保存")
        }
    }) {
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(18.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            SectionTitle("存储")
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.Bottom) {
                Box(Modifier.weight(1f)) { LabeledField("最终文件保存目录", value.downloadDir, { value = value.copy(downloadDir = it) }) }
                IconButton(onClick = { NativeDialogs.chooseFolder(value.downloadDir)?.let { value = value.copy(downloadDir = it) } }) { Icon(Icons.Default.FolderOpen, "选择目录") }
                IconButton(onClick = { state.launch { state.api.openExplorer(value.downloadDir) } }) { Icon(Icons.Default.OpenInBrowser, "打开目录") }
            }
            Text("下载完成后的文件保存在这里。", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.Bottom) {
                Box(Modifier.weight(1f)) { LabeledField("缓存与过程文件目录", value.tempDir, { value = value.copy(tempDir = it) }) }
                IconButton(onClick = { NativeDialogs.chooseFolder(value.tempDir)?.let { value = value.copy(tempDir = it) } }) { Icon(Icons.Default.FolderOpen, "选择目录") }
                IconButton(onClick = { state.launch { state.api.openExplorer(value.tempDir) } }) { Icon(Icons.Default.OpenInBrowser, "打开目录") }
            }
            Text("分片、断点、BT 数据和任务日志位于该目录的 .tasks；跨磁盘完成时会复制后原子替换。", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)

            SectionTitle("下载性能")
            Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Box(Modifier.weight(1f)) { NumberField("默认并发", value.defaultConcurrency, 1, 256) { value = value.copy(defaultConcurrency = it) } }
                Box(Modifier.weight(1f)) { NumberField("最大同时任务", value.maxConcurrentTasks, 1, 16) { value = value.copy(maxConcurrentTasks = it) } }
                Box(Modifier.weight(1f)) { NumberField("HTTP 分段 MiB", value.httpChunkSizeMb, 1, 64) { value = value.copy(httpChunkSizeMb = it) } }
            }

            SectionTitle("浏览器接管")
            ToggleRow("启用浏览器下载接管", "关闭后浏览器继续使用自己的下载器", value.browserTakeoverEnabled) { value = value.copy(browserTakeoverEnabled = it) }
            NumberField("最小接管大小（MiB，0 表示不限）", value.browserTakeoverMinMb, 0, 10240) { value = value.copy(browserTakeoverMinMb = it) }

            SectionTitle("BT 下载")
            Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Box(Modifier.weight(1f)) { NumberField("上传上限 KiB/s", value.btUploadLimitKib, 0, 1048576) { value = value.copy(btUploadLimitKib = it) } }
                Box(Modifier.weight(1f)) { NumberField("最大 Peer 连接", value.btMaxConnections, 10, 1000) { value = value.copy(btMaxConnections = it) } }
            }
            ToggleRow("启用 DHT 节点发现", "提高无 Tracker 磁力链接的发现能力", value.btEnableDht) { value = value.copy(btEnableDht = it) }

            SectionTitle("浏览器令牌")
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.Bottom) {
                Box(Modifier.weight(1f)) { LabeledField("Token", value.token, {}, readOnly = true) }
                IconButton(onClick = { Toolkit.getDefaultToolkit().systemClipboard.setContents(StringSelection(value.token), null); message = "Token 已复制" }) { Icon(Icons.Default.ContentCopy, "复制") }
            }

            TextButton(onClick = { advanced = !advanced }, contentPadding = PaddingValues(0.dp)) { Text(if (advanced) "收起高级选项" else "展开高级请求选项") }
            if (advanced) {
                LabeledField("默认 Referer", value.defaultReferer, { value = value.copy(defaultReferer = it) })
                LabeledField("默认 Origin", value.defaultOrigin, { value = value.copy(defaultOrigin = it) })
                LabeledField("默认 User-Agent", value.defaultUserAgent, { value = value.copy(defaultUserAgent = it) })
                LabeledField("默认 Cookie", value.defaultCookie, { value = value.copy(defaultCookie = it) })
                LabeledField("FFmpeg 路径", value.ffmpegPath, { value = value.copy(ffmpegPath = it) })
                LabeledField("允许域名（逗号分隔）", value.allowedHosts.joinToString(","), { text -> value = value.copy(allowedHosts = text.split(',').map(String::trim).filter(String::isNotBlank)) })
                ToggleRow("保留临时文件", "仅排查下载问题时启用", value.keepTempFiles) { value = value.copy(keepTempFiles = it) }
            }

            SectionTitle("软件更新")
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text(update?.let { "当前 v${it.currentVersion} · ${if (it.available) "可更新到 v${it.latestVersion}" else "已是最新"}" } ?: "尚未检查")
                    update?.notes?.takeIf(String::isNotBlank)?.let { Text(it, maxLines = 2, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant) }
                }
                OutlinedButton(onClick = { state.launch { update = state.api.updateInfo(true); state.updateInfo = update } }, shape = DesktopControlShape) { Icon(Icons.Default.Refresh, null, Modifier.size(16.dp)); Spacer(Modifier.width(5.dp)); Text("检查") }
                if (update?.available == true && update?.canAutoInstall == true) {
                    Spacer(Modifier.width(7.dp)); Button(onClick = { state.launch { state.api.installUpdate() } }, shape = DesktopControlShape) { Text("安装更新") }
                }
            }
            if (message.isNotBlank()) Surface(color = MaterialTheme.colorScheme.primaryContainer, shape = MaterialTheme.shapes.small) { Text(message, Modifier.fillMaxWidth().padding(10.dp), style = MaterialTheme.typography.bodySmall) }
        }
    }
}

@Composable
fun IntegrationDialog(state: AppState, onClose: () -> Unit) {
    var browser by remember { mutableStateOf(state.browserStatus) }
    var message by remember { mutableStateOf("") }
    LaunchedEffect(Unit) {
        browser = runCatching { state.api.browserStatus() }.getOrDefault(browser)
    }
    Modal("浏览器插件", "Chrome/Edge 与 Firefox 插件负责资源识别和下载接管", 650.dp, 390.dp, onClose, footer = { OutlinedButton(onClick = onClose, shape = DesktopControlShape) { Text("关闭") } }) {
        Column(Modifier.fillMaxSize().padding(18.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
            StatusPanel(if (browser.detected) "浏览器插件已连接" else if (browser.seenBefore) "插件连接已断开" else "插件未连接", if (browser.detected) "版本 ${browser.version}" else browser.message, browser.detected)
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Button(onClick = {
                    val path = state.bootstrap.root.resolve("browser-extension/chrome")
                    runCatching { NativeDialogs.openFolder(path) }.onSuccess { message = "已打开插件目录，请在 Chrome 扩展页选择“加载已解压的扩展程序”。" }
                }, shape = DesktopControlShape) { Icon(Icons.Default.FolderOpen, null, Modifier.size(17.dp)); Spacer(Modifier.width(6.dp)); Text("打开 Chromium 插件") }
                OutlinedButton(onClick = { runCatching { NativeDialogs.openUri("https://addons.mozilla.org/developers/") } }, shape = DesktopControlShape) { Text("Firefox 附加组件") }
            }
            HorizontalDivider()
            Text("安装包只内置 Chromium 插件。Firefox 请使用同版本 GitHub Release 中提交 Mozilla 审核的插件包；未签名 ZIP 仅用于临时测试。", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Text("插件未连接时，浏览器会继续使用自己的下载器。Cookie 仅在你对站点明确授权后读取，不会把主站凭据发送到未捕获的 CDN。", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            if (message.isNotBlank()) Surface(color = MaterialTheme.colorScheme.surfaceVariant, shape = MaterialTheme.shapes.small) { Text(message, Modifier.fillMaxWidth().padding(10.dp), style = MaterialTheme.typography.bodySmall) }
        }
    }
}

@Composable
fun TaskDetailsDialog(state: AppState, task: TaskItem, onClose: () -> Unit, onLog: () -> Unit) {
    var torrentFiles by remember { mutableStateOf(TorrentFiles()) }
    var selectedFiles by remember { mutableStateOf<List<Int>>(emptyList()) }
    LaunchedEffect(task.id, task.status) {
        if (task.taskType == "torrent") runCatching { state.api.torrentFiles(task.id) }.onSuccess { torrentFiles = it; selectedFiles = it.selected }
    }
    Modal(task.displayName, task.url, 790.dp, 680.dp, onClose, footer = {
        OutlinedButton(onClick = onLog, shape = DesktopControlShape) { Text("查看日志") }
        Spacer(Modifier.weight(1f))
        if ("pause" in task.availableActions) OutlinedButton(onClick = { state.perform("pause", listOf(task)) }, shape = DesktopControlShape) { Icon(Icons.Default.Pause, null, Modifier.size(16.dp)); Spacer(Modifier.width(5.dp)); Text("暂停") }
        if ("resume" in task.availableActions) Button(onClick = { state.perform("resume", listOf(task)) }, shape = DesktopControlShape) { Icon(Icons.Default.RestartAlt, null, Modifier.size(16.dp)); Spacer(Modifier.width(5.dp)); Text("恢复") }
        if ("retry" in task.availableActions) Button(onClick = { state.perform("retry", listOf(task)) }, shape = DesktopControlShape) { Text("重试") }
        if ("preview" in task.availableActions) OutlinedButton(onClick = { NativeDialogs.openUri(state.api.playerUrl(task.id)) }, shape = DesktopControlShape) { Text(if (task.status == "done") "内置播放" else "边下边播") }
        if (task.outputPath.isNotBlank()) OutlinedButton(onClick = { state.launch { state.api.openExplorer(task.outputPath) } }, shape = DesktopControlShape) { Text("所在位置") }
        if (task.outputIsFile && task.status == "done") OutlinedButton(onClick = { state.launch { state.api.launchFile(task.outputPath) } }, shape = DesktopControlShape) { Icon(Icons.Default.PlayArrow, null, Modifier.size(16.dp)); Spacer(Modifier.width(5.dp)); Text("播放") }
    }) {
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(18.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
            FlowRow(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(1.dp), verticalArrangement = Arrangement.spacedBy(1.dp)) {
                Detail("类型", task.taskType.uppercase()); Detail("状态", statusLabel(task.status)); Detail("阶段", stageLabel(task.stage)); Detail("进度", "%.1f%%".format(task.progressPercent))
                Detail("总大小", formatBytes(task.totalBytes)); Detail("已下载", formatBytes(task.downloadedBytes)); Detail("下载速度", formatSpeed(task.speedBytesPerSec)); Detail("剩余时间", formatEta(task.etaSeconds))
                Detail(if (task.taskType == "torrent") "Piece" else "分片", "${task.completedSegments}/${task.totalSegments}"); Detail(if (task.taskType == "torrent") "Peer / Seed" else "活动线程", if (task.taskType == "torrent") "${task.peerCount} / ${task.seedCount}" else "${task.activeWorkers}/${task.maxWorkers}"); Detail("重连", task.reconnectCount.toString()); Detail("更新时间", formatDate(task.updatedAt))
            }
            if (task.lastLog.isNotBlank()) InfoBlock("最近日志", task.lastLog)
            if (task.errorMessage.isNotBlank() || task.errorCode.isNotBlank()) {
                Surface(color = MaterialTheme.colorScheme.error.copy(alpha = .10f), shape = MaterialTheme.shapes.medium) {
                    Column(Modifier.fillMaxWidth().padding(12.dp), verticalArrangement = Arrangement.spacedBy(7.dp)) {
                        Text("下载失败", color = MaterialTheme.colorScheme.error, fontWeight = FontWeight.SemiBold)
                        if (task.errorCode.isNotBlank()) Text("错误代码  ${task.errorCode}", style = MaterialTheme.typography.bodySmall)
                        if (task.errorStage.isNotBlank()) Text("发生阶段  ${task.errorStage}", style = MaterialTheme.typography.bodySmall)
                        SelectionContainer { Text(task.errorMessage, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error) }
                        if (task.errorHint.isNotBlank()) Text("处理建议：${task.errorHint}", style = MaterialTheme.typography.bodySmall)
                    }
                }
            }
            if (task.taskType == "torrent" && torrentFiles.files.isNotEmpty()) {
                SectionTitle("BT 文件选择 (${selectedFiles.size}/${torrentFiles.files.size})")
                Column(Modifier.fillMaxWidth().height(180.dp).verticalScroll(rememberScrollState())) {
                    torrentFiles.files.forEach { file ->
                        Row(Modifier.fillMaxWidth().height(34.dp), verticalAlignment = Alignment.CenterVertically) {
                            Checkbox(file.index in selectedFiles, { checked -> selectedFiles = if (checked) selectedFiles + file.index else selectedFiles - file.index }, enabled = task.status !in setOf("downloading", "done"))
                            Text(file.path, Modifier.weight(1f), maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodySmall)
                            Text(formatBytes(file.size), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        }
                    }
                }
                if (task.status !in setOf("downloading", "done")) OutlinedButton(onClick = { state.launch { state.api.selectTorrentFiles(task.id, selectedFiles) } }, enabled = selectedFiles.isNotEmpty(), shape = DesktopControlShape) { Text("保存文件选择") }
            }
            if (task.outputPath.isNotBlank()) InfoBlock("输出位置", task.outputPath)
        }
    }
}

@Composable
fun LogDialog(state: AppState, task: TaskItem, onClose: () -> Unit) {
    var log by remember { mutableStateOf("正在读取日志…") }
    LaunchedEffect(task.id) { log = runCatching { state.api.log(task.id).log }.getOrElse { it.message ?: "读取失败" } }
    Modal("任务日志", task.displayName, 820.dp, 620.dp, onClose, footer = {
        OutlinedButton(onClick = { Toolkit.getDefaultToolkit().systemClipboard.setContents(StringSelection(log), null) }, shape = DesktopControlShape) { Icon(Icons.Default.ContentCopy, null, Modifier.size(16.dp)); Spacer(Modifier.width(5.dp)); Text("复制") }
        Spacer(Modifier.width(8.dp)); Button(onClick = onClose, shape = DesktopControlShape) { Text("关闭") }
    }) {
        SelectionContainer {
            Text(log, Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp), fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.bodySmall)
        }
    }
}

@Composable
private fun LabeledField(label: String, value: String, onChange: (String) -> Unit, placeholder: String = "", leading: androidx.compose.ui.graphics.vector.ImageVector? = null, readOnly: Boolean = false) {
    OutlinedTextField(
        value, onChange, Modifier.fillMaxWidth(), label = { Text(label) }, placeholder = if (placeholder.isNotBlank()) ({ Text(placeholder) }) else null,
        leadingIcon = leading?.let { icon -> ({ Icon(icon, null, Modifier.size(18.dp)) }) }, singleLine = true, readOnly = readOnly,
        textStyle = MaterialTheme.typography.bodyMedium,
    )
}

@Composable
private fun NumberField(label: String, value: Int, min: Int, max: Int, onChange: (Int) -> Unit) {
    LabeledField(label, value.toString(), { text -> text.toIntOrNull()?.coerceIn(min, max)?.let(onChange) })
}

@Composable
private fun ToggleRow(title: String, note: String, checked: Boolean, onChange: (Boolean) -> Unit) {
    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f)) { Text(title); Text(note, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant) }
        Switch(checked, onChange)
    }
}

@Composable private fun SectionTitle(value: String) { Text(value, style = MaterialTheme.typography.titleSmall, color = MaterialTheme.colorScheme.primary, modifier = Modifier.padding(top = 4.dp)) }
@Composable private fun InlineError(value: String) { Surface(color = MaterialTheme.colorScheme.error.copy(alpha = .10f), shape = MaterialTheme.shapes.small) { Text(value, Modifier.fillMaxWidth().padding(10.dp), color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall) } }

@Composable
private fun StatusPanel(title: String, note: String, active: Boolean) {
    Surface(color = MaterialTheme.colorScheme.surfaceVariant, shape = MaterialTheme.shapes.medium) {
        Row(Modifier.fillMaxWidth().padding(12.dp), verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.size(10.dp).background(if (active) Color(0xFF53B987) else MaterialTheme.colorScheme.outline, MaterialTheme.shapes.extraSmall))
            Spacer(Modifier.width(10.dp)); Column { Text(title, fontWeight = FontWeight.SemiBold); Text(note, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant) }
        }
    }
}

@Composable
private fun Detail(label: String, value: String) {
    Surface(color = MaterialTheme.colorScheme.surfaceVariant, modifier = Modifier.width(184.dp).height(60.dp)) {
        Column(Modifier.padding(10.dp), verticalArrangement = Arrangement.spacedBy(3.dp)) {
            Text(label, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Text(value, maxLines = 1, overflow = TextOverflow.Ellipsis, fontWeight = FontWeight.SemiBold)
        }
    }
}

@Composable
private fun InfoBlock(label: String, value: String) {
    Surface(color = MaterialTheme.colorScheme.surfaceVariant, shape = MaterialTheme.shapes.small) {
        Column(Modifier.fillMaxWidth().padding(11.dp), verticalArrangement = Arrangement.spacedBy(5.dp)) {
            Text(label, style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
            SelectionContainer { Text(value, style = MaterialTheme.typography.bodySmall) }
        }
    }
}
