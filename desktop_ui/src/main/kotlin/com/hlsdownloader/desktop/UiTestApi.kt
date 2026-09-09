package com.hlsdownloader.desktop

import com.sun.net.httpserver.HttpExchange
import com.sun.net.httpserver.HttpServer
import java.awt.EventQueue
import java.awt.KeyboardFocusManager
import java.awt.Point
import java.awt.Rectangle
import java.awt.Robot
import java.awt.Component
import java.awt.Toolkit
import java.awt.Window
import java.awt.datatransfer.DataFlavor
import java.awt.datatransfer.StringSelection
import java.awt.event.InputEvent
import java.awt.event.KeyEvent
import java.awt.image.BufferedImage
import java.io.ByteArrayOutputStream
import java.net.InetAddress
import java.net.InetSocketAddress
import java.nio.charset.StandardCharsets
import java.nio.file.Files
import java.nio.file.Path
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors
import javax.imageio.ImageIO
import javax.swing.SwingUtilities
import kotlinx.serialization.Serializable
import kotlinx.serialization.SerialName
import kotlinx.serialization.encodeToString

private const val TEST_API_HEADER = "X-HLS-Test-Token"
private const val MAX_ACTION_BYTES = 64 * 1024

@Serializable
internal data class UiTestAction(
    val type: String,
    val x: Int? = null,
    val y: Int? = null,
    @SerialName("to_x") val toX: Int? = null,
    @SerialName("to_y") val toY: Int? = null,
    val index: Int? = null,
    val key: String? = null,
    val modifiers: List<String> = emptyList(),
    val text: String? = null,
    val delta: Int? = null,
)

internal fun validateUiTestAction(action: UiTestAction, width: Int, height: Int): String? = when (action.type) {
    "activate" -> null
    "click", "right_click" -> when {
        action.x == null || action.y == null -> "click actions require x and y"
        action.x !in 0 until width || action.y !in 0 until height -> "coordinates are outside the current window"
        else -> null
    }
    "drag" -> when {
        action.x == null || action.y == null || action.toX == null || action.toY == null -> "drag actions require x, y, to_x and to_y"
        action.x !in 0 until width || action.y !in 0 until height || action.toX !in 0 until width || action.toY !in 0 until height -> "coordinates are outside the current window"
        else -> null
    }
    "scroll" -> when {
        action.x == null || action.y == null || action.delta == null || action.delta == 0 -> "scroll action requires x, y and a non-zero delta"
        action.x !in 0 until width || action.y !in 0 until height -> "coordinates are outside the current window"
        action.delta !in -20..20 -> "scroll delta must be between -20 and 20"
        else -> null
    }
    "select_task" -> if (action.index == null || action.index < 0) "select_task requires a non-negative index" else null
    "open_task_menu" -> when {
        action.index == null || action.index < 0 -> "open_task_menu requires a non-negative index"
        action.x == null || action.y == null -> "open_task_menu requires x and y"
        action.x !in 0 until width || action.y !in 0 until height -> "coordinates are outside the current window"
        else -> null
    }
    "key" -> if (action.key.isNullOrBlank()) "key action requires key" else null
    "type" -> when {
        action.text == null -> "type action requires text"
        action.text.length > 8192 -> "text exceeds 8192 characters"
        else -> null
    }
    else -> "unsupported action type"
}

internal object UiTestState {
    @Volatile private var selectedTaskIds: List<String> = emptyList()
    @Volatile private var inputTarget: String = ""
    @Volatile private var taskSelector: ((Int, Boolean, Boolean) -> Set<String>)? = null
    @Volatile private var taskContextOpener: ((Int, Int, Int) -> Unit)? = null
    @Volatile private var contextMenuPosition: List<Int> = emptyList()
    @Volatile private var contextMenuTaskIds: List<String> = emptyList()
    @Volatile private var contextMenuActions: List<String> = emptyList()

    fun updateSelection(ids: Set<String>) {
        selectedTaskIds = ids.sorted()
    }

    fun updateInputTarget(component: Component) {
        inputTarget = "${component.javaClass.name};mouse=${component.mouseListeners.size};motion=${component.mouseMotionListeners.size}"
    }

    fun installTaskSelector(selector: ((Int, Boolean, Boolean) -> Set<String>)?) {
        taskSelector = selector
    }

