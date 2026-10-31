package com.hlsdownloader.desktop

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import com.hlsdownloader.desktop.icons.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import java.net.URI

@Composable internal fun BrowserHandoffDialog(
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
    // 目录初值只在接管请求首次出现时读取一次设置，避免设置对象更新时重置用户输入
    var directory by remember(offer.handoffId) { mutableStateOf(directoryFor(category)) }
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
                Surface(Modifier.fillMaxWidth(), color = selectedSurface, shape = RoundedCornerShape(Radius.md)) {
                    Row(Modifier.padding(14.dp), verticalAlignment = Alignment.CenterVertically) {
                        Icon(Icons.Outlined.Downloading, "下载文件", tint = blue, modifier = Modifier.size(26.dp))
                        Spacer(Modifier.width(11.dp))
                        Column(Modifier.weight(1f)) {
                            Text(filename.ifBlank { offer.title.ifBlank { "新下载" } }, color = ink, fontSize = TypeScale.title, fontWeight = FontWeight.SemiBold, maxLines = 2, overflow = TextOverflow.Ellipsis)
                            Text(
                                listOf(resourceKindLabel(offer.resourceKind), extension, offer.mimeType.takeIf { it.isNotBlank() }, if (offer.size > 0) formatBytes(offer.size) else "大小未知").filterNotNull().joinToString(" · "),
                                color = muted,
                                fontSize = TypeScale.caption,
                                modifier = Modifier.padding(top = 3.dp),
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                            )
                            if (downloadHost.isNotBlank()) Text(downloadHost, color = faint, fontSize = TypeScale.micro, modifier = Modifier.padding(top = 2.dp), maxLines = 1, overflow = TextOverflow.Ellipsis)
                        }
                    }
                }
                duplicate?.let {
                    Spacer(Modifier.height(9.dp))
                    Surface(Modifier.fillMaxWidth(), color = warnSurface, shape = RoundedCornerShape(Radius.md)) {
                        Row(Modifier.padding(horizontal = 11.dp, vertical = 9.dp), verticalAlignment = Alignment.CenterVertically) {
                            Icon(Icons.Outlined.WarningAmber, "重复任务", tint = warningColor, modifier = Modifier.size(17.dp))
                            Spacer(Modifier.width(8.dp))
                            Text("已有同一地址的任务：${it.filename}（${it.status}）", color = ink, fontSize = TypeScale.caption, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        }
                    }
                }
                Spacer(Modifier.height(13.dp))
                DialogLabel("文件名")
                OutlinedTextField(filename, { filename = it }, Modifier.fillMaxWidth(), enabled = !busy, singleLine = true, isError = filename.isNotBlank() && !validFilename, shape = RoundedCornerShape(Radius.md), supportingText = { if (filename.isNotBlank() && !validFilename) Text("文件名不能包含路径或控制字符", fontSize = TypeScale.micro) })
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
                        ) { Text(choice.label, fontSize = TypeScale.caption, fontWeight = if (category == choice) FontWeight.SemiBold else FontWeight.Normal) }
                    }
                }
                Spacer(Modifier.height(10.dp))
                DialogLabel("保存到")
                Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                    OutlinedTextField(directory, { directory = it }, Modifier.weight(1f), enabled = !busy, singleLine = true, shape = RoundedCornerShape(Radius.md), placeholder = { Text("使用下载引擎默认目录") })
                    Spacer(Modifier.width(7.dp))
                    Button(onClick = { chooseDirectory(directory, "选择下载保存位置")?.let { directory = it } }, enabled = !busy, colors = ButtonDefaults.buttonColors(rail, ink), border = BorderStroke(1.dp, border)) {
                        Icon(Icons.Outlined.FolderOpen, "选择保存文件夹", modifier = Modifier.size(17.dp))
                        Spacer(Modifier.width(6.dp)); Text("选择", fontSize = TypeScale.caption)
                    }
                }
                Row(Modifier.fillMaxWidth().padding(top = 7.dp), verticalAlignment = Alignment.CenterVertically) {
                    Checkbox(rememberDirectory, { rememberDirectory = it }, accessibilityLabel = "记住此分类的保存位置")
                    Spacer(Modifier.width(8.dp)); Text("记住“${category.label}”文件的保存位置", color = ink, fontSize = TypeScale.caption)
                }
                Spacer(Modifier.height(11.dp))
                Surface(Modifier.fillMaxWidth(), color = dialogSurface, shape = RoundedCornerShape(Radius.md), border = BorderStroke(1.dp, border)) {
                    Column(Modifier.padding(11.dp), verticalArrangement = Arrangement.spacedBy(5.dp)) {
                        Row(Modifier.fillMaxWidth()) {
                            Icon(Icons.Outlined.GppGood, "请求上下文已继承", tint = successColor, modifier = Modifier.size(16.dp))
                            Spacer(Modifier.width(7.dp)); Text("网站请求上下文", color = ink, fontSize = TypeScale.caption, fontWeight = FontWeight.SemiBold)
                            Spacer(Modifier.weight(1f)); Text("由下载引擎安全保管", color = successColor, fontSize = TypeScale.micro)
                        }
                        DetailLine("来源网页", sourceHost.ifBlank { "未捕获" })
                        DetailLine("来源地址", safeResourceLocation(source))
                        DetailLine("下载地址", safeResourceLocation(offer.url))
                        Text("支持沿用 Referer、Origin、User-Agent、Cookie 与 Authorization；敏感值只由下载引擎保管。", color = faint, fontSize = TypeScale.micro, lineHeight = TypeScale.lineMicro)
                    }
                }
                if (sourceHost.isNotBlank()) Row(Modifier.fillMaxWidth().padding(top = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                    Checkbox(suppressSiteKind, { suppressSiteKind = it }, accessibilityLabel = "以后不再提示此网站的同类资源")
                    Spacer(Modifier.width(8.dp)); Text("以后不再自动提示 $sourceHost 的${resourceKindLabel(offer.resourceKind)}", color = ink, fontSize = TypeScale.caption)
                }
            }
        },
        actions = {
            DialogSecondary("保留浏览器下载") { if (!busy) onReject(suppressSiteKind) }
            DialogPrimary(if (busy) "处理中…" else "确认下载", enabled = validFilename && !busy, onClick = accept)
        },
    )
}

private fun handoffCategory(offer: HandoffOfferDto): TaskCategory =
    downloadCategory(offer.filename, offer.resourceKind)

internal fun canonicalHandoffUrl(value: String) = value.substringBefore('#').trimEnd('/')

internal fun resourceKindLabel(kind: String) = when (kind.lowercase()) {
    "hls" -> "HLS 媒体"
    "dash" -> "DASH 媒体"
    "live" -> "直播流"
    "torrent" -> "BT 任务"
    "ftp" -> "FTP 文件"
    "sftp" -> "SFTP 文件"
    else -> "文件"
}
