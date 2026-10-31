package com.hlsdownloader.desktop

import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.draw.clip
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.*
import javax.swing.JFileChooser
import javax.swing.filechooser.FileNameExtensionFilter

private fun JsonObject.text(key: String) = (get(key) as? JsonPrimitive)?.contentOrNull.orEmpty()
private fun JsonObject.rows(key: String) = (get(key) as? JsonArray).orEmpty().mapNotNull { it as? JsonObject }

@Composable
internal fun ZipPreviewDialog(onDismiss: () -> Unit) {
    var source by remember { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf("") }
    var preview by remember { mutableStateOf<JsonObject?>(null) }
    val scope = rememberCoroutineScope()
    val focus = remember { FocusRequester() }
    LaunchedEffect(Unit) { delay(100); focus.requestFocus() }
    fun load() {
        busy = true; error = ""; preview = null
        scope.launch {
            runCatching { withContext(Dispatchers.IO) {
                EnginePipeClient.parityRequest("preview_zip", "preview_zip", buildJsonObject { put("source", source.trim()) })["preview"]?.jsonObject
                    ?: kotlin.error("引擎未返回 ZIP 目录")
            } }.onSuccess { preview = it }.onFailure { error = it.message ?: "ZIP 预览失败" }
            busy = false
        }
    }
    WorkbenchDialog(onDismiss, "ZIP 内容预览", "仅读取目录，不解压或执行文件。远程预览需要 Range 支持。", 760.dp,
        content = {
            DialogLabel("本地路径或 HTTP(S) 地址")
            OutlinedTextField(source, { source = it }, Modifier.fillMaxWidth(), focusRequester = focus, enabled = !busy, singleLine = true)
            DialogSecondary("选择本地 ZIP", !busy) {
                val chooser = JFileChooser().apply { fileFilter = FileNameExtensionFilter("ZIP 归档", "zip"); dialogTitle = "选择 ZIP 文件" }
                if (chooser.showOpenDialog(WorkbenchWindow.awtWindow) == JFileChooser.APPROVE_OPTION) source = chooser.selectedFile.absolutePath
            }
            if (busy) Text("正在读取 ZIP 目录…", color = muted)
            if (error.isNotEmpty()) Text(error, color = errorStrong)
            preview?.let { result ->
                val entries = result.rows("entries")
                Text("${entries.size} 项 · 已读取 ${result.text("metadata_bytes_read")} 字节元数据 · ${if (result.text("zip64") == "true") "ZIP64" else "ZIP"}", color = muted)
                if (entries.isEmpty()) Text("空归档", color = faint)
                LazyColumn(Modifier.fillMaxWidth().heightIn(max = 300.dp).clip(RoundedCornerShape(Radius.md)).border(1.dp, border, RoundedCornerShape(Radius.md)).padding(8.dp)) {
                    items(entries) { entry ->
                        Column(Modifier.fillMaxWidth().padding(vertical = 6.dp)) {
                            Text(entry.text("path"), color = ink)
                            Text("原始 ${entry.text("uncompressed_bytes")} B · 压缩 ${entry.text("compressed_bytes")} B · 方法 ${entry.text("method")}${if (entry.text("encrypted") == "true") " · 已加密" else ""}", color = faint)
                        }
                    }
                }
            }
        }, actions = {
            DialogSecondary("关闭", onClick = onDismiss)
            DialogPrimary("读取目录", !busy && source.isNotBlank(), ::load)
        })
}