    fun installTaskContextOpener(opener: ((Int, Int, Int) -> Unit)?) {
        taskContextOpener = opener
    }

    fun openTaskMenu(index: Int, x: Int, y: Int) {
        taskContextOpener?.invoke(index, x, y)
            ?: throw IllegalStateException("task table is not available")
    }

    fun updateContextMenu(position: androidx.compose.ui.unit.IntOffset, taskIds: Set<String>, actions: List<String>) {
        contextMenuPosition = listOf(position.x, position.y)
        contextMenuTaskIds = taskIds.sorted()
        contextMenuActions = actions
    }

    fun selectTask(index: Int, modifiers: List<String>) {
        val normalized = modifiers.map(String::uppercase)
        val next = taskSelector?.invoke(index, "SHIFT" in normalized, normalized.any { it == "CTRL" || it == "CONTROL" })
            ?: throw IllegalStateException("task table is not available")
        updateSelection(next)
    }

    fun snapshot(): UiSelectionSnapshot = UiSelectionSnapshot(
        selectedTaskIds.size,
        selectedTaskIds,
        inputTarget,
        contextMenuPosition,
        contextMenuTaskIds,
        contextMenuActions,
    )
}

@Serializable
internal data class UiSelectionSnapshot(
    val selectedCount: Int,
    val selectedTaskIds: List<String>,
    val inputTarget: String,
    val contextMenuPosition: List<Int>,
    val contextMenuTaskIds: List<String>,
    val contextMenuActions: List<String>,
)

