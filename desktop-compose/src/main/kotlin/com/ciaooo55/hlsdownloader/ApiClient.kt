package com.ciaooo55.hlsdownloader

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.decodeFromJsonElement
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put
import java.net.URI
import java.net.URLEncoder
import java.net.http.HttpClient
import java.net.http.HttpRequest
import java.net.http.HttpResponse
import java.nio.charset.StandardCharsets
import java.nio.file.Files
import java.nio.file.Path
import java.time.Duration

class ApiException(val status: Int, message: String) : RuntimeException(message)

class ApiClient(
    private var port: Int = 8765,
    var token: String = "55555",
) {
    private val http = HttpClient.newBuilder()
        .connectTimeout(Duration.ofSeconds(3))
        .build()
    val json = Json { ignoreUnknownKeys = true; encodeDefaults = true; explicitNulls = false }
    private val base get() = "http://127.0.0.1:$port/api"

    fun reconfigure(newPort: Int, newToken: String) {
        port = newPort
        token = newToken
    }

    private suspend fun request(
        path: String,
        method: String = "GET",
        body: String? = null,
        authorized: Boolean = true,
        timeout: Duration = Duration.ofSeconds(30),
    ): JsonElement = withContext(Dispatchers.IO) {
        val builder = HttpRequest.newBuilder(URI.create(base + path)).timeout(timeout)
            .header("Accept", "application/json")
        if (authorized) builder.header("X-Token", token)
        if (body != null) builder.header("Content-Type", "application/json")
        builder.method(method, body?.let(HttpRequest.BodyPublishers::ofString) ?: HttpRequest.BodyPublishers.noBody())
        val response = http.send(builder.build(), HttpResponse.BodyHandlers.ofString(StandardCharsets.UTF_8))
        val parsed = runCatching { json.parseToJsonElement(response.body()) }.getOrElse { buildJsonObject {} }
        if (response.statusCode() !in 200..299) {
            val message = (parsed as? JsonObject)?.get("detail")?.jsonPrimitive?.content
                ?: "HTTP ${response.statusCode()}"
            throw ApiException(response.statusCode(), message)
        }
        parsed
    }

    private inline fun <reified T> JsonElement.decode(): T = json.decodeFromJsonElement<T>(this)
    private fun esc(value: String) = URLEncoder.encode(value, StandardCharsets.UTF_8)

    suspend fun health(): Health = request("/health", authorized = false, timeout = Duration.ofSeconds(2)).decode()
    suspend fun presenter(): PresenterStatus = request("/browser/presenter", timeout = Duration.ofSeconds(2)).decode()
    suspend fun activate(): OkResult = request("/app/activate", "POST", "{}").decode()
    suspend fun startSession(): JsonObject = request("/desktop/session/start", "POST", "{}").jsonObject
    suspend fun stopSession(): JsonObject = request("/desktop/session/stop", "POST", "{}").jsonObject
    suspend fun stopCore(): OkResult = request("/desktop/core/shutdown", "POST", "{}").decode()
    suspend fun commands(after: Long): DesktopCommands = request(
        "/desktop/session/commands?after=$after&timeout=20",
        timeout = Duration.ofSeconds(24),
    ).decode()

    suspend fun markHandoffPresented(id: String): BrowserHandoff =
        request("/desktop/handoffs/${esc(id)}/presented", "POST", "{}").decode()

    suspend fun tasks(): List<TaskItem> = request("/tasks").decode()
    suspend fun settings(): AppSettings = request("/settings").decode()
    suspend fun saveSettings(value: AppSettings): AppSettings =
        request("/settings", "POST", json.encodeToString(value)).decode()

    suspend fun createTask(value: TaskCreate): TaskItem =
        request("/tasks", "POST", json.encodeToString(value)).decode()

    suspend fun createBatch(values: List<TaskCreate>): List<TaskItem> =
        request("/tasks/batch", "POST", json.encodeToString(BatchCreate(values))).decode()

    suspend fun recognize(url: String, referer: String, origin: String, userAgent: String, cookie: String): RecognitionResult {
        val body = buildJsonObject {
            put("url", url); put("referer", referer); put("origin", origin)
            put("user_agent", userAgent); put("cookie", cookie)
        }
        return request("/recognize", "POST", body.toString()).decode()
    }

    suspend fun taskAction(id: String, action: String): OkResult =
        request("/tasks/${esc(id)}/$action", "POST", "{}").decode()

    suspend fun deleteTask(id: String, files: Boolean): OkResult =
        request("/tasks/${esc(id)}?delete_files=$files", "DELETE").decode()

    suspend fun clearCompleted(): OkResult = request("/tasks/completed", "DELETE").decode()
    suspend fun log(id: String): LogResult = request("/tasks/${esc(id)}/log").decode()
    suspend fun torrentFiles(id: String): TorrentFiles = request("/tasks/${esc(id)}/files").decode()
    suspend fun selectTorrentFiles(id: String, indexes: List<Int>): OkResult = request(
        "/tasks/${esc(id)}/files", "PUT", json.encodeToString(TorrentSelection(indexes)),
    ).decode()

    suspend fun browserHandoffs(): List<BrowserHandoff> = request("/browser/handoffs").decode()
    suspend fun browserHandoff(id: String): BrowserHandoff = request("/browser/handoffs/${esc(id)}").decode()
    suspend fun resolveHandoff(id: String, action: String, decision: BrowserHandoffDecision? = null): BrowserHandoff =
        request(
            "/browser/handoffs/${esc(id)}/$action",
            "POST",
            decision?.let { json.encodeToString(it) } ?: "{}",
        ).decode()

    suspend fun browserStatus(): BrowserStatus = request("/browser/status").decode()
    suspend fun updateInfo(force: Boolean = false): UpdateInfo = request("/update/check?force=$force").decode()
    suspend fun installUpdate(): OkResult = request("/update/install", "POST", "{}").decode()

    suspend fun openExplorer(path: String): OkResult = request(
        "/open-explorer", "POST", buildJsonObject { put("path", path) }.toString(),
    ).decode()

    suspend fun launchFile(path: String): OkResult = request(
        "/launch-file", "POST", buildJsonObject { put("path", path) }.toString(),
    ).decode()

    fun playerUrl(id: String) = "http://127.0.0.1:$port/ui?play=${esc(id)}&token=${esc(token)}"

    suspend fun uploadTorrent(path: Path, title: String = ""): TaskItem = withContext(Dispatchers.IO) {
        val boundary = "----HLSDownloader${System.nanoTime()}"
        val prefix = buildString {
            append("--$boundary\r\nContent-Disposition: form-data; name=\"title\"\r\n\r\n$title\r\n")
            append("--$boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"")
            append(path.fileName.toString().replace("\"", ""))
            append("\"\r\nContent-Type: application/x-bittorrent\r\n\r\n")
        }.toByteArray(StandardCharsets.UTF_8)
        val suffix = "\r\n--$boundary--\r\n".toByteArray(StandardCharsets.UTF_8)
        val request = HttpRequest.newBuilder(URI.create("$base/tasks/torrent-file"))
            .timeout(Duration.ofMinutes(2))
            .header("X-Token", token)
            .header("Content-Type", "multipart/form-data; boundary=$boundary")
            .POST(HttpRequest.BodyPublishers.concat(
                HttpRequest.BodyPublishers.ofByteArray(prefix),
                HttpRequest.BodyPublishers.ofFile(path),
                HttpRequest.BodyPublishers.ofByteArray(suffix),
            )).build()
        val response = http.send(request, HttpResponse.BodyHandlers.ofString(StandardCharsets.UTF_8))
        val parsed = json.parseToJsonElement(response.body())
        if (response.statusCode() !in 200..299) {
            val message = (parsed as? JsonObject)?.get("detail")?.jsonPrimitive?.content ?: "HTTP ${response.statusCode()}"
            throw ApiException(response.statusCode(), message)
        }
        parsed.decode<TaskItem>()
    }
}

fun loadApiConfig(root: Path): Pair<Int, String> {
    val local = System.getenv("LOCALAPPDATA")?.let { Path.of(it, "HLS Downloader", "config.json") }
    val candidates = listOfNotNull(local, root.resolve("config.json"), root.resolve("config.default.json"))
    val json = Json { ignoreUnknownKeys = true }
    for (path in candidates) {
        if (!Files.isRegularFile(path)) continue
        runCatching {
            val value = json.parseToJsonElement(Files.readString(path)).jsonObject
            val port = value["port"]?.jsonPrimitive?.content?.toIntOrNull() ?: 8765
            val token = value["token"]?.jsonPrimitive?.content ?: "55555"
            return port to token
        }
    }
    return 8765 to "55555"
}