@Composable
internal fun CrawlProjectsDialog(defaultDirectory: String, onDismiss: () -> Unit) {
    var projects by remember { mutableStateOf<List<JsonObject>>(emptyList()) }
    var selectedId by remember { mutableStateOf("") }
    var chosen by remember(selectedId) { mutableStateOf<Set<String>>(emptySet()) }
    var creating by remember { mutableStateOf(false) }
    var url by remember { mutableStateOf("") }
    var directory by remember { mutableStateOf(defaultDirectory) }
    var queue by remember { mutableStateOf("default") }
    var depth by remember { mutableStateOf("2") }
    var pages by remember { mutableStateOf("500") }
    var concurrency by remember { mutableStateOf("2") }
    var extensions by remember { mutableStateOf("") }
    var include by remember { mutableStateOf("") }
    var exclude by remember { mutableStateOf("") }
    var interval by remember { mutableStateOf("0") }
    var sameOrigin by remember { mutableStateOf(true) }
    var offline by remember { mutableStateOf(false) }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf("") }
    val scope = rememberCoroutineScope()
    suspend fun request(value: JsonObject): List<JsonObject> = withContext(Dispatchers.IO) {
        EnginePipeClient.parityRequest("crawl", "crawl_projects", buildJsonObject { put("request", value) }).rows("projects")
    }
    fun action(action: String, urls: Set<String>? = null) {
        busy = true; error = ""
        scope.launch {
            runCatching {
                request(buildJsonObject { put("action", action); put("project_id", selectedId); urls?.let { put("urls", JsonArray(it.map(::JsonPrimitive))) } })
                val summaries = request(buildJsonObject { put("action", "list") })
                val detail = if (selectedId.isNotEmpty()) request(buildJsonObject { put("action", "get"); put("project_id", selectedId) }).firstOrNull() else null
                summaries.map { if (it.text("id") == selectedId && detail != null) detail else it }
            }.onSuccess { projects = it }.onFailure { error = it.message ?: "项目操作失败" }
            busy = false
        }
    }
    LaunchedEffect(Unit) {
        while (true) {
            if (!busy) runCatching {
                val summaries = request(buildJsonObject { put("action", "list") })
                val id = selectedId.ifEmpty { summaries.firstOrNull()?.text("id").orEmpty() }
                val detail = if (id.isNotEmpty()) request(buildJsonObject { put("action", "get"); put("project_id", id) }).firstOrNull() else null
                summaries.map { if (it.text("id") == id && detail != null) detail else it }
            }
                .onSuccess { projects = it; if (selectedId.isEmpty()) selectedId = it.firstOrNull()?.text("id").orEmpty() }
                .onFailure { error = it.message ?: "无法读取项目" }
            delay(2000)
        }
    }
    val project = projects.find { it.text("id") == selectedId }
    val resources = project?.rows("resources").orEmpty()
    WorkbenchDialog(onDismiss, "抓站与同步项目", "先发现并预览，再确认下载；静态离线不执行网页脚本，远端删除不会删除本地文件。", 860.dp,
        content = {
            if (error.isNotEmpty()) Text(error, color = errorStrong)
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                DialogSecondary(if (creating) "返回项目" else "新建项目", !busy) { creating = !creating }
                DialogSecondary("刷新", !busy) { action("list") }
            }
            if (creating) {
                DialogLabel("入口 URL")
                OutlinedTextField(url, { url = it }, Modifier.fillMaxWidth(), singleLine = true)
                DialogLabel("输出目录（项目会创建独立子目录）")
                OutlinedTextField(directory, { directory = it }, Modifier.fillMaxWidth(), singleLine = true)
                DialogSecondary("选择目录") { chooseDirectory(directory)?.let { directory = it } }
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Column(Modifier.weight(1f)) { DialogLabel("深度 0–10"); OutlinedTextField(depth, { depth = it }, singleLine = true) }
                    Column(Modifier.weight(1f)) { DialogLabel("页面数 1–500"); OutlinedTextField(pages, { pages = it }, singleLine = true) }
                    Column(Modifier.weight(1f)) { DialogLabel("页面并发 1–2"); OutlinedTextField(concurrency, { concurrency = it }, singleLine = true) }
                }
                DialogLabel("扩展名筛选（逗号分隔，留空为全部；离线模式保存引用资源）")
                OutlinedTextField(extensions, { extensions = it }, Modifier.fillMaxWidth(), singleLine = true)
                DialogLabel("包含路径前缀 / 排除路径前缀（逗号分隔）")
                OutlinedTextField(include, { include = it }, Modifier.fillMaxWidth(), singleLine = true, placeholder = { Text("如 /files/") })
                OutlinedTextField(exclude, { exclude = it }, Modifier.fillMaxWidth(), singleLine = true, placeholder = { Text("如 /logout,/admin/") })
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Column(Modifier.weight(1f)) { DialogLabel("队列 ID"); OutlinedTextField(queue, { queue = it }, singleLine = true) }
                    Column(Modifier.weight(1f)) { DialogLabel("同步秒数（0 关闭，最少 60）"); OutlinedTextField(interval, { interval = it }, singleLine = true) }
                }
                SettingRow("同源边界", "跨域跳转也遵守此边界", sameOrigin) { sameOrigin = it }
                SettingRow("静态离线", "保存 HTML、CSS 和引用资源，成功保存后改写本地引用", offline) { offline = it }
            } else {
                LazyColumn(Modifier.fillMaxWidth().heightIn(max = 110.dp)) {
                    items(projects, key = { it.text("id") }) { item ->
                        Text("${if (item.text("id") == selectedId) "● " else "○ "}${item["spec"]?.jsonObject?.text("entry_url")} · ${item.text("status")}", color = ink,
                            modifier = Modifier.fillMaxWidth().clickable { selectedId = item.text("id") }.padding(6.dp))
                    }
                }
                project?.let {
                    Text("已访问 ${it.text("visited_pages")} 页 · 发现 ${resources.size} 个资源 · ${it.text("reason")}", color = muted)
                    Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        DialogSecondary(if (it.text("status") == "paused") "继续发现与同步" else "暂停发现与同步", !busy) { action(if (it.text("status") == "paused") "resume" else "pause") }
                        DialogSecondary("立即检查更新", !busy) { action("sync") }
                        DialogSecondary("下载全部发现", !busy && resources.isNotEmpty()) { action("enqueue", emptySet()) }
                    }
                    Text("已加入的下载使用任务列表中的暂停、继续和重试操作。", color = faint)
                    LazyColumn(Modifier.fillMaxWidth().heightIn(max = 260.dp).clip(RoundedCornerShape(Radius.md)).border(1.dp, border, RoundedCornerShape(Radius.md)).padding(6.dp)) {
                        items(resources, key = { it.text("url") }) { resource ->
                            Row(Modifier.fillMaxWidth().padding(vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                                Checkbox(resource.text("url") in chosen, { checked -> chosen = if (checked) chosen + resource.text("url") else chosen - resource.text("url") }, accessibilityLabel = "选择 ${resource.text("url")}")
                                Column(Modifier.weight(1f)) {
                                    Text(resource.text("url"), color = ink)
                                    Text("${resource.text("kind")} · ${if (resource.text("saved") == "true") "已保存" else if (resource.text("selected") == "true") "已确认下载" else "待确认"} ${resource.text("error")}", color = faint)
                                }
                            }
                        }
                    }
                }
            }
        }, actions = {
            DialogSecondary("关闭", onClick = onDismiss)
            if (creating) DialogPrimary("创建并发现", !busy && url.isNotBlank() && depth.toIntOrNull() in 0..10 && pages.toIntOrNull() in 1..500 && concurrency.toIntOrNull() in 1..2 && interval.toLongOrNull()?.let { it == 0L || it >= 60 } == true) {
                busy = true; error = ""
                scope.launch {
                    runCatching {
                        request(buildJsonObject {
                            put("action", "create")
                            put("spec", buildJsonObject {
                                put("entry_url", url.trim()); put("output_dir", directory.trim()); put("queue_id", queue.trim())
                                put("max_depth", depth.toInt()); put("max_pages", pages.toInt()); put("page_concurrency", concurrency.toInt())
                                put("same_origin", sameOrigin); put("offline", offline); put("sync_interval_secs", interval.toLong())
                                listOf("extensions" to extensions, "include_paths" to include, "exclude_paths" to exclude).forEach { (key, text) -> put(key, JsonArray(text.split(',').map(String::trim).filter(String::isNotEmpty).map(::JsonPrimitive))) }
                            })
                        })
                    }.onSuccess { selectedId = it.first().text("id"); creating = false; projects = projects + it }
                        .onFailure { error = it.message ?: "创建项目失败" }
                    busy = false
                }
            } else DialogPrimary("下载所选（${chosen.size}）", !busy && chosen.isNotEmpty()) { action("enqueue", chosen) }
        })
}