internal class UiTestApi private constructor(
    private val server: HttpServer,
    private val executor: ExecutorService,
    private val window: Window,
    private val token: String,
    private val robot: Robot,
    private val previousAlwaysOnTop: Boolean,
) : AutoCloseable {
    val port: Int get() = server.address.port

    override fun close() {
        server.stop(0)
        executor.shutdownNow()
        runCatching { onEventThread { window.isAlwaysOnTop = previousAlwaysOnTop } }
    }

    private fun installRoutes() {
        server.createContext("/health") { exchange ->
            handle(exchange, "GET") {
                jsonResponse(exchange, 200, """{"ok":true,"product":"HLS Downloader","version":"${Product.version}"}""")
            }
        }
        server.createContext("/window") { exchange ->
            handle(exchange, "GET") {
                val snapshot = onEventThread {
                    val icon = window.iconImages.maxByOrNull { it.getWidth(null) * it.getHeight(null) }
                    WindowSnapshot(window.x, window.y, window.width, window.height, window.isActive, window.isShowing, window.iconImages.size, icon?.getWidth(null) ?: 0, icon?.getHeight(null) ?: 0)
                }
                jsonResponse(exchange, 200, protocolJson.encodeToString(snapshot))
            }
        }
        server.createContext("/state") { exchange ->
            handle(exchange, "GET") {
                jsonResponse(exchange, 200, protocolJson.encodeToString(UiTestState.snapshot()))
            }
        }
        server.createContext("/screen") { exchange ->
            handle(exchange, "GET") {
                val image = withFocusedWindow {
                    val bounds = java.awt.GraphicsEnvironment.getLocalGraphicsEnvironment().defaultScreenDevice.defaultConfiguration.bounds
                    robot.createScreenCapture(bounds)
                }
                pngResponse(exchange, image)
            }
        }
        server.createContext("/screenshot") { exchange ->
            handle(exchange, "GET") {
                val image = withFocusedWindow {
                    if (exchange.requestURI.query == "mode=paint") {
                        onEventThread {
                            BufferedImage(window.width, window.height, BufferedImage.TYPE_INT_ARGB).also { image ->
                                val graphics = image.createGraphics()
                                try {
                                    window.printAll(graphics)
                                } finally {
                                    graphics.dispose()
                                }
                            }
                        }
                    } else {
                        val bounds = onEventThread {
                            val location = window.locationOnScreen
                            Rectangle(location.x, location.y, window.width, window.height)
                        }
                        robot.createScreenCapture(bounds)
                    }
                }
                pngResponse(exchange, image)
            }
        }
        server.createContext("/action") { exchange ->
            handle(exchange, "POST") {
                val declaredLength = exchange.requestHeaders.getFirst("Content-Length")?.toLongOrNull()
                require(declaredLength == null || declaredLength <= MAX_ACTION_BYTES) { "action body is too large" }
                val body = exchange.requestBody.readNBytes(MAX_ACTION_BYTES + 1)
                require(body.size <= MAX_ACTION_BYTES) { "action body is too large" }
                val action = protocolJson.decodeFromString<UiTestAction>(body.toString(StandardCharsets.UTF_8))
                val dimensions = onEventThread { window.width to window.height }
                validateUiTestAction(action, dimensions.first, dimensions.second)?.let { throw IllegalArgumentException(it) }
                perform(action)
                jsonResponse(exchange, 200, """{"ok":true,"action":"${escapeJson(action.type)}"}""")
            }
        }
    }

    private fun handle(exchange: HttpExchange, method: String, block: () -> Unit) {
        try {
            exchange.responseHeaders.set("Cache-Control", "no-store")
            if (exchange.requestMethod != method) {
                jsonResponse(exchange, 405, """{"ok":false,"error":"method_not_allowed"}""")
                return
            }
            if (exchange.requestHeaders.getFirst(TEST_API_HEADER) != token) {
                jsonResponse(exchange, 401, """{"ok":false,"error":"unauthorized"}""")
                return
            }
            block()
        } catch (error: IllegalArgumentException) {
            jsonResponse(exchange, 400, """{"ok":false,"error":"${escapeJson(error.message ?: "invalid_request")}"}""")
        } catch (error: Exception) {
            jsonResponse(exchange, 500, """{"ok":false,"error":"${escapeJson(error.message ?: "internal_error")}"}""")
        } finally {
            exchange.close()
        }
    }

    private fun perform(action: UiTestAction) {
        withFocusedWindow {
            when (action.type) {
                "activate" -> Unit
                "click", "right_click" -> {
                    dispatchMouseClick(action.x!!, action.y!!, action.type == "right_click", action.modifiers)
                }
                "drag" -> {
                    dispatchMouseDrag(action.x!!, action.y!!, action.toX!!, action.toY!!, action.modifiers)
                }
                "scroll" -> dispatchMouseWheel(action.x!!, action.y!!, action.delta!!, action.modifiers)
                "select_task" -> onEventThread { UiTestState.selectTask(action.index!!, action.modifiers) }
                "open_task_menu" -> onEventThread { UiTestState.openTaskMenu(action.index!!, action.x!!, action.y!!) }
                "key" -> pressKey(action.key!!, action.modifiers)
                "type" -> typeText(action.text!!)
            }
            robot.waitForIdle()
            Thread.sleep(120)
        }
    }

    private fun <T> withFocusedWindow(block: () -> T): T {
        val previousAlwaysOnTop = onEventThread {
            val previous = window.isAlwaysOnTop
            window.setAlwaysOnTop(true)
            val active = KeyboardFocusManager.getCurrentKeyboardFocusManager().activeWindow
            if (!belongsToWindow(active, window)) {
                window.toFront()
                window.requestFocus()
            }
            previous
        }
        Thread.sleep(100)
        return try {
            block()
        } finally {
            onEventThread { window.setAlwaysOnTop(previousAlwaysOnTop) }
        }
    }

    private fun pressKey(name: String, modifiers: List<String>) {
        dispatchKeyStroke(keyCode(name), modifiers)
    }

    private fun dispatchMouseClick(x: Int, y: Int, secondary: Boolean, modifiers: List<String>) {
        val point = onEventThread {
            mouseTarget(x, y)
            window.locationOnScreen.let { Point(it.x + x, it.y + y) }
        }
        val buttonMask = if (secondary) InputEvent.BUTTON3_DOWN_MASK else InputEvent.BUTTON1_DOWN_MASK
        withRobotModifiers(modifiers) {
            robot.mouseMove(point.x, point.y)
            robot.mousePress(buttonMask)
            robot.delay(25)
            robot.mouseRelease(buttonMask)
        }
    }

    private fun dispatchMouseDrag(fromX: Int, fromY: Int, toX: Int, toY: Int, modifiers: List<String>) {
        val origin = onEventThread {
            mouseTarget(fromX, fromY)
            window.locationOnScreen
        }
        withRobotModifiers(modifiers) {
            robot.mouseMove(origin.x + fromX, origin.y + fromY)
            robot.mousePress(InputEvent.BUTTON1_DOWN_MASK)
            repeat(20) { step ->
                val ratio = (step + 1) / 20.0
                val x = (fromX + (toX - fromX) * ratio).toInt()
                val y = (fromY + (toY - fromY) * ratio).toInt()
                robot.mouseMove(origin.x + x, origin.y + y)
                robot.delay(8)
            }
            robot.mouseRelease(InputEvent.BUTTON1_DOWN_MASK)
        }
    }

    private fun dispatchMouseWheel(x: Int, y: Int, delta: Int, modifiers: List<String>) {
        val point = onEventThread {
            mouseTarget(x, y)
            window.locationOnScreen.let { Point(it.x + x, it.y + y) }
        }
        withRobotModifiers(modifiers) {
            robot.mouseMove(point.x, point.y)
            robot.mouseWheel(delta)
        }
    }

    private fun withRobotModifiers(modifiers: List<String>, action: () -> Unit) {
        val keys = modifiers.mapNotNull { modifier ->
            when (modifier.uppercase()) {
                "CTRL", "CONTROL" -> KeyEvent.VK_CONTROL
                "SHIFT" -> KeyEvent.VK_SHIFT
                "ALT" -> KeyEvent.VK_ALT
                else -> null
            }
        }.distinct()
        keys.forEach(robot::keyPress)
        try {
            action()
        } finally {
            keys.asReversed().forEach(robot::keyRelease)
        }
    }

    private fun mouseTarget(x: Int, y: Int): Component {
        val deepest = SwingUtilities.getDeepestComponentAt(window, x, y) ?: window
        return (generateSequence(deepest) { it.parent }
            .firstOrNull { it.mouseListeners.isNotEmpty() || it.mouseMotionListeners.isNotEmpty() }
            ?: deepest).also(UiTestState::updateInputTarget)
    }

    private fun mouseModifierMask(modifiers: List<String>): Int = modifiers.fold(0) { mask, modifier ->
        mask or when (modifier.uppercase()) {
            "CTRL", "CONTROL" -> InputEvent.CTRL_DOWN_MASK
            "SHIFT" -> InputEvent.SHIFT_DOWN_MASK
            "ALT" -> InputEvent.ALT_DOWN_MASK
            else -> 0
        }
    }

    private fun typeText(value: String) {
        onEventThread {
            val target = KeyboardFocusManager.getCurrentKeyboardFocusManager().focusOwner
                ?: throw IllegalStateException("no focused input control")
            value.forEach { char ->
                target.dispatchEvent(KeyEvent(
                    target,
                    KeyEvent.KEY_TYPED,
                    System.currentTimeMillis(),
                    0,
                    KeyEvent.VK_UNDEFINED,
                    char,
                ))
            }
        }
    }

    private fun dispatchKeyStroke(code: Int, modifiers: List<String>) {
        onEventThread {
            val target = KeyboardFocusManager.getCurrentKeyboardFocusManager().focusOwner
                ?: throw IllegalStateException("no focused control")
            val modifierMask = modifiers.fold(0) { mask, modifier ->
                mask or when (modifier.uppercase()) {
                    "CTRL", "CONTROL" -> InputEvent.CTRL_DOWN_MASK
                    "SHIFT" -> InputEvent.SHIFT_DOWN_MASK
                    "ALT" -> InputEvent.ALT_DOWN_MASK
                    else -> 0
                }
            }
            val now = System.currentTimeMillis()
            target.dispatchEvent(KeyEvent(target, KeyEvent.KEY_PRESSED, now, modifierMask, code, KeyEvent.CHAR_UNDEFINED))
            target.dispatchEvent(KeyEvent(target, KeyEvent.KEY_RELEASED, now + 1, modifierMask, code, KeyEvent.CHAR_UNDEFINED))
        }
    }

    private fun pasteText(value: String) {
        val clipboard = Toolkit.getDefaultToolkit().systemClipboard
        val previous = runCatching { clipboard.getContents(null) }.getOrNull()
        clipboard.setContents(StringSelection(value), null)
        try {
            dispatchKeyStroke(KeyEvent.VK_V, listOf("CTRL"))
            robot.waitForIdle()
            Thread.sleep(250)
        } finally {
            if (previous != null) runCatching { clipboard.setContents(previous, null) }
        }
    }

    private fun keyCode(name: String): Int = when (name.trim().uppercase()) {
        "CTRL", "CONTROL" -> KeyEvent.VK_CONTROL
        "SHIFT" -> KeyEvent.VK_SHIFT
        "ALT" -> KeyEvent.VK_ALT
        "ENTER", "RETURN" -> KeyEvent.VK_ENTER
        "ESC", "ESCAPE" -> KeyEvent.VK_ESCAPE
        "TAB" -> KeyEvent.VK_TAB
        "SPACE" -> KeyEvent.VK_SPACE
        "DELETE" -> KeyEvent.VK_DELETE
        "BACKSPACE" -> KeyEvent.VK_BACK_SPACE
        "UP" -> KeyEvent.VK_UP
        "DOWN" -> KeyEvent.VK_DOWN
        "LEFT" -> KeyEvent.VK_LEFT
        "RIGHT" -> KeyEvent.VK_RIGHT
        "HOME" -> KeyEvent.VK_HOME
        "END" -> KeyEvent.VK_END
        "PAGEUP" -> KeyEvent.VK_PAGE_UP
        "PAGEDOWN" -> KeyEvent.VK_PAGE_DOWN
        "F1" -> KeyEvent.VK_F1
        "F5" -> KeyEvent.VK_F5
        "F11" -> KeyEvent.VK_F11
        else -> name.singleOrNull()?.let { KeyEvent.getExtendedKeyCodeForChar(it.code) }
            ?.takeUnless { it == KeyEvent.VK_UNDEFINED }
            ?: throw IllegalArgumentException("unsupported key")
    }

    private fun jsonResponse(exchange: HttpExchange, status: Int, body: String) =
        bytesResponse(exchange, status, "application/json; charset=utf-8", body.toByteArray(StandardCharsets.UTF_8))

    private fun pngResponse(exchange: HttpExchange, image: BufferedImage) {
        val output = ByteArrayOutputStream()
        check(ImageIO.write(image, "png", output)) { "PNG encoder unavailable" }
        bytesResponse(exchange, 200, "image/png", output.toByteArray())
    }

    private fun bytesResponse(exchange: HttpExchange, status: Int, contentType: String, body: ByteArray) {
        exchange.responseHeaders.set("Content-Type", contentType)
        exchange.sendResponseHeaders(status, body.size.toLong())
        exchange.responseBody.use { it.write(body) }
    }

    companion object {
        fun startIfEnabled(window: Window): UiTestApi? {
            if (System.getenv("HLS_UI_TEST_API") != "1") return null
            val token = System.getenv("HLS_UI_TEST_TOKEN").orEmpty()
            require(token.length >= 16) { "HLS_UI_TEST_TOKEN must contain at least 16 characters" }
            val requestedPort = System.getenv("HLS_UI_TEST_PORT")?.toIntOrNull() ?: 19739
            require(requestedPort == 0 || requestedPort in 1024..65535) { "HLS_UI_TEST_PORT is invalid" }
            val server = HttpServer.create(InetSocketAddress(InetAddress.getLoopbackAddress(), requestedPort), 8)
            val executor = Executors.newSingleThreadExecutor { runnable ->
                Thread(runnable, "hls-ui-test-api").apply { isDaemon = true }
            }
            val previousAlwaysOnTop = onEventThread {
                val previous = window.isAlwaysOnTop
                window.toFront()
                window.requestFocus()
                previous
            }
            val api = UiTestApi(server, executor, window, token, Robot(), previousAlwaysOnTop)
            server.executor = executor
            api.installRoutes()
            server.start()
            System.getenv("HLS_UI_TEST_PORT_FILE")?.takeIf(String::isNotBlank)?.let { file ->
                val path = Path.of(file).toAbsolutePath().normalize()
                path.parent?.let(Files::createDirectories)
                Files.writeString(path, api.port.toString(), StandardCharsets.UTF_8)
            }
            println("UI_TEST_API=http://127.0.0.1:${api.port}")
            return api
        }
    }
}

