package com.hlsdownloader.desktop

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.rememberScrollbarAdapter
import androidx.compose.foundation.ScrollbarStyle
import androidx.compose.foundation.VerticalScrollbar
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

private data class SettingsTab(val label: String, val icon: ImageVector)

@Composable
internal fun FullSettingsDialog(
    onDismiss: () -> Unit,
    current: EngineSettingsDto,
    discoveredDevices: List<CastDeviceDto> = emptyList(),
    discoveringDevices: Boolean = false,
    onDiscoverDevices: (String) -> Unit = {},
    initialTab: String = "通用",
    onSave: (EngineSettingsDto, String?) -> Unit,
) {
    val tabs = remember { listOf(
        SettingsTab("通用", Icons.Outlined.Tune), SettingsTab("下载", Icons.Outlined.Downloading),
        SettingsTab("计划", Icons.Outlined.Schedule), SettingsTab("网络", Icons.Outlined.Language),
        SettingsTab("媒体", Icons.Outlined.Movie), SettingsTab("BT", Icons.Outlined.Hub),
        SettingsTab("投屏与推送", Icons.Outlined.Cast), SettingsTab("安全", Icons.Outlined.Security),
        SettingsTab("浏览器", Icons.Outlined.Extension), SettingsTab("外观", Icons.Outlined.Palette),
    ) }
    var selected by remember(initialTab) { mutableStateOf(initialTab) }
    var draft by remember(current) { mutableStateOf(current) }
    var concurrency by remember(current) { mutableStateOf(current.defaultConcurrency.toString()) }
    var speedLimit by remember(current) { mutableStateOf(current.speedLimitKib.toString()) }
    var queueMax by remember(current) { mutableStateOf(current.queueMax.toString()) }
    var retryMax by remember(current) { mutableStateOf(current.autoRetryMax.toString()) }
    var chunkMb by remember(current) { mutableStateOf(current.httpChunkSizeMb.toString()) }
    var scheduleLimit by remember(current) { mutableStateOf(current.scheduleKib.toString()) }
    var liveMinutes by remember(current) { mutableStateOf(current.liveRecordMaxMinutes.toString()) }
    var takeoverMinimum by remember(current) { mutableStateOf(current.takeoverMinimumBytes.toString()) }
    var harvestMinimum by remember(current) { mutableStateOf(current.harvestMinimumBytes.toString()) }
    var btUploadLimit by remember(current) { mutableStateOf(current.btUploadLimitKib.toString()) }
    var btConnections by remember(current) { mutableStateOf(current.btMaxConnections.toString()) }
    var defaultCookie by remember(current.defaultCookieConfigured) { mutableStateOf("") }
    var clearDefaultCookie by remember(current.defaultCookieConfigured) { mutableStateOf(false) }
    var deviceMode by remember(initialTab) { mutableStateOf("cast") }
    val contentScroll = rememberScrollState()

    LaunchedEffect(selected) { contentScroll.scrollTo(0) }

    WorkbenchDialog(onDismiss, "设置", "HLS Downloader ${Product.version}", 880.dp, scrollable = false, content = {
        Row(Modifier.fillMaxWidth().height(420.dp), verticalAlignment = Alignment.Top) {
            Column(Modifier.width(138.dp).fillMaxHeight().padding(end = 12.dp), verticalArrangement = Arrangement.spacedBy(3.dp)) {
                tabs.forEach { tab ->
                    val active = tab.label == selected
                    Row(
                        Modifier.fillMaxWidth().height(38.dp).clip(RoundedCornerShape(7.dp))
                            .background(if (active) selectedSurface else surface2)
                            .clickable { selected = tab.label }.padding(horizontal = 10.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Icon(tab.icon, null, modifier = Modifier.size(17.dp), tint = if (active) blue else muted)
                        Spacer(Modifier.width(8.dp)); Text(tab.label, fontSize = 12.sp, fontWeight = if (active) FontWeight.SemiBold else FontWeight.Normal, color = ink)
                    }
                }
            }
            VerticalDivider(Modifier.fillMaxHeight().padding(end = 14.dp), color = border)
            Box(Modifier.weight(1f).fillMaxHeight()) {
            Column(
                Modifier.fillMaxSize().verticalScroll(contentScroll)
                    .padding(end = 14.dp, bottom = 18.dp),
            ) {
                when (selected) {
                    "通用" -> SettingsSection("文件与运行") {
                        V7DirectoryField("默认下载目录", draft.downloadDirectory, "选择默认下载目录") { draft = draft.copy(downloadDirectory = it) }
                        V7DirectoryField("缓存与过程文件目录", draft.tempDirectory, "选择缓存与过程文件目录") { draft = draft.copy(tempDirectory = it) }
                        SettingRow("自动分类目录", "根据媒体、程序、压缩包和其他类型选择保存位置", draft.autoCategory) { draft = draft.copy(autoCategory = it) }
                        if (draft.autoCategory) {
                            V7DirectoryField("媒体目录", draft.categoryDirMedia, "选择媒体目录") { draft = draft.copy(categoryDirMedia = it) }
                            V7DirectoryField("程序目录", draft.categoryDirProgram, "选择程序目录") { draft = draft.copy(categoryDirProgram = it) }
                            V7DirectoryField("压缩包目录", draft.categoryDirArchive, "选择压缩包目录") { draft = draft.copy(categoryDirArchive = it) }
                            V7DirectoryField("其他目录", draft.categoryDirOther, "选择其他文件目录") { draft = draft.copy(categoryDirOther = it) }
                        }
                        SettingRow("监视剪贴板", "检测剪贴板中的下载链接", draft.clipboardWatch) { draft = draft.copy(clipboardWatch = it) }
                        SettingRow("恢复未完成任务", "启动下载引擎后恢复中断任务", draft.resumeInterrupted) { draft = draft.copy(resumeInterrupted = it) }
                        SettingRow("开机启动", "登录 Windows 后启动下载引擎", draft.startOnLogin) { draft = draft.copy(startOnLogin = it) }
                        SettingRow("完成提示音", "下载完成后播放系统声音", draft.completionSoundEnabled) { draft = draft.copy(completionSoundEnabled = it) }
                        SettingRow("下载进度窗口", "显示紧凑的活动任务进度", draft.progressWindowEnabled) { draft = draft.copy(progressWindowEnabled = it) }
                        SettingRow("完成通知", "显示本机完成提示", draft.completePopupEnabled) { draft = draft.copy(completePopupEnabled = it) }
                    }
                    "下载" -> SettingsSection("下载引擎") {
                        V7NumberField("默认并发连接数", concurrency) { concurrency = it }
                        V7NumberField("最大同时任务数", queueMax) { queueMax = it }
                        V7NumberField("全局限速 KiB/s（0 不限制）", speedLimit) { speedLimit = it }
                        V7NumberField("HTTP 分段大小 MiB", chunkMb) { chunkMb = it }
                        V7NumberField("失败自动重试次数", retryMax) { retryMax = it }
                        V7Choice("同名文件处理", listOf("rename" to "自动重命名", "overwrite" to "覆盖", "skip" to "跳过"), draft.existingFilePolicy) { draft = draft.copy(existingFilePolicy = it) }
                        SettingRow("默认允许重复", "相同链接可以创建独立任务", draft.allowDuplicate) { draft = draft.copy(allowDuplicate = it) }
                        SettingRow("保留过程文件", "保留分片、清单和调试文件", draft.keepTempFiles) { draft = draft.copy(keepTempFiles = it) }
                    }
                    "计划" -> SettingsSection("限速与队列计划") {
                        SettingRow("启用定时限速", "在指定时间段使用独立限速", draft.scheduleEnabled) { draft = draft.copy(scheduleEnabled = it) }
                        V7Field("限速开始", draft.scheduleStart) { draft = draft.copy(scheduleStart = it) }
                        V7Field("限速结束", draft.scheduleEnd) { draft = draft.copy(scheduleEnd = it) }
                        V7NumberField("计划限速 KiB/s", scheduleLimit) { scheduleLimit = it }
                        SettingRow("队列自动开始", "到达计划时间后开始排队任务", draft.queueAutoStartEnabled) { draft = draft.copy(queueAutoStartEnabled = it) }
                        V7Field("自动开始时间", draft.queueAutoStartTime) { draft = draft.copy(queueAutoStartTime = it) }
                        SettingRow("队列自动停止", "到达计划时间后暂停活动任务", draft.queueAutoStopEnabled) { draft = draft.copy(queueAutoStopEnabled = it) }
                        V7Field("自动停止时间", draft.queueAutoStopTime) { draft = draft.copy(queueAutoStopTime = it) }
                        V7Field("活动星期（1-7，以逗号分隔）", draft.queueActiveDays) { draft = draft.copy(queueActiveDays = it) }
                        V7Choice("全部完成后", listOf("none" to "无", "sleep" to "睡眠", "shutdown" to "关机"), draft.completionPowerAction) { draft = draft.copy(completionPowerAction = it) }
                    }
                    "网络" -> SettingsSection("代理与站点") {
                        Row(
                            Modifier.fillMaxWidth().clip(RoundedCornerShape(7.dp))
                                .background(selectedSurface).padding(horizontal = 12.dp, vertical = 10.dp),
                            verticalAlignment = Alignment.Top,
                        ) {
                            Icon(Icons.Outlined.VerifiedUser, null, modifier = Modifier.size(17.dp), tint = blue)
                            Spacer(Modifier.width(9.dp))
                            Column {
                                Text("浏览器任务自动沿用页面请求", fontSize = 12.sp, fontWeight = FontWeight.SemiBold, color = ink)
                                Text(
                                    "下载引擎优先使用页面和资源源站实际的 Referer、Origin、User-Agent 及同源凭据。",
                                    color = muted, fontSize = 10.sp, lineHeight = 15.sp, modifier = Modifier.padding(top = 3.dp),
                                )
                            }
                        }
                        Spacer(Modifier.height(12.dp))
                        V7Choice("代理模式", listOf("direct" to "直连", "system" to "系统代理", "manual" to "手动代理"), draft.proxyMode) { draft = draft.copy(proxyMode = it) }
                        if (draft.proxyMode == "manual") V7Field("代理地址", draft.proxyUrl) { draft = draft.copy(proxyUrl = it) }
                        V7Field("代理绕过列表", draft.proxyBypass) { draft = draft.copy(proxyBypass = it) }
                        DialogLabel("手动任务默认请求头（可选）")
                        Text("只在任务没有浏览器请求上下文时使用。", color = muted, fontSize = 10.sp, modifier = Modifier.padding(bottom = 8.dp))
                        V7Field("默认 Referer", draft.defaultReferer) { draft = draft.copy(defaultReferer = it) }
                        V7Field("默认 Origin", draft.defaultOrigin) { draft = draft.copy(defaultOrigin = it) }
                        DialogLabel("默认 Cookie")
                        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                            OutlinedTextField(
                                defaultCookie,
                                { defaultCookie = it; clearDefaultCookie = false },
                                Modifier.weight(1f),
                                singleLine = true,
                                shape = RoundedCornerShape(7.dp),
                                placeholder = { Text(if (current.defaultCookieConfigured) "已安全保存；留空保持不变" else "例如 sessionid=…") },
                                visualTransformation = PasswordVisualTransformation(),
                            )
                            if (current.defaultCookieConfigured || defaultCookie.isNotEmpty()) {
                                Spacer(Modifier.width(8.dp))
                                DialogSecondary(if (clearDefaultCookie) "已清除" else "清除") { defaultCookie = ""; clearDefaultCookie = true }
                            }
                        }
                        Text("下载引擎会安全保存凭据，界面不会回显内容。", color = muted, fontSize = 10.sp, modifier = Modifier.padding(top = 5.dp, bottom = 9.dp))
                        V7Field("默认 User-Agent", draft.defaultUserAgent, lines = 2) { draft = draft.copy(defaultUserAgent = it) }
                        V7Field("允许的域名（逗号分隔，留空不限制）", draft.allowedHosts) { draft = draft.copy(allowedHosts = it) }
                        V7Field("站点规则", draft.siteRules, lines = 5) { draft = draft.copy(siteRules = it) }
                    }
                    "媒体" -> SettingsSection("媒体处理与直播") {
                        V7Field("FFmpeg 路径", draft.ffmpegPath) { draft = draft.copy(ffmpegPath = it) }
                        V7NumberField("直播录制上限（分钟，0 不限制）", liveMinutes) { liveMinutes = it }
                        SettingRow("下载外挂字幕", "保存 HLS/DASH 字幕轨道", draft.downloadSubtitles) { draft = draft.copy(downloadSubtitles = it) }
                        SettingRow("跳过广告片段", "跳过清单明确标记的广告段", draft.skipAdSegments) { draft = draft.copy(skipAdSegments = it) }
                    }
                    "BT" -> SettingsSection("BT 与种子") {
                        V7DirectoryField("种子监视目录", draft.torrentWatch, "选择种子监视目录") { draft = draft.copy(torrentWatch = it) }
                        SettingRow("监视新种子", "自动导入监视目录中新出现的种子、磁力或 URL 文件", draft.torrentWatchEnabled) { draft = draft.copy(torrentWatchEnabled = it) }
                        V7NumberField("BT 上传上限 KiB/s（0 不限制）", btUploadLimit) { btUploadLimit = it }
                        V7NumberField("BT 最大 Peer 连接", btConnections) { btConnections = it }
                        SettingRow("启用 DHT 节点发现", "Tracker 没有返回足够节点时继续通过 DHT 查找", draft.btEnableDht) { draft = draft.copy(btEnableDht = it) }
                    }
                    "投屏与推送" -> SettingsSection("局域网投屏与 TVBox 推送") {
                        V7Choice(
                            "选择功能",
                            listOf("cast" to "投屏", "tvbox" to "TVBox 推送"),
                            deviceMode,
                        ) { deviceMode = it }
                        val visibleDevices = settingsDevicesForMode(discoveredDevices, deviceMode)
                        val isTvBoxMode = deviceMode == "tvbox"
                        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                            Column(Modifier.weight(1f)) {
                                Text(if (isTvBoxMode) "TVBox 接收端" else "投屏设备", fontSize = 12.sp, fontWeight = FontWeight.SemiBold, color = ink)
                                Text(
                                    if (isTvBoxMode) "查找已开启推送接收服务的电视盒子。" else "查找支持 DLNA 或 Chromecast 的电视和音箱。",
                                    color = muted, fontSize = 10.sp, modifier = Modifier.padding(top = 2.dp),
                                )
                            }
                            TextButton(onClick = { onDiscoverDevices(deviceMode) }, enabled = !discoveringDevices) {
                                Icon(if (discoveringDevices) Icons.Outlined.HourglassTop else Icons.Outlined.Refresh, null, Modifier.size(16.dp), tint = blue)
                                Spacer(Modifier.width(6.dp))
                                Text(
                                    if (discoveringDevices) "正在扫描" else if (isTvBoxMode) "扫描 TVBox" else "扫描投屏设备",
                                    fontSize = 11.sp, color = blue,
                                )
                            }
                        }
                        Spacer(Modifier.height(8.dp))
                        if (visibleDevices.isEmpty()) {
                            Box(
                                Modifier.fillMaxWidth().height(72.dp).clip(RoundedCornerShape(7.dp)).background(surface2),
                                contentAlignment = Alignment.Center,
                            ) {
                                Text(
                                    when {
                                        discoveringDevices -> if (isTvBoxMode) "正在搜索 TVBox 接收端…" else "正在搜索 DLNA / Chromecast…"
                                        isTvBoxMode -> "未发现 TVBox；可扫描或填写下方手工地址"
                                        else -> "未发现投屏设备；请确认电视已开启投屏功能"
                                    },
                                    color = muted, fontSize = 11.sp,
                                )
                            }
                        } else {
                            visibleDevices.forEach { device ->
                                val active = draft.preferredCastDeviceId == device.id
                                Row(
                                    Modifier.fillMaxWidth().heightIn(min = 52.dp).clip(RoundedCornerShape(7.dp))
                                        .background(if (active) selectedSurface else surface2)
                                        .clickable { draft = draft.copy(preferredCastDeviceId = device.id) }
                                        .padding(horizontal = 11.dp, vertical = 8.dp),
                                    verticalAlignment = Alignment.CenterVertically,
                                ) {
                                    Icon(castDeviceIcon(device), null, Modifier.size(18.dp), tint = if (active) blue else muted)
                                    Spacer(Modifier.width(10.dp))
                                    Column(Modifier.weight(1f)) {
                                        Text(device.label, color = ink, fontSize = 11.sp, fontWeight = FontWeight.Medium, maxLines = 1)
                                        Text(castDeviceDescription(device), color = muted, fontSize = 10.sp, maxLines = 1)
                                    }
                                    if (active) Text("首选", color = blue, fontSize = 10.sp, fontWeight = FontWeight.SemiBold)
                                }
                                Spacer(Modifier.height(5.dp))
                            }
                        }
                        if (isTvBoxMode) {
                            Spacer(Modifier.height(10.dp))
                            V7Field("TVBox 手工地址（可选）", draft.tvboxEndpoint) { draft = draft.copy(tvboxEndpoint = it) }
                            Text("自动扫描未发现接收端时，可填写完整的 HTTP(S) 地址。", color = muted, fontSize = 10.sp)
                        } else {
                            Spacer(Modifier.height(10.dp))
                            Text("投屏会建立可控制的播放会话；TVBox 推送是另一种接收协议，请切换到“TVBox 推送”。", color = muted, fontSize = 10.sp, lineHeight = 15.sp)
                        }
                    }
                    "安全" -> SettingsSection("发布与扫描") {
                        SettingRow("完成后病毒扫描", "使用 Windows Defender 或指定扫描程序", draft.avScanEnabled) { draft = draft.copy(avScanEnabled = it) }
                        V7Field("扫描命令", draft.avScanCommand, lines = 2) { draft = draft.copy(avScanCommand = it) }
                        SettingRow("发现威胁时标记失败", "保留文件并将任务标记为失败，关闭后只记录扫描结果", draft.avScanFailOnThreat) { draft = draft.copy(avScanFailOnThreat = it) }
                        Text("凭据由下载引擎安全保存，界面只显示是否已配置。", color = muted, fontSize = 11.sp, lineHeight = 17.sp)
                    }
                    "浏览器" -> SettingsSection("浏览器下载接管") {
                        SettingRow("接管浏览器下载", "插件识别到资源后显示确认窗口", draft.takeoverEnabled) { draft = draft.copy(takeoverEnabled = it) }
                        V7NumberField("最小接管大小（字节）", takeoverMinimum) { takeoverMinimum = it }
                        V7NumberField("页面抓取最小大小（字节）", harvestMinimum) { harvestMinimum = it }
                        Text("插件会自动确认与下载器的兼容状态。", color = muted, fontSize = 11.sp)
                    }
                    else -> SettingsSection("外观与可访问性") {
                        SettingRow("深色模式", "切换工作台与所有弹窗的配色", draft.darkMode) { draft = draft.copy(darkMode = it) }
                        SettingRow("减弱动画", "关闭非必要过渡并降低动态反馈", draft.reduceMotion) { draft = draft.copy(reduceMotion = it) }
                        Text("界面字体使用 Segoe UI Variable / Microsoft YaHei UI，并跟随 Windows DPI。", color = muted, fontSize = 11.sp, lineHeight = 17.sp)
                    }
                }
            }
            VerticalScrollbar(
                rememberScrollbarAdapter(contentScroll),
                Modifier.align(Alignment.CenterEnd).fillMaxHeight().width(6.dp),
                style = ScrollbarStyle(
                    minimalHeight = 28.dp,
                    thickness = 6.dp,
                    shape = RoundedCornerShape(3.dp),
                    hoverDurationMillis = 160,
                    unhoverColor = muted.copy(alpha = 0.42f),
                    hoverColor = blue.copy(alpha = 0.82f),
                ),
            )
            }
        }
    }, actions = {
        DialogSecondary("取消", onDismiss)
        DialogPrimary("保存设置") {
            onSave(draft.copy(
                defaultConcurrency = concurrency.toLongOrNull()?.coerceIn(1, 128) ?: current.defaultConcurrency,
                speedLimitKib = speedLimit.toLongOrNull()?.coerceIn(0, 10_000_000) ?: current.speedLimitKib,
                queueMax = queueMax.toLongOrNull()?.coerceIn(1, 128) ?: current.queueMax,
                autoRetryMax = retryMax.toLongOrNull()?.coerceIn(0, 20) ?: current.autoRetryMax,
                httpChunkSizeMb = chunkMb.toLongOrNull()?.coerceIn(1, 64) ?: current.httpChunkSizeMb,
                scheduleKib = scheduleLimit.toLongOrNull()?.coerceIn(0, 10_000_000) ?: current.scheduleKib,
                liveRecordMaxMinutes = liveMinutes.toLongOrNull()?.coerceIn(0, 10080) ?: current.liveRecordMaxMinutes,
                takeoverMinimumBytes = takeoverMinimum.toLongOrNull()?.coerceAtLeast(0) ?: current.takeoverMinimumBytes,
                harvestMinimumBytes = harvestMinimum.toLongOrNull()?.coerceAtLeast(0) ?: current.harvestMinimumBytes,
                btUploadLimitKib = btUploadLimit.toLongOrNull()?.coerceIn(0, 1_048_576) ?: current.btUploadLimitKib,
                btMaxConnections = btConnections.toLongOrNull()?.coerceIn(10, 1_000) ?: current.btMaxConnections,
            ), when {
                clearDefaultCookie -> ""
                defaultCookie.isNotEmpty() -> defaultCookie
                else -> null
            })
            onDismiss()
        }
    })
}

