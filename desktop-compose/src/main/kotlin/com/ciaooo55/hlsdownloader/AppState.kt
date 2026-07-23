package com.ciaooo55.hlsdownloader

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import kotlinx.coroutines.CoroutineExceptionHandler
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import java.awt.Toolkit
import java.awt.datatransfer.DataFlavor

enum class TaskFilter(val label: String) {
    ALL("全部任务"), ACTIVE("进行中"), DONE("已完成"), MEDIA("媒体"), PROGRAM("程序"), ARCHIVE("压缩包"), OTHER("其他")
}

class AppState(
    val bootstrap: BootstrapResult,
    private val onActivate: () -> Unit,
    private val onExit: () -> Unit,
) {
    val api = bootstrap.api
    private val handler = CoroutineExceptionHandler { _, throwable -> error = throwable.message ?: "操作失败" }
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main + handler)
    val tasks = mutableStateListOf<TaskItem>()
    val selected = mutableStateListOf<String>()
    val handoffs = mutableStateListOf<BrowserHandoff>()
    var settings by mutableStateOf(AppSettings())
    var filter by mutableStateOf(TaskFilter.ALL)
    var query by mutableStateOf("")
    var version by mutableStateOf("")
    var loading by mutableStateOf(true)
    var error by mutableStateOf("")
    var busy by mutableStateOf(false)
    var browserStatus by mutableStateOf(BrowserStatus())
    var updateInfo by mutableStateOf<UpdateInfo?>(null)
    private var pollJob: Job? = null
    private var commandJob: Job? = null

    val filteredTasks: List<TaskItem> get() = tasks.filter { task ->
        val category = downloadCategory(task.displayName, task.mimeType)
        val matchesFilter = when (filter) {
            TaskFilter.ALL -> true
            TaskFilter.ACTIVE -> task.status !in setOf("done", "failed", "canceled", "unsupported")
            TaskFilter.DONE -> task.status == "done"
            TaskFilter.MEDIA -> category == "media"
            TaskFilter.PROGRAM -> category == "program"
            TaskFilter.ARCHIVE -> category == "archive"
            TaskFilter.OTHER -> category == "other"
        }
        val needle = query.trim().lowercase()
        matchesFilter && (needle.isBlank() || listOf(task.displayName, task.url, task.status, task.outputPath).any { needle in it.lowercase() })
    }

    val selectedTasks: List<TaskItem> get() = tasks.filter { it.id in selected }

    fun start() {
        scope.launch {
            refreshAll()
            loading = false
        }
        pollJob = scope.launch {
            while (true) {
                delay(800)
                runCatching { refreshTasks() }.onFailure { error = it.message ?: "无法刷新任务" }
            }
        }
        commandJob = scope.launch {
            var sequence = 0L
            while (true) {
                val result = runCatching { api.commands(sequence) }.getOrElse { delay(800); continue }
                sequence = maxOf(sequence, result.sequence)
                result.commands.forEach { command ->
                    when (command.kind) {
                        "activate" -> onActivate()
                        "shutdown" -> onExit()
                        "handoff" -> receiveHandoff(command.handoffId)
                    }
                }
            }
        }
    }

    suspend fun refreshAll() {
        val health = api.health()
        version = health.version
        settings = api.settings()
        api.reconfigure(settings.port, settings.token)
        refreshTasks()
        handoffs.clear()
        handoffs.addAll(api.browserHandoffs().filter { it.status == "pending" })
        browserStatus = runCatching { api.browserStatus() }.getOrDefault(BrowserStatus())
        updateInfo = runCatching { api.updateInfo() }.getOrNull()
    }

    suspend fun refreshTasks() {
        val fresh = api.tasks()
        tasks.clear(); tasks.addAll(fresh)
        selected.removeAll { id -> fresh.none { it.id == id } }
    }

    private suspend fun receiveHandoff(id: String) {
        val item = runCatching { api.browserHandoff(id) }.getOrNull() ?: return
        if (handoffs.none { it.id == id }) handoffs.add(item)
        runCatching { api.markHandoffPresented(id) }
        onActivate()
    }

    fun select(id: String, additive: Boolean = false, range: Boolean = false) {
        if (range && selected.isNotEmpty()) {
            val list = filteredTasks
            val anchor = list.indexOfFirst { it.id == selected.last() }
            val target = list.indexOfFirst { it.id == id }
            if (anchor >= 0 && target >= 0) {
                if (!additive) selected.clear()
                val rangeIds = list.subList(minOf(anchor, target), maxOf(anchor, target) + 1).map { it.id }
                rangeIds.forEach { if (it !in selected) selected.add(it) }
                return
            }
        }
        if (!additive) selected.clear()
        if (id in selected) selected.remove(id) else selected.add(id)
    }

    fun selectAllVisible() {
        val ids = filteredTasks.map { it.id }
        if (ids.isNotEmpty() && ids.all { it in selected }) selected.removeAll(ids.toSet())
        else ids.forEach { if (it !in selected) selected.add(it) }
    }

    fun selectVisibleRange(anchor: Int, target: Int, baseSelection: Set<String> = emptySet()) {
        val list = filteredTasks
        if (anchor !in list.indices || target !in list.indices) return
        val next = LinkedHashSet(baseSelection)
        list.subList(minOf(anchor, target), maxOf(anchor, target) + 1).forEach { next.add(it.id) }
        selected.clear()
        selected.addAll(next)
    }

    fun perform(action: String, targets: List<TaskItem> = selectedTasks, deleteFiles: Boolean = false) {
        if (targets.isEmpty()) return
        scope.launch {
            busy = true; error = ""
            try {
                targets.forEach { task ->
                    if (action == "delete") api.deleteTask(task.id, deleteFiles) else api.taskAction(task.id, action)
                }
                refreshTasks()
            } finally { busy = false }
        }
    }

    fun clearCompleted() = scope.launch { api.clearCompleted(); refreshTasks() }
    fun createTask(value: TaskCreate, done: () -> Unit = {}) = scope.launch {
        busy = true; error = ""
        try { api.createTask(value); refreshTasks(); done() } finally { busy = false }
    }
    fun createBatch(values: List<TaskCreate>, done: () -> Unit = {}) = scope.launch {
        busy = true; error = ""
        try { api.createBatch(values); refreshTasks(); done() } finally { busy = false }
    }
    fun uploadTorrent(path: java.nio.file.Path, done: () -> Unit = {}) = scope.launch {
        busy = true; error = ""
        try { api.uploadTorrent(path); refreshTasks(); done() } finally { busy = false }
    }
    fun saveSettings(value: AppSettings, done: () -> Unit = {}) = scope.launch {
        busy = true; error = ""
        try {
            settings = api.saveSettings(value)
            api.reconfigure(settings.port, settings.token)
            done()
        } finally { busy = false }
    }
    fun resolveHandoff(item: BrowserHandoff, action: String, decision: BrowserHandoffDecision? = null) = scope.launch {
        busy = true; error = ""
        try { api.resolveHandoff(item.id, action, decision); handoffs.removeAll { it.id == item.id }; refreshTasks() }
        finally { busy = false }
    }
    fun showError(message: String) { error = message }
    fun dismissError() { error = "" }
    fun pasteUrl(): String = runCatching {
        Toolkit.getDefaultToolkit().systemClipboard.getData(DataFlavor.stringFlavor)?.toString().orEmpty()
    }.getOrDefault("")
    fun launch(block: suspend () -> Unit) = scope.launch { block() }

    fun dispose() {
        pollJob?.cancel(); commandJob?.cancel(); scope.coroutineContext[Job]?.cancel()
    }
}