internal data class RobotKey(val code: Int, val shift: Boolean = false)

internal fun robotKeyForChar(char: Char): RobotKey? = when {
    char in 'a'..'z' -> RobotKey(KeyEvent.getExtendedKeyCodeForChar(char.code))
    char in 'A'..'Z' -> RobotKey(KeyEvent.getExtendedKeyCodeForChar(char.code), true)
    char in '0'..'9' -> RobotKey(KeyEvent.getExtendedKeyCodeForChar(char.code))
    else -> when (char) {
        ' ' -> RobotKey(KeyEvent.VK_SPACE)
        '\n' -> RobotKey(KeyEvent.VK_ENTER)
        '\t' -> RobotKey(KeyEvent.VK_TAB)
        '.' -> RobotKey(KeyEvent.VK_PERIOD)
        ',' -> RobotKey(KeyEvent.VK_COMMA)
        '/' -> RobotKey(KeyEvent.VK_SLASH)
        '\\' -> RobotKey(KeyEvent.VK_BACK_SLASH)
        '-' -> RobotKey(KeyEvent.VK_MINUS)
        '_' -> RobotKey(KeyEvent.VK_MINUS, true)
        '=' -> RobotKey(KeyEvent.VK_EQUALS)
        '+' -> RobotKey(KeyEvent.VK_EQUALS, true)
        ':' -> RobotKey(KeyEvent.VK_SEMICOLON, true)
        ';' -> RobotKey(KeyEvent.VK_SEMICOLON)
        '?' -> RobotKey(KeyEvent.VK_SLASH, true)
        '&' -> RobotKey(KeyEvent.VK_7, true)
        '%' -> RobotKey(KeyEvent.VK_5, true)
        '#' -> RobotKey(KeyEvent.VK_3, true)
        '@' -> RobotKey(KeyEvent.VK_2, true)
        '!' -> RobotKey(KeyEvent.VK_1, true)
        '$' -> RobotKey(KeyEvent.VK_4, true)
        '^' -> RobotKey(KeyEvent.VK_6, true)
        '*' -> RobotKey(KeyEvent.VK_8, true)
        '(' -> RobotKey(KeyEvent.VK_9, true)
        ')' -> RobotKey(KeyEvent.VK_0, true)
        '[' -> RobotKey(KeyEvent.VK_OPEN_BRACKET)
        ']' -> RobotKey(KeyEvent.VK_CLOSE_BRACKET)
        '{' -> RobotKey(KeyEvent.VK_OPEN_BRACKET, true)
        '}' -> RobotKey(KeyEvent.VK_CLOSE_BRACKET, true)
        '\'' -> RobotKey(KeyEvent.VK_QUOTE)
        '"' -> RobotKey(KeyEvent.VK_QUOTE, true)
        '<' -> RobotKey(KeyEvent.VK_COMMA, true)
        '>' -> RobotKey(KeyEvent.VK_PERIOD, true)
        '|' -> RobotKey(KeyEvent.VK_BACK_SLASH, true)
        '`' -> RobotKey(KeyEvent.VK_BACK_QUOTE)
        '~' -> RobotKey(KeyEvent.VK_BACK_QUOTE, true)
        else -> null
    }
}

internal fun belongsToWindow(candidate: Window?, root: Window): Boolean {
    var current = candidate
    while (current != null) {
        if (current === root) return true
        current = current.owner
    }
    return false
}

@Serializable
private data class WindowSnapshot(
    val x: Int,
    val y: Int,
    val width: Int,
    val height: Int,
    val active: Boolean,
    val showing: Boolean,
    val iconCount: Int,
    val iconWidth: Int,
    val iconHeight: Int,
)

private fun escapeJson(value: String): String = buildString(value.length + 8) {
    value.forEach { char ->
        when (char) {
            '\\' -> append("\\\\")
            '"' -> append("\\\"")
            '\n' -> append("\\n")
            '\r' -> append("\\r")
            '\t' -> append("\\t")
            else -> if (char.code < 0x20) append("\\u%04x".format(char.code)) else append(char)
        }
    }
}

private fun <T> onEventThread(block: () -> T): T {
    if (EventQueue.isDispatchThread()) return block()
    var value: Result<T>? = null
    EventQueue.invokeAndWait { value = runCatching(block) }
    return value!!.getOrThrow()
}