@Composable private fun V7Field(label: String, value: String, lines: Int = 1, onValue: (String) -> Unit) {
    DialogLabel(label)
    OutlinedTextField(value, onValue, Modifier.fillMaxWidth(), singleLine = lines == 1, minLines = lines, maxLines = lines.coerceAtLeast(3), shape = RoundedCornerShape(7.dp))
    Spacer(Modifier.height(9.dp))
}

@Composable private fun V7NumberField(label: String, value: String, onValue: (String) -> Unit) = V7Field(label, value) { onValue(it.filter(Char::isDigit)) }

@Composable private fun V7DirectoryField(label: String, value: String, title: String, onValue: (String) -> Unit) {
    DialogLabel(label)
    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
        OutlinedTextField(value, onValue, Modifier.weight(1f), singleLine = true, shape = RoundedCornerShape(7.dp))
        Spacer(Modifier.width(8.dp))
        DialogSecondary("选择目录") { chooseDirectory(value, title)?.let(onValue) }
    }
    Spacer(Modifier.height(9.dp))
}

@Composable private fun V7Choice(label: String, options: List<Pair<String, String>>, selected: String, onSelect: (String) -> Unit) {
    DialogLabel(label)
    Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(7.dp)).background(surface2).padding(3.dp)) {
        options.forEach { (value, text) -> TextButton(onClick = { onSelect(value) }, Modifier.weight(1f).clip(RoundedCornerShape(5.dp)).background(if (selected == value) selectedSurface else androidx.compose.ui.graphics.Color.Transparent)) { Text(text, fontSize = 11.sp, color = if (selected == value) blue else muted) } }
    }
    Spacer(Modifier.height(9.dp))
}

private fun castDeviceIcon(device: CastDeviceDto): ImageVector = when {
    device.serviceType.equals("chromecast", true) || device.id.startsWith("chromecast:") -> Icons.Outlined.CastConnected
    device.serviceType.equals("tvbox", true) || device.id.startsWith("tvbox:") -> Icons.Outlined.Tv
    else -> Icons.Outlined.Devices
}

internal fun isTvBoxDevice(device: CastDeviceDto): Boolean =
    device.serviceType.equals("tvbox", true) || device.id.startsWith("tvbox:")

internal fun settingsDevicesForMode(devices: List<CastDeviceDto>, mode: String): List<CastDeviceDto> =
    if (mode == "tvbox") devices.filter(::isTvBoxDevice) else devices.filterNot(::isTvBoxDevice)

internal fun castDeviceDescription(device: CastDeviceDto): String {
    val kind = when {
        device.serviceType.equals("chromecast", true) || device.id.startsWith("chromecast:") -> "Chromecast"
        device.serviceType.equals("tvbox", true) || device.id.startsWith("tvbox:") -> "TVBox"
        else -> "DLNA"
    }
    val address = device.location.ifBlank { device.controlUrl }.trim()
    return if (address.isBlank()) kind else "$kind · $address"
}
