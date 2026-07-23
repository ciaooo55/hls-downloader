package com.ciaooo55.hlsdownloader

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.Download
import androidx.compose.material.icons.filled.FolderOpen
import androidx.compose.material.icons.filled.Warning
import androidx.compose.material3.Button
import androidx.compose.material3.Checkbox
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.DpSize
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Window
import androidx.compose.ui.window.rememberWindowState

@Composable
fun HandoffWindow(state: AppState, item: BrowserHandoff, dark: Boolean) {
    val fallback = runCatching { java.net.URI.create(item.url).path.substringAfterLast('/').ifBlank { "download" } }.getOrDefault("download")
    var filename by remember(item.id) { mutableStateOf(item.filename.ifBlank { fallback }) }
    var category by remember(item.id) { mutableStateOf(downloadCategory(filename, item.mimeType)) }
    var directory by remember(item.id) { mutableStateOf(state.settings.browserCategoryDirs[category] ?: state.settings.downloadDir) }
    var rememberDirectory by remember(item.id) { mutableStateOf(true) }
    // AWT reports the requested size in physical pixels on some scaled Windows
    // desktops while Compose lays out density-independent content. Leave enough
    // room for all four categories and the fixed action footer at 125–150% DPI.
    val windowState = rememberWindowState(size = DpSize(560.dp, 680.dp))

    Window(
        onCloseRequest = { if (!state.busy) state.resolveHandoff(item, "cancel") },
        title = "下载文件信息 - HLS Downloader",
        state = windowState,
        alwaysOnTop = true,
        resizable = true,
    ) {
        HlsTheme(dark) {
            Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.surface) {
                Column(Modifier.fillMaxSize()) {
                    Row(Modifier.fillMaxWidth().padding(16.dp, 14.dp, 10.dp, 12.dp), verticalAlignment = Alignment.Top) {
                        Column(Modifier.weight(1f)) {
                            Text("下载文件信息", style = MaterialTheme.typography.titleLarge)
                            Text("浏览器已暂停，由 HLS Downloader 接管${if (state.handoffs.size > 1) " · 还有 ${state.handoffs.size - 1} 个待确认" else ""}", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        }
                        IconButton(onClick = { state.resolveHandoff(item, "cancel") }, enabled = !state.busy) { Icon(Icons.Default.Close, "取消下载") }
                    }
                    androidx.compose.material3.HorizontalDivider()
                    Column(Modifier.weight(1f).fillMaxWidth().verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                        if (item.duplicate) {
                            Surface(color = MaterialTheme.colorScheme.error.copy(alpha = .10f), shape = MaterialTheme.shapes.small) {
                                Row(Modifier.fillMaxWidth().padding(10.dp), verticalAlignment = Alignment.Top) {
                                    Icon(Icons.Default.Warning, null, Modifier.size(17.dp), tint = MaterialTheme.colorScheme.error)
                                    Spacer(Modifier.width(8.dp)); Column {
                                        Text("下载列表中已有相同链接", fontWeight = FontWeight.SemiBold)
                                        Text(item.duplicateMessage.ifBlank { "仍可继续下载，也可以取消。" }, style = MaterialTheme.typography.bodySmall)
                                    }
                                }
                            }
                        }
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Icon(Icons.Default.Download, null, Modifier.size(24.dp), tint = MaterialTheme.colorScheme.primary)
                            Spacer(Modifier.width(10.dp)); Column(Modifier.weight(1f)) {
                                Text(filename, maxLines = 1, overflow = TextOverflow.Ellipsis, fontWeight = FontWeight.SemiBold)
                                Text("${item.mimeType.ifBlank { "类型未知" }} · ${formatBytes(item.size)}", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                            }
                        }
                        Text(hostOf(item.url), maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        OutlinedTextField(filename, { filename = it }, Modifier.fillMaxWidth(), label = { Text("文件名") }, singleLine = true)
                        Text("分类", style = MaterialTheme.typography.labelLarge)
                        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                            listOf("media", "program", "archive", "other").forEach { value ->
                                val selected = category == value
                                if (selected) FilledTonalButton(onClick = {}, shape = DesktopControlShape, modifier = Modifier.weight(1f)) { Text(categoryLabel(value)) }
                                else OutlinedButton(onClick = {
                                    category = value
                                    directory = state.settings.browserCategoryDirs[value] ?: state.settings.downloadDir
                                }, shape = DesktopControlShape, modifier = Modifier.weight(1f)) { Text(categoryLabel(value)) }
                            }
                        }
                        Row(horizontalArrangement = Arrangement.spacedBy(7.dp), verticalAlignment = Alignment.CenterVertically) {
                            OutlinedTextField(directory, { directory = it }, Modifier.weight(1f), label = { Text("保存到") }, singleLine = true)
                            IconButton(onClick = { NativeDialogs.chooseFolder(directory)?.let { directory = it } }) { Icon(Icons.Default.FolderOpen, "选择目录") }
                        }
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Checkbox(rememberDirectory, { rememberDirectory = it })
                            Text("记住“${categoryLabel(category)}”文件的保存位置", style = MaterialTheme.typography.bodySmall)
                        }
                        Text(item.url, maxLines = 3, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                    androidx.compose.material3.HorizontalDivider()
                    Row(Modifier.fillMaxWidth().padding(14.dp, 10.dp), horizontalArrangement = Arrangement.End) {
                        OutlinedButton(onClick = { state.resolveHandoff(item, "cancel") }, enabled = !state.busy, shape = DesktopControlShape) { Icon(Icons.Default.Close, null, Modifier.size(16.dp)); Spacer(Modifier.width(5.dp)); Text("取消") }
                        Spacer(Modifier.width(8.dp))
                        Button(
                            onClick = { state.resolveHandoff(item, "accept", BrowserHandoffDecision(filename.trim(), directory.trim(), category, rememberDirectory)) },
                            enabled = filename.isNotBlank() && directory.isNotBlank() && !state.busy,
                            shape = DesktopControlShape,
                        ) { Icon(Icons.Default.Download, null, Modifier.size(16.dp)); Spacer(Modifier.width(5.dp)); Text(if (state.busy) "处理中…" else "开始下载") }
                    }
                }
            }
        }
    }
}
