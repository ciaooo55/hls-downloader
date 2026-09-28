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
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.Composable
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInWindow

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
    /** 供 `open_task_details` 按 id 打开一个**不在任务列表里**的任务（详情弹窗的按 id 兜底分支）。 */
    @SerialName("task_id") val taskId: String? = null,
    val key: String? = null,
    val modifiers: List<String> = emptyList(),
    /** 上下文菜单动作名（retry/delete/...），配 context_menu_action 用。 */
    @SerialName("context_action") val contextAction: String? = null,
    val text: String? = null,
    val delta: Int? = null,
)

internal fun validateUiTestAction(action: UiTestAction, width: Int, height: Int): String? = when (action.type) {
    "activate" -> when {
        action.x != null || action.y != null -> when {
            action.x == null || action.y == null -> "activate with coordinates requires both x and y"
            action.x !in 0 until width || action.y !in 0 until height -> "coordinates are outside the current window"
            else -> null
        }
        else -> null
    }
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
    "move" -> when {
        action.x == null || action.y == null -> "move action requires x and y"
        action.x !in 0 until width || action.y !in 0 until height -> "coordinates are outside the current window"
        else -> null
    }
    "press" -> when {
        action.x == null || action.y == null -> "press actions require x and y"
        action.x !in 0 until width || action.y !in 0 until height -> "coordinates are outside the current window"
        else -> null
    }
    "release" -> null
    "scroll" -> when {
        action.x == null || action.y == null || action.delta == null || action.delta == 0 -> "scroll action requires x, y and a non-zero delta"
        action.x !in 0 until width || action.y !in 0 until height -> "coordinates are outside the current window"
        action.delta !in -20..20 -> "scroll delta must be between -20 and 20"
        else -> null
    }
    "select_task" -> if (action.index == null || action.index < 0) "select_task requires a non-negative index" else null
    "context_menu_action" -> when {
        action.contextAction == null -> "context_menu_action requires a contextAction"
        action.contextAction.isBlank() -> "context_menu_action requires a non-blank contextAction"
        action.taskId == null && (action.index == null || action.index < 0) -> "context_menu_action requires a non-negative index or a taskId"
        else -> null
    }
    "open_task_details" -> when {
        action.taskId == null && (action.index == null || action.index < 0) ->
            "open_task_details requires a non-negative index or a taskId"
        action.x == null || action.y == null -> "open_task_details requires x and y"
        action.x !in 0 until width || action.y !in 0 until height -> "coordinates are outside the current window"
        else -> null
    }
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
    /**
     * 直接触发"上下文菜单某个动作"的钩子（不靠坐标）。
     *
     * 为什么需要它：第一百二十一到第一百三十三轮，我想验证断线时批量操作的人话提示，
     * 但菜单项只能靠坐标点，而多选菜单的项位量不出来（模型也没有读图能力），
     * 三个候选偏移全试过都没命中。釜底抽薪的办法是让测试 API 一条调用就走到
     * onBatchAction(targets, action)，也就是菜单项 onClick 真正调的同一个函数。
     */
    @Volatile private var taskContextActionInvoker: ((Int?, String) -> Unit)? = null

    fun installTaskContextActionInvoker(invoker: ((Int?, String) -> Unit)?) {
        taskContextActionInvoker = invoker
    }

    fun invokeTaskContextAction(index: Int?, action: String?) {
        taskContextActionInvoker?.invoke(index, action ?: return)
    }

    @Volatile private var taskDetailsOpener: ((Int?, String?, Int, Int) -> Unit)? = null
    @Volatile private var contextMenuPosition: List<Int> = emptyList()
    @Volatile private var contextMenuTaskIds: List<String> = emptyList()
    @Volatile private var contextMenuActions: List<String> = emptyList()

    /**
     * 当前生效的侧栏筛选（[TaskFilter.label]），折叠栏与展开态共用同一个状态。
     *
     * 为什么需要它：折叠栏把文字标签换成了图标，**"点得中、点得对"就不再是肉眼能确认的事**——
     * 40dp 的点击区、11 个入口、还有底部固定的"管理队列"，光看截图只能证明它长得对。
     * 有了这个字段，`/state` 就能直接断言"点了第 4 个图标 ⇒ 筛选变成 失败"，
     * 而不是靠"选中数从 3 变 0"这种间接推断。
     */
    @Volatile private var activeFilter: String = ""

    /** 当前选中的分类标签（未选为空串）。与 [activeFilter] 同理，让折叠栏的分类组也可断言。 */
    @Volatile private var activeCategory: String = ""

    /** 当前选中的队列 id（未选为空串）。 */
    @Volatile private var activeQueueId: String = ""

    /**
     * 引擎连接状态文案（连接中/已连接/重连中）。
     *
     * 为什么需要它：此前判断"UI 到底连没连上引擎"只能靠 controlBounds 里有没有
     * taskrow 控件这种间接推断，而那条依据本身不可靠（bounds 只注册不注销）。
     * 有了这个字段，/state 就能直接断言"UI 显示的是已连接"，
     * 传输层改动能被正面验证而不是靠猜。
     */
    @Volatile private var engineText: String = ""

    /**
     * 最近的通知（弹条/通知中心）历史，条目是 level 与 message。
     *
     * 为什么需要它：第一百二十一轮把断线时的任务操作失败换成了人话提示，
     * 但 notice/snackbar 此前没有导出口，那句文案在本机无法被正面断言。
     * 有了这个字段，就能断言"这条消息确实出现、就是这个字符串"。
     */
    @Volatile private var notices: List<Triple<String, String, Long>> = emptyList()

    /**
     * 已命名控件的窗口内边框（左、上、右、下，单位像素）。
     *
     * 为什么需要它：「窄控件 / 对话框内部控件的按压反馈」这类测量，此前靠**从截图的
     * 分层图里估坐标**。那有三个后果：窗口尺寸一变、对话框内容一变，估出来的坐标全部作废；
     * 定位失败的报错只是「差分 0 像素」，看不出是没修好还是根本没测到那个东西；
     * 而且复现一次要重跑整套启动 + 找坐标。
     * 让控件自己用 `onGloballyPositioned` 上报边框，坐标就从「估」变成「读」——
     * `/state` 直接给出中心点，`/action` 的 `move`/`press` 就能精确落点。
     *
     * 边框注册 + 离开组合即注销：`controlBounds` 只反映当前在屏上的控件。
     * 调用方应看 key 在不在，而不是看宽高是否为 0——后者曾经带我得出过
     * 「对话框已打开」的错误结论（把关掉之前残留的 `dialog.secondary.取消` 当成了证据）。
     */
    @Volatile private var controlBounds: Map<String, List<Int>> = emptyMap()

    fun reportControlBounds(name: String, left: Int, top: Int, right: Int, bottom: Int) {
        if (name.isBlank()) return
        val next = controlBounds.toMutableMap()
        next[name] = listOf(left, top, right, bottom)
        controlBounds = next
    }

    /**
     * 控件离开组合（对话框关闭、列表滚出窗口）时撤掉它的边框记录。
     *
     * 只注册不注销时，条目会带着最后一帧的非零边框一直留着，于是"这个控件此刻
     * 在不在屏上"根本没法从 `controlBounds` 判断。
     */
    fun clearControlBounds(name: String) {
        if (name.isBlank() || !controlBounds.containsKey(name)) return
        val next = controlBounds.toMutableMap()
        next.remove(name)
        controlBounds = next
    }

    fun updateFilter(label: String) {
        activeFilter = label
    }

    fun updateSidebarSelection(categoryLabel: String, queueId: String) {
        activeCategory = categoryLabel
        activeQueueId = queueId
    }

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

    /**
     * 按行索引导出任务详情弹窗（与 [taskContextOpener] 同一套安装方式）。
     *
     * 为什么需要它：详情弹窗在本机此前**完全打不开**——可见夹具没有一个是预开
     * 它的（`visualFixture` 预开的弹窗里没有一个设置 `detailTaskId`），
     * 而行双击 / 行内菜单"详情与日志"这两个真实入口在夹具里又都点不中。
     * 于是"按 id 单独取单个任务"那类改动拿不到任何运行时证据。
     * 有了这个钩子，`/action` 就能像 `open_task_menu` 一样直接把弹窗拉起来。
     * 可传 `task_id` 打开一个**不在列表里**的任务——详情弹窗按 id 单独取兜底那条分支，
     * 只有这样才能触达（按 index 打开时目标任务必然在列表里）。
     */
    fun installTaskDetailsOpener(opener: ((Int?, String?, Int, Int) -> Unit)?) {
        taskDetailsOpener = opener
    }

    fun openTaskDetails(index: Int?, taskId: String?, x: Int, y: Int) {
        taskDetailsOpener?.invoke(index, taskId, x, y)
    }

    fun openTaskMenu(index: Int, x: Int, y: Int) {
        taskContextOpener?.invoke(index, x, y)
            ?: throw IllegalStateException("task table is not available")
    }

    fun updateEngineText(text: String) {
        engineText = text
    }

    fun updateNotices(entries: List<Triple<String, String, Long>>) {
        notices = entries
    }

    fun updateContextMenu(position: androidx.compose.ui.unit.IntOffset, taskIds: Set<String>, actions: List<String>) {
        contextMenuPosition = listOf(position.x, position.y)
        contextMenuTaskIds = taskIds.sorted()
        contextMenuActions = actions
    }

    /**
     * 菜单真正关闭时调用。没有它，`/state` 会一直报告上一次打开的菜单：
     * 等待 `contextMenuActions` 变空的校验脚本会空转到超时，而人读 JSON
     * 会看到一个屏幕上已经不存在的菜单——正是会静默污染证据的那类缺陷。
     */
    fun closeContextMenu() {
        contextMenuPosition = emptyList()
        contextMenuTaskIds = emptyList()
        contextMenuActions = emptyList()
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
        activeFilter,
        activeCategory,
        activeQueueId,
        controlBounds,
        engineText,
        notices.map { (level, message, at) -> listOf(level, message, at.toString()) },
    )
}

