package com.hlsdownloader.desktop

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import com.hlsdownloader.desktop.icons.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

@Composable
internal fun NewTaskDialog(
    onDismiss: () -> Unit,
    initialUrl: String,
    defaults: EngineSettingsDto,
    probing: Boolean,
    creating: Boolean = false,
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
    WorkbenchDialog(onDismiss, "新建下载", "创建文件、媒体、远程协议或 BT 下载任务", 760.dp, dismissible = !probing && !creating, content = {
        Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(Radius.md)).background(surface2).padding(3.dp)) { listOf("基本", "连接", "请求", "计划").forEach { item -> TextButton(onClick = { tab = item }, Modifier.weight(1f).reportControlBounds("newtask.tab.$item"), containerColor = segmentBackground(tab == item, rail, Color.Transparent), hoverColor = if (tab == item) rail else surface3, pressedColor = if (tab == item) rail else pressedSurface, pressScale = SEGMENT_SCALE) { Text(item, color = if (tab == item) blue else muted, fontSize = TypeScale.caption, fontWeight = if (tab == item) FontWeight.SemiBold else FontWeight.Normal) } } }
        Spacer(Modifier.height(14.dp))
        when (tab) {
            "基本" -> {
                DialogLabel("下载链接或 cURL 命令")
                OutlinedTextField(
                    url,
                    { url = it },
                    Modifier.fillMaxWidth().reportControlBounds("newtask.url").onPreviewKeyEvent { event ->
                        if (event.type == KeyEventType.KeyDown && event.key == Key.Enter &&
                            !curlMode && !probing && !creating && validInput && parsedHeaders.isSuccess
                        ) {
                            onCreate(buildDraft())
                            true
                        } else false
                    },
                    focusRequester = urlFocus,
                    placeholder = { Text("粘贴链接、磁力链接或浏览器“复制为 cURL”") },
                    singleLine = !curlMode,
                    minLines = if (curlMode) 3 else 1,
                    maxLines = if (curlMode) 5 else 1,
                    isError = url.isNotBlank() && !validInput,
                    shape = RoundedCornerShape(Radius.md),
                    supportingText = {
                        Text(
                            when {
                                url.isNotBlank() && !validInput -> "链接格式无效或 cURL 命令不完整"
                                curlMode -> "已识别 cURL；请求头、Cookie 与 POST 请求体由下载引擎安全导入"
                                normalized != null -> "类型：${EnginePipeClient.recognizeResourceKind(normalized)}"
                                else -> "等待输入"
                            },
                            fontSize = TypeScale.micro,
                        )
                    },
                )
                Spacer(Modifier.height(8.dp)); DialogLabel("文件名（留空自动识别）"); OutlinedTextField(filename, { filename = it }, Modifier.fillMaxWidth(), singleLine = true, shape = RoundedCornerShape(Radius.md)); Spacer(Modifier.height(8.dp)); DialogLabel("保存到"); Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) { OutlinedTextField(directory, { directory = it }, Modifier.weight(1f), singleLine = true, shape = RoundedCornerShape(Radius.md)); Spacer(Modifier.width(8.dp)); DialogSecondary("选择目录") { chooseDirectory(directory, "选择下载保存目录")?.let { directory = it } } }; SettingRow("允许重复任务", "同一资源已存在时仍创建新任务", allowDuplicate) { allowDuplicate = it }
            }
            "连接" -> {
                DialogLabel("并发连接数（0 使用默认值）"); OutlinedTextField(concurrency, { concurrency = it.filter(Char::isDigit) }, Modifier.fillMaxWidth(), singleLine = true, shape = RoundedCornerShape(Radius.md)); Spacer(Modifier.height(9.dp)); DialogLabel("任务限速 KiB/s（0 不限制）"); OutlinedTextField(speed, { speed = it.filter(Char::isDigit) }, Modifier.fillMaxWidth(), singleLine = true, shape = RoundedCornerShape(Radius.md)); Spacer(Modifier.height(9.dp)); DialogLabel("代理地址（可选）"); OutlinedTextField(proxy, { proxy = it }, Modifier.fillMaxWidth(), singleLine = true, shape = RoundedCornerShape(Radius.md)); Spacer(Modifier.height(9.dp)); DialogLabel("校验和（算法:值，可选）"); OutlinedTextField(checksum, { checksum = it }, Modifier.fillMaxWidth(), singleLine = true, shape = RoundedCornerShape(Radius.md), placeholder = { Text("sha256:...") }); Spacer(Modifier.height(9.dp)); DialogLabel("备用下载地址（每行一个）"); OutlinedTextField(mirrors, { mirrors = it }, Modifier.fillMaxWidth(), minLines = 3, maxLines = 4, shape = RoundedCornerShape(Radius.md), placeholder = { Text("https://mirror.example.com/file.bin") }); Text("仅普通 HTTP(S) 文件使用；媒体清单、BT 与远程协议会忽略。", color = faint, fontSize = TypeScale.micro, modifier = Modifier.padding(top = 5.dp))
            }
            "请求" -> {
                if (curlMode) {
                    Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(Radius.md)).background(selectedSurface).padding(11.dp), verticalAlignment = Alignment.CenterVertically) {
                        Icon(Icons.Outlined.Security, null, tint = blue, modifier = Modifier.size(18.dp)); Spacer(Modifier.width(9.dp)); Text("cURL 中的请求上下文由下载引擎解析并加密保存，下方字段作为显式覆盖值。", color = muted, fontSize = TypeScale.micro)
                    }
                    Spacer(Modifier.height(10.dp))
                }
                DialogLabel("请求方式")
                Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(Radius.md)).background(surface2).padding(3.dp)) {
                    listOf("GET", "POST", "HEAD").forEach { value -> TextButton(onClick = { requestMethod = value }, Modifier.weight(1f).reportControlBounds("newtask.method.$value"), containerColor = segmentBackground(requestMethod == value, selectedSurface, Color.Transparent), hoverColor = if (requestMethod == value) selectedSurface else surface3, pressedColor = if (requestMethod == value) selectedSurface else pressedSurface, pressScale = SEGMENT_SCALE) { Text(value, color = if (requestMethod == value) blue else muted, fontSize = TypeScale.caption) } }
                }
                Spacer(Modifier.height(9.dp))
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    Column(Modifier.weight(1f)) {
                        DialogLabel("Referer")
                        OutlinedTextField(referer, { referer = it }, Modifier.fillMaxWidth(), singleLine = true, shape = RoundedCornerShape(Radius.md))
                    }
                    Column(Modifier.weight(1f)) {
                        DialogLabel("Origin")
                        OutlinedTextField(origin, { origin = it }, Modifier.fillMaxWidth(), singleLine = true, shape = RoundedCornerShape(Radius.md))
                    }
                }
                Spacer(Modifier.height(9.dp))
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    Column(Modifier.weight(1f)) {
                        DialogLabel("User-Agent")
                        OutlinedTextField(userAgent, { userAgent = it }, Modifier.fillMaxWidth(), singleLine = true, shape = RoundedCornerShape(Radius.md))
                    }
                    Column(Modifier.weight(1f)) {
                        DialogLabel("Cookie")
                        OutlinedTextField(cookie, { cookie = it }, Modifier.fillMaxWidth(), singleLine = true, visualTransformation = PasswordVisualTransformation(), shape = RoundedCornerShape(Radius.md), placeholder = { Text("下载引擎加密保存") })
                    }
                }
                Spacer(Modifier.height(9.dp)); DialogLabel("其他请求头（每行“名称: 值”）"); OutlinedTextField(requestHeaders, { requestHeaders = it }, Modifier.fillMaxWidth(), minLines = 3, maxLines = 3, isError = parsedHeaders.isFailure, shape = RoundedCornerShape(Radius.md), placeholder = { Text("Authorization: Bearer ...\nX-Playback-Token: ...") }); Text(parsedHeaders.exceptionOrNull()?.message ?: "敏感请求头只保存在下载引擎的加密凭据中。", color = if (parsedHeaders.isFailure) errorStrong else faint, fontSize = TypeScale.micro, modifier = Modifier.padding(top = 5.dp))
            }
            else -> {
                DialogLabel("计划开始（ISO 时间或留空）"); OutlinedTextField(startAt, { startAt = it }, Modifier.fillMaxWidth(), singleLine = true, shape = RoundedCornerShape(Radius.md)); Spacer(Modifier.height(9.dp)); DialogLabel("计划停止（ISO 时间或留空）"); OutlinedTextField(stopAt, { stopAt = it }, Modifier.fillMaxWidth(), singleLine = true, shape = RoundedCornerShape(Radius.md)); Spacer(Modifier.height(9.dp)); DialogLabel("完成后动作"); Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(Radius.md)).background(surface2).padding(3.dp)) { POWER_ACTION_OPTIONS.forEach { (value, label) -> TextButton(onClick = { completionAction = value }, Modifier.weight(1f), containerColor = segmentBackground(completionAction == value, selectedSurface, Color.Transparent), contentPadding = PaddingValues(horizontal = 4.dp), hoverColor = if (completionAction == value) selectedSurface else surface3, pressedColor = if (completionAction == value) selectedSurface else pressedSurface, pressScale = SEGMENT_SCALE) { Text(label, color = if (completionAction == value) blue else muted, fontSize = TypeScale.micro) } } }
            }
        }
    }, actions = {
        DialogSecondary("取消", onClick = onDismiss, enabled = !probing && !creating)
        TextButton(
            onClick = { if (!probing && normalized != null && parsedHeaders.isSuccess) onProbe(buildDraft()) },
            enabled = !probing && !creating && normalized != null && parsedHeaders.isSuccess,
            contentPadding = PaddingValues(horizontal = 12.dp),
            modifier = Modifier.reportControlBounds("newtask.probe"),
        ) { Text(if (probing) "分析中…" else "分析资源", fontSize = TypeScale.body, color = if (!probing && normalized != null && parsedHeaders.isSuccess) muted else faint) }
        DialogPrimary(if (creating) "创建中…" else "创建下载", !probing && !creating && validInput && parsedHeaders.isSuccess) {
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
@Composable internal fun BatchAddDialog(onDismiss: () -> Unit, busy: Boolean, onCreate: suspend (List<String>) -> List<Pair<String, String>>) {
    var urls by remember { mutableStateOf("") }
    var feedback by remember { mutableStateOf("") }
    var failureDetails by remember { mutableStateOf("") }
    val scope = rememberCoroutineScope()
    val inputFocus = remember { FocusRequester() }
    LaunchedEffect(Unit) { delay(120); inputFocus.requestFocus() }
    val entries = urls.lineSequence().map(String::trim).filter(String::isNotBlank).toList()
    val invalidLines = urls.lineSequence().mapIndexedNotNull { index, line ->
        (index + 1).takeIf { line.isNotBlank() && runCatching { EnginePipeClient.normalizeDownloadUrl(line.trim()) }.isFailure }
    }.toList()
    WorkbenchDialog(onDismiss, "批量添加", "每行一个下载链接，请修正无效链接后创建", 620.dp, dismissible = !busy, content = {
        DialogLabel("链接列表")
        OutlinedTextField(urls, { urls = it }, Modifier.fillMaxWidth().heightIn(min = 180.dp), focusRequester = inputFocus, enabled = !busy,
            placeholder = { Text("https://example.com/file.zip") }, minLines = 7, maxLines = 10, shape = RoundedCornerShape(Radius.md))
        Text(if (invalidLines.isEmpty()) "共 ${entries.size} 条链接" else "第 ${invalidLines.take(8).joinToString("、")} 行链接无效${if (invalidLines.size > 8) "等 ${invalidLines.size} 行" else ""}，请修正后创建",
            color = if (invalidLines.isNotEmpty()) warningColor else faint, fontSize = TypeScale.caption, modifier = Modifier.padding(top = 7.dp))
        if (feedback.isNotEmpty()) {
            Text(feedback, color = warningColor, fontSize = TypeScale.caption, modifier = Modifier.padding(top = 9.dp))
            Text(failureDetails, color = muted, fontSize = TypeScale.micro, modifier = Modifier.padding(top = 5.dp))
        }
    }, actions = {
        DialogSecondary("取消", onClick = onDismiss, enabled = !busy)
        DialogPrimary(if (busy) "添加中…" else "添加 ${entries.size} 条链接", !busy && entries.isNotEmpty() && invalidLines.isEmpty()) {
            scope.launch {
                val failures = onCreate(entries)
                if (failures.isNotEmpty()) {
                    urls = failures.joinToString("\n") { it.first }
                    feedback = "成功处理 ${entries.size - failures.size} 条（新建或复用已有任务）；${failures.size} 条失败，已保留在输入框中。"
                    failureDetails = failures.take(3).joinToString("\n") { it.second } + if (failures.size > 3) "\n其余 ${failures.size - 3} 个任务也未创建。" else ""
                }
            }
        }
    })
}
@Composable
internal fun HarvestDialog(
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
                shape = RoundedCornerShape(Radius.md),
                isError = url.isNotBlank() && !validUrl,
            )
            Text(
                "不会执行网页脚本，也不会继续打开子页面；单次最多返回 100 个资源。",
                color = faint,
                fontSize = TypeScale.caption,
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
                shape = RoundedCornerShape(Radius.md),
                isError = !validReferer,
            )
            Text("用于需要来源页校验的文件和媒体地址。", color = faint, fontSize = TypeScale.caption, modifier = Modifier.padding(top = 7.dp))
        },
        actions = {
            DialogSecondary("取消", onClick = onDismiss)
            DialogPrimary("抓取本页链接", validUrl && validReferer) { onHarvest(url.trim(), referer.trim()) }
        },
    )
}
@Composable internal fun SettingsSection(title: String, content: @Composable ColumnScope.() -> Unit) = Column(Modifier.fillMaxWidth().padding(end = 5.dp, bottom = 14.dp), content = { Text(title, color = ink, fontSize = TypeScale.body, fontWeight = FontWeight.SemiBold); Spacer(Modifier.height(12.dp)); content(); Spacer(Modifier.height(8.dp)) })
@Composable internal fun SettingRow(label: String, detail: String, checked: Boolean, onChecked: (Boolean) -> Unit) = Row(Modifier.fillMaxWidth().padding(vertical = 5.dp), verticalAlignment = Alignment.CenterVertically) { Column(Modifier.weight(1f)) { Text(label, color = ink, fontSize = TypeScale.body, fontWeight = FontWeight.Medium); Text(detail, color = faint, fontSize = TypeScale.micro, modifier = Modifier.padding(top = 3.dp)) }; Switch(checked, onChecked, accessibilityLabel = label) }