/**
 * 把一个控件在窗口里的边框报给 [UiTestState]，供 `/state` 的 `controlBounds` 读取。
 *
 * 用法：给需要被精确定位的控件加上 `Modifier.reportControlBounds("name")`。
 * 只在测试 API 开着时才有意义，但**不加门**——上报本身只是往一个 Map 里写边界，
 * 编译期与运行期开销都可忽略；真正的门在 `UiTestApi` 只在 `HLS_UI_TEST_API=1` 时启动。
 */
@Composable
internal fun Modifier.reportControlBounds(name: String): Modifier {
    // 离开组合就注销，`controlBounds` 才等于"此刻屏上真实存在的控件"。
    DisposableEffect(name) {
        onDispose { runCatching { UiTestState.clearControlBounds(name) } }
    }
    val bounds = remember { mutableStateOf<IntArray?>(null) }
    return this.then(
        Modifier.onGloballyPositioned { coordinates ->
            val origin = coordinates.positionInWindow()
            val size = coordinates.size
            bounds.value = intArrayOf(origin.x.toInt(), origin.y.toInt(), (origin.x + size.width).toInt(), (origin.y + size.height).toInt())
            runCatching {
                UiTestState.reportControlBounds(
                    name,
                    origin.x.toInt(),
                    origin.y.toInt(),
                    (origin.x + size.width).toInt(),
                    (origin.y + size.height).toInt(),
                )
            }
        }
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
    // 末尾新增字段，带默认值 ⇒ 旧脚本按原字段读不受影响（JSON 是增量扩展）。
    val activeFilter: String = "",
    val activeCategory: String = "",
    val activeQueueId: String = "",
    /** 已命名控件的窗口内边框 [left, top, right, bottom]；键就是调用方给的名字。 */
    val controlBounds: Map<String, List<Int>> = emptyMap(),
    /** 引擎连接状态文案；空字符串表示当前夹具没有这方面的信息。 */
    val engineText: String = "",
    /** 最近通知，每项是 [level, message, at]（at 是 System.currentTimeMillis）；最多保留末尾若干条。 */
    val noticeHistory: List<List<String>> = emptyList(),
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

    /**
     * 当前仍被按住的鼠标键掩码；0 表示没有键被按住。
     *
     * 为什么需要它：按下反馈（hover 底色 / press 缩放）**只活在按下期间**，
     * 而 `click` 一路 press→release，截图永远采不到那一帧。
     * `press` 按住不放、`release` 才抬，两者之间的 `/screenshot`
     * 才能把"按下时到底渲染成什么样"变成可断言的像素。
     */
    @Volatile
    private var heldButtonMask: Int = 0

    /** 任何后续鼠标动作前先把可能残留的按下抬掉，避免上一次 `press` 把指针黏住。 */
    private fun releaseHeldButton() {
        val mask = heldButtonMask
        if (mask != 0) {
            heldButtonMask = 0
            runCatching { robot.mouseRelease(mask) }
        }
    }

    override fun close() {
        server.stop(0)
        executor.shutdownNow()
        runCatching { releaseHeldButton() }
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
                "activate" -> if (action.x != null && action.y != null) dispatchMouseClick(action.x, action.y, false, action.modifiers) else Unit
                "click", "right_click" -> {
                    dispatchMouseClick(action.x!!, action.y!!, action.type == "right_click", action.modifiers)
                }
                "press" -> dispatchMousePress(action.x!!, action.y!!, action.modifiers)
                "release" -> releaseHeldButton()
                "drag" -> {
                    dispatchMouseDrag(action.x!!, action.y!!, action.toX!!, action.toY!!, action.modifiers)
                }
                "move" -> dispatchMouseMove(action.x!!, action.y!!)
                "scroll" -> dispatchMouseWheel(action.x!!, action.y!!, action.delta!!, action.modifiers)
                "select_task" -> onEventThread { UiTestState.selectTask(action.index!!, action.modifiers) }
                "open_task_menu" -> onEventThread { UiTestState.openTaskMenu(action.index!!, action.x!!, action.y!!) }
                "context_menu_action" -> onEventThread { UiTestState.invokeTaskContextAction(action.index, action.contextAction) }
                "open_task_details" -> onEventThread { UiTestState.openTaskDetails(action.index, action.taskId, action.x!!, action.y!!) }
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
        releaseHeldButton()
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

    /**
     * 按下**不抬**：为的是把"按下反馈"那一帧留住。
     *
     * `click` 是 press→(25ms)→release， Compose 的 press 态在 release 前就结束了，
     * 截图只能拿到 hover 态。所以这里让按钮**停在按下状态**，调用方先截图、
     * 再发 `press` 的后续动作或 `release` 把键抬掉——[releaseHeldButton] 保证
     * 任何鼠标动作（含下一次 `click`）都会自动收尾，指针不会被黏住。
     */
    private fun dispatchMousePress(x: Int, y: Int, modifiers: List<String>) {
        releaseHeldButton()
        val point = onEventThread {
            mouseTarget(x, y)
            window.locationOnScreen.let { Point(it.x + x, it.y + y) }
        }
        withRobotModifiers(modifiers) {
            robot.mouseMove(point.x, point.y)
            robot.mousePress(InputEvent.BUTTON1_DOWN_MASK)
        }
        heldButtonMask = InputEvent.BUTTON1_DOWN_MASK
        robot.delay(90)
    }

    /**
     * 只把指针移过去、不按键——用来触发 hover 态（tooltip、`:hover` 样式）。
     *
     * 悬停是能改变布局与配色的状态，本项目在 hover 规则上栽过（淡底家族那条最差项就是 `:hover`），
     * 所以它必须能被夹具测到，而不是只能靠人手去悬停。
     */
    private fun dispatchMouseMove(x: Int, y: Int) {
        val point = onEventThread {
            mouseTarget(x, y)
            window.locationOnScreen.let { Point(it.x + x, it.y + y) }
        }
        robot.mouseMove(point.x, point.y)
    }

    private fun dispatchMouseDrag(fromX: Int, fromY: Int, toX: Int, toY: Int, modifiers: List<String>) {
        releaseHeldButton()
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
        releaseHeldButton()
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
