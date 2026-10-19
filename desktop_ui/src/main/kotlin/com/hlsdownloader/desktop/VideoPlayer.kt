package com.hlsdownloader.desktop

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.focusable
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import com.hlsdownloader.desktop.icons.Close
import com.hlsdownloader.desktop.icons.Forward10
import com.hlsdownloader.desktop.icons.Pause
import com.hlsdownloader.desktop.icons.PlayArrow
import com.hlsdownloader.desktop.icons.Replay10
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.FocusDirection
import androidx.compose.ui.focus.focusProperties
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.input.key.*
import androidx.compose.ui.input.pointer.*
import androidx.compose.ui.layout.boundsInWindow
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.*
import androidx.compose.ui.window.*
import java.awt.Dimension
import java.awt.KeyboardFocusManager
import java.awt.Graphics
import java.awt.Graphics2D
import java.awt.RenderingHints
import java.awt.image.BufferedImage
import java.awt.event.ComponentAdapter
import java.awt.event.ComponentEvent
import java.awt.event.WindowAdapter
import java.awt.event.WindowEvent
import java.awt.geom.RoundRectangle2D
import java.io.ByteArrayInputStream
import java.util.Base64
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong
import kotlinx.coroutines.*
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import javax.imageio.ImageIO
import javax.swing.JPanel
import javax.swing.JWindow
import kotlin.math.roundToInt

private val playerBackground = Color(0xff101318)
private val playerSurface = Color(0xff191e26)
private val playerInk = Color(0xffeef2f7)
private val playerMuted = Color(0xffa9b3c2)
private val playerAccent = Color(0xff83adff)

// 播放、宿主移动与关闭共用顺序；旧浮层异步退出不能停止刚打开的新会话。
private object PlayerSessionAccess {
    val mutex = Mutex()
    val generation = AtomicLong()
}

private data class VideoArea(val main: Boolean, val x: Int, val y: Int, val width: Int, val height: Int)

internal fun playerTime(seconds: Double): String {
    val total = seconds.takeIf { it.isFinite() && it > 0 }?.toLong() ?: 0
    return if (total >= 3600) "%d:%02d:%02d".format(total / 3600, total / 60 % 60, total % 60)
    else "%d:%02d".format(total / 60, total % 60)
}

@Composable
internal fun LocalVideoPlayer(taskId: String, task: TaskDto?, onClose: () -> Unit, onFullscreen: (Boolean) -> Unit, onModalChanged: (Boolean) -> Unit, openRequest: Int) {
    val scope = rememberCoroutineScope()
    val epoch = remember { PlayerSessionAccess.generation.incrementAndGet() }
    val closed = remember { AtomicBoolean(false) }
    val initialized = remember { AtomicBoolean(false) }
    val latestFullscreen by rememberUpdatedState(onFullscreen)
    var snapshot by remember { mutableStateOf(PlayerSnapshot()) }
    var area by remember { mutableStateOf<VideoArea?>(null) }
    var started by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var retry by remember { mutableIntStateOf(0) }
    var pip by remember { mutableStateOf(false) }
    var fullscreen by remember { mutableStateOf(false) }
    var closing by remember { mutableStateOf(false) }
    val pipState = rememberWindowState(width = 520.dp, height = 360.dp, position = WindowPosition(Alignment.BottomEnd))

    suspend fun request(action: String): PlayerSnapshot? = withContext(Dispatchers.IO) {
        PlayerSessionAccess.mutex.withLock {
            if (epoch != PlayerSessionAccess.generation.get() || closed.get()) null
            else EnginePipeClient.playerControl(action).playerSnapshot()
        }
    }
    fun control(action: String) { if (closing) return; scope.launch {
        try { request(action)?.let { snapshot = it }; error = null }
        catch (cancelled: CancellationException) { throw cancelled }
        catch (failure: Exception) { error = failure.message ?: "播放器操作失败" }
    } }
    fun close() {
        if (closing) return
        closing = true
        scope.launch {
            try {
                request("stop")
                closed.set(true)
                latestFullscreen(false)
                onClose()
            } catch (cancelled: CancellationException) { throw cancelled }
            catch (failure: Exception) { error = failure.message ?: "关闭失败，请重试"; closing = false }
        }
    }
    fun togglePip() { if (closing) return; scope.launch {
        try {
            request("hide")
            fullscreen = false
            latestFullscreen(false)
            area = null
            pip = !pip
        } catch (cancelled: CancellationException) { throw cancelled }
        catch (failure: Exception) { error = failure.message ?: "画中画切换失败" }
    } }

    DisposableEffect(Unit) { onDispose {
        latestFullscreen(false)
        onModalChanged(false)
        if (!closed.get()) CoroutineScope(Dispatchers.IO).launch {
            PlayerSessionAccess.mutex.withLock {
                if (epoch == PlayerSessionAccess.generation.get()) {
                    runCatching { EnginePipeClient.playerControl("stop") }
                }
            }
        }
    } }
    LaunchedEffect(pip, fullscreen) {
        onModalChanged(!pip)
        latestFullscreen(fullscreen && !pip)
        pipState.placement = if (pip && fullscreen) WindowPlacement.Fullscreen else WindowPlacement.Floating
    }
    LaunchedEffect(openRequest) {
        if (pip) {
            try {
                request("hide")
                area = null
                fullscreen = false
                pip = false
            } catch (cancelled: CancellationException) { throw cancelled }
            catch (failure: Exception) { onModalChanged(false); error = failure.message ?: "返回播放器失败" }
        }
    }
    LaunchedEffect(area, retry) {
        val target = area ?: return@LaunchedEffect
        delay(40)
        try {
            val result = withContext(Dispatchers.IO) { PlayerSessionAccess.mutex.withLock {
                if (epoch != PlayerSessionAccess.generation.get() || closed.get()) return@withLock null
                val host = if (target.main) "embed_main" else "embed_host"
                EnginePipeClient.playerControl("$host:${target.x},${target.y},${target.width},${target.height}")
                if (!initialized.get()) {
                    val first = EnginePipeClient.playTask(taskId).playerSnapshot()
                        ?: throw IllegalStateException("未收到播放器状态")
                    initialized.set(true)
                    first
                } else EnginePipeClient.playerControl("status").playerSnapshot()
            } }
            if (result != null) { snapshot = result; started = true; error = null }
        } catch (cancelled: CancellationException) { throw cancelled }
        catch (failure: Exception) { error = failure.message ?: "视频区域初始化失败" }
    }
    LaunchedEffect(started) {
        if (!started) return@LaunchedEffect
        while (true) {
            delay(400)
            try {
                val next = request("status") ?: continue
                if (!next.active || next.taskId != taskId) {
                    closed.set(true); latestFullscreen(false); onClose(); return@LaunchedEffect
                }
                snapshot = next
            } catch (cancelled: CancellationException) { throw cancelled }
            catch (failure: Exception) { error = failure.message ?: "播放状态连接中断" }
        }
    }

    val title = task?.let { it.title.ifBlank { it.filename } }?.takeIf { it.isNotBlank() }
        ?: snapshot.title.ifBlank { "视频" }
    val speed = task?.takeIf { it.status in setOf("running", "downloading", "recording") }?.let {
        val value = it.speedBytesPerSecond / 1024.0
        if (value >= 1024) "%.1f MiB/s".format(value / 1024) else "%.0f KiB/s".format(value)
    }
    val panel: @Composable (((Offset) -> Unit)?) -> Unit = { onDrag ->
        PlaybackPanel(taskId, title, snapshot, speed, pip, fullscreen, started, closing, error,
            // 两个宿主独立排版；忽略已隐藏宿主迟到的布局回调。
            onArea = { if (it.main != pip && !closing) area = it }, onAction = ::control, onClose = ::close,
            onPip = ::togglePip, onFullscreen = { fullscreen = !fullscreen }, onRetry = { retry++ }, onDrag = onDrag)
    }
    // 小窗隐藏后仍保留宿主，直到视频移回主窗口；销毁父 HWND 会连带销毁视频。
    Window(onCloseRequest = ::close, title = "HLS Downloader 播放器", state = pipState, visible = pip,
            undecorated = true, alwaysOnTop = true, resizable = true) {
            val density = LocalDensity.current.density
            DisposableEffect(window, pip) {
                val testApi = if (pip) UiTestApi.startIfEnabled(window, "HLS_UI_TEST_PIP_PORT") else null
                onDispose { testApi?.close() }
            }
            DisposableEffect(window, fullscreen) {
                window.minimumSize = Dimension(360, 280)
                val listener = object : ComponentAdapter() {
                    override fun componentResized(event: ComponentEvent?) {
                        runCatching { window.shape = if (fullscreen) null else RoundRectangle2D.Double(0.0, 0.0, window.width.toDouble(), window.height.toDouble(), 20.0, 20.0) }
                    }
                }
                window.addComponentListener(listener)
                listener.componentResized(null)
                onDispose { window.removeComponentListener(listener) }
            }
            if (pip) panel { delta -> window.setLocation(window.x + (delta.x / density).roundToInt(), window.y + (delta.y / density).roundToInt()) }
    }
    if (!pip) {
        Box(Modifier.fillMaxSize().background(Color.Black.copy(alpha = .72f)).clickable(enabled = !closing) {}, contentAlignment = Alignment.Center) {
            Box(if (fullscreen) Modifier.fillMaxSize() else Modifier.padding(20.dp).widthIn(max = 960.dp)
                .fillMaxWidth().fillMaxHeight(.82f)) { panel(null) }
        }
    }
}

@Composable
private fun PlaybackPanel(
    taskId: String, title: String, state: PlayerSnapshot, downloadSpeed: String?, pip: Boolean,
    fullscreen: Boolean, started: Boolean, closing: Boolean, error: String?,
    onArea: (VideoArea) -> Unit, onAction: (String) -> Unit, onClose: () -> Unit,
    onPip: () -> Unit, onFullscreen: () -> Unit, onRetry: () -> Unit, onDrag: ((Offset) -> Unit)?,
) {
    val focus = remember { FocusRequester() }
    val density = LocalDensity.current
    var scrub by remember { mutableStateOf<Float?>(null) }
    var volume by remember { mutableStateOf<Float?>(null) }
    var hover by remember { mutableStateOf<Double?>(null) }
    var trackWidth by remember { mutableIntStateOf(1) }
    var thumbnail by remember { mutableStateOf<BufferedImage?>(null) }
    var trackOrigin by remember { mutableStateOf(Offset.Zero) }
    var thumbnailMessage by remember { mutableStateOf("正在预览…") }
    val cache = remember(taskId) { LinkedHashMap<Int, BufferedImage>(48, .75f, true) }
    val bucket = hover?.let { (it / 10).toInt() * 10 }
    LaunchedEffect(Unit) { focus.requestFocus() }
    LaunchedEffect(bucket) {
        thumbnail = null
        withContext(Dispatchers.IO) { runCatching { EnginePipeClient.playerThumbnail(taskId, null) } }
        val key = bucket ?: return@LaunchedEffect
        thumbnail = cache[key]
        if (thumbnail != null) return@LaunchedEffect
        thumbnailMessage = "正在预览…"
        delay(180)
        try {
            val image = withContext(Dispatchers.IO) {
                val encoded = EnginePipeClient.playerThumbnail(taskId, key.toDouble())
                ImageIO.read(ByteArrayInputStream(Base64.getDecoder().decode(encoded)))
                    ?: throw IllegalStateException("缩略图无法解码")
            }
            cache[key] = image
            while (cache.size > 48) cache.remove(cache.keys.first())
            thumbnail = image
        } catch (cancelled: CancellationException) { throw cancelled }
        catch (_: Exception) { thumbnailMessage = "此处暂不可预览" }
    }
    DisposableEffect(Unit) { onDispose {
        cache.clear()
        CoroutineScope(Dispatchers.IO).launch { runCatching { EnginePipeClient.playerThumbnail(taskId, null) } }
    } }
    Column(Modifier.fillMaxSize().clip(RoundedCornerShape(if (fullscreen) 0.dp else 12.dp))
        .background(playerBackground).border(1.dp, Color(0xff343e4c), RoundedCornerShape(if (fullscreen) 0.dp else 12.dp))
        .focusProperties { onExit = {
            if (!pip && (requestedFocusDirection == FocusDirection.Next || requestedFocusDirection == FocusDirection.Previous)) {
                focus.requestFocus()
            }
        } }.onKeyEvent {
            if (it.type != KeyEventType.KeyDown) false
            else when (it.key) {
                Key.Escape -> { if (fullscreen) onFullscreen() else onClose(); true }
                Key.Spacebar, Key.K -> if (started) { onAction(if (state.paused) "resume" else "pause"); true } else false
                Key.DirectionLeft -> if (started) { onAction("seek_back"); true } else false
                Key.DirectionRight -> if (started) { onAction("seek_fwd"); true } else false
                Key.F -> if (started) { onFullscreen(); true } else false
                Key.M -> if (started) { onAction("mute"); true } else false
                else -> false
            }
        }.focusRequester(focus).focusable()) {
        Row(Modifier.fillMaxWidth().height(48.dp).background(playerSurface).reportControlBounds("video.header")
            .then(if (onDrag != null) Modifier.pointerInput(onDrag) { detectDragGestures { change, amount -> change.consume(); onDrag(amount) } } else Modifier)
            .padding(start = 14.dp, end = 6.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(title, color = playerInk, fontSize = 13.sp, fontWeight = FontWeight.SemiBold, maxLines = 1,
                overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
            downloadSpeed?.let { Text(it, color = Color(0xff78d6ad), fontSize = 11.sp, modifier = Modifier.padding(horizontal = 8.dp)) }
            PlayerButton("关闭播放器，下载继续", enabled = !closing, icon = Icons.Outlined.Close, onClick = onClose)
        }
        Box(Modifier.weight(1f).fillMaxWidth().background(Color.Black).reportControlBounds("video.surface").clickable(enabled = started) {
            focus.requestFocus()
            onAction(if (state.paused) "resume" else "pause")
        }.onGloballyPositioned {
            val bounds = it.boundsInWindow()
            if (bounds.width >= 32 && bounds.height >= 32) onArea(VideoArea(!pip, bounds.left.roundToInt(), bounds.top.roundToInt(), bounds.width.roundToInt(), bounds.height.roundToInt()))
        }, contentAlignment = Alignment.Center) {
            // 原生视频只覆盖此矩形；所有 Compose 控件位于矩形之外，避免 HWND 遮住按钮。
            if (!started) Text(if (error == null) "正在准备播放…" else "视频暂未打开", color = playerMuted, fontSize = 13.sp)
        }
        Column(Modifier.fillMaxWidth().background(playerSurface).padding(horizontal = 10.dp, vertical = 5.dp)) {
            error?.let { Row(verticalAlignment = Alignment.CenterVertically) {
                Text(it, color = Color(0xffffb4ab), fontSize = 11.sp, maxLines = 2, modifier = Modifier.weight(1f))
                TextButton(onClick = onRetry) { Text("重试", color = playerAccent) }
            } }
            Text(if (state.buffering) "正在缓冲目标片段…" else "向前缓冲 ${playerTime((state.bufferedEnd - state.position).coerceAtLeast(0.0))}",
                color = playerMuted, fontSize = 11.sp)
            Box(Modifier.fillMaxWidth().height(28.dp).onSizeChanged { trackWidth = it.width }
                .onGloballyPositioned { trackOrigin = it.boundsInWindow().topLeft }
                .pointerInput(state.duration) { awaitPointerEventScope { while (true) {
                    val event = awaitPointerEvent()
                    hover = if (event.type == PointerEventType.Exit || state.duration <= 0) null
                        else event.changes.firstOrNull()?.position?.x?.let { (it / trackWidth).coerceIn(0f, 1f) * state.duration }
                } } }) {
                Canvas(Modifier.fillMaxSize()) {
                    val y = center.y - 2.dp.toPx()
                    drawRoundRect(Color(0xff343e4c), Offset(0f, y), Size(size.width, 4.dp.toPx()), CornerRadius(2.dp.toPx()))
                    if (state.duration > 0) {
                        val start = (state.position / state.duration).toFloat().coerceIn(0f, 1f) * size.width
                        val end = (state.bufferedEnd / state.duration).toFloat().coerceIn(0f, 1f) * size.width
                        drawRect(Color(0xff8794a8), Offset(start, y), Size((end - start).coerceAtLeast(0f), 4.dp.toPx()))
                    }
                }
                Slider(value = scrub ?: state.position.toFloat().coerceIn(0f, state.duration.toFloat().coerceAtLeast(.1f)),
                    onValueChange = { scrub = it }, onValueChangeFinished = { scrub?.let { onAction("seek_to:$it") }; scrub = null },
                    valueRange = 0f..state.duration.toFloat().coerceAtLeast(.1f), enabled = started && state.positionAvailable,
                    colors = SliderDefaults.colors(thumbColor = playerAccent, activeTrackColor = playerAccent, inactiveTrackColor = Color.Transparent),
                    modifier = Modifier.fillMaxSize().reportControlBounds("video.timeline").semantics { contentDescription = "播放进度；灰色为播放缓冲，蓝色为已播放" })
                hover?.let { time ->
                    val previewWidth = with(density) { 184.dp.roundToPx() }
                    val x = ((time / state.duration) * trackWidth - previewWidth / 2).roundToInt().coerceIn(0, (trackWidth - previewWidth).coerceAtLeast(0))
                    PlayerThumbnailPreview(thumbnail, thumbnailMessage, time,
                        trackOrigin + Offset(x.toFloat(), with(density) { (-136).dp.toPx() }), density.density) { hover = null }
                }
            }
            Row(Modifier.fillMaxWidth().height(36.dp), verticalAlignment = Alignment.CenterVertically) {
                PlayerButton(if (state.paused) "播放" else "暂停", enabled = started,
                    icon = if (state.paused) Icons.Outlined.PlayArrow else Icons.Outlined.Pause, emphasized = true) { onAction(if (state.paused) "resume" else "pause") }
                PlayerButton("后退 10 秒", enabled = started, icon = Icons.Outlined.Replay10) { onAction("seek_back") }
                PlayerButton("前进 10 秒", enabled = started, icon = Icons.Outlined.Forward10) { onAction("seek_fwd") }
                Text("${playerTime((scrub ?: state.position.toFloat()).toDouble())} / ${playerTime(state.duration)}", color = playerMuted, fontSize = 11.sp, modifier = Modifier.weight(1f).padding(start = 5.dp))
                PlayerRate(state.speed, started) { onAction("speed:$it") }
            }
            Row(Modifier.fillMaxWidth().height(36.dp), verticalAlignment = Alignment.CenterVertically) {
                PlayerButton(if (state.muted) "取消静音" else "静音", if (state.muted) "静音" else "音量", started) { onAction("mute") }
                Slider(value = volume ?: state.volume.toFloat().coerceIn(0f, 100f), onValueChange = { volume = it },
                    onValueChangeFinished = { volume?.let { onAction("volume:$it") }; volume = null }, valueRange = 0f..100f, enabled = started,
                    colors = SliderDefaults.colors(thumbColor = playerAccent, activeTrackColor = playerAccent),
                    modifier = Modifier.weight(1f).height(30.dp).reportControlBounds("video.volume").semantics { contentDescription = "音量" })
                PlayerTracks("音轨", state.tracks.filter { it.kind == "audio" }, false, started) { onAction("audio:$it") }
                PlayerTracks("字幕", state.tracks.filter { it.kind == "sub" }, true, started) { onAction("subtitle:$it") }
                PlayerButton(if (pip) "返回主界面播放器" else "画中画", if (pip) "返回" else "小窗", started, onClick = onPip)
                PlayerButton(if (fullscreen) "退出全屏" else "全屏", if (fullscreen) "还原" else "全屏", started, onClick = onFullscreen)
            }
        }
    }
}

@Composable
private fun PlayerThumbnailPreview(image: BufferedImage?, message: String, seconds: Double, origin: Offset, scale: Float, onDismiss: () -> Unit) {
    val owner = remember { KeyboardFocusManager.getCurrentKeyboardFocusManager().activeWindow } ?: return
    val picture by rememberUpdatedState(image)
    val text by rememberUpdatedState(message)
    val time by rememberUpdatedState(playerTime(seconds))
    val dismiss by rememberUpdatedState(onDismiss)
    // Compose Popup 与视频子 HWND 共用画布时会被视频遮挡；只让预览使用原窗口所属的原生 tooltip。
    val popup = remember(owner) { JWindow(owner).apply {
        focusableWindowState = false
        isAutoRequestFocus = false
        type = java.awt.Window.Type.POPUP
        setSize(184, 132)
        shape = RoundRectangle2D.Double(0.0, 0.0, 184.0, 132.0, 12.0, 12.0)
        contentPane = object : JPanel() {
            override fun paintComponent(graphics: Graphics) {
                val g = graphics.create() as Graphics2D
                try {
                    g.color = java.awt.Color(0x10, 0x13, 0x18)
                    g.fillRect(0, 0, width, height)
                    g.setRenderingHint(RenderingHints.KEY_INTERPOLATION, RenderingHints.VALUE_INTERPOLATION_BILINEAR)
                    val frame = picture
                    g.color = java.awt.Color(0xee, 0xf2, 0xf7)
                    g.font = font.deriveFont(11f)
                    if (frame != null) g.drawImage(frame, (184-frame.width)/2, (104-frame.height)/2, null)
                    else g.drawString(text, (184-g.fontMetrics.stringWidth(text))/2, 57)
                    g.drawString(time, (184-g.fontMetrics.stringWidth(time))/2, 122)
                    g.color = java.awt.Color(0x53, 0x61, 0x74)
                    g.drawRoundRect(0, 0, width-1, height-1, 12, 12)
                } finally { g.dispose() }
            }
        }
    } }
    DisposableEffect(popup, owner) {
        val listener = object : WindowAdapter() { override fun windowLostFocus(event: WindowEvent?) { dismiss() } }
        owner.addWindowFocusListener(listener)
        onDispose { owner.removeWindowFocusListener(listener); popup.dispose() }
    }
    SideEffect {
        if (!owner.isShowing) { popup.isVisible = false; return@SideEffect }
        val location = owner.locationOnScreen
        val screen = owner.graphicsConfiguration.bounds
        val x = (location.x + origin.x / scale).roundToInt().coerceIn(screen.x, (screen.x+screen.width-184).coerceAtLeast(screen.x))
        val y = (location.y + origin.y / scale).roundToInt().coerceIn(screen.y, (screen.y+screen.height-132).coerceAtLeast(screen.y))
        popup.setLocation(x, y)
        popup.isVisible = owner.isActive
        popup.repaint()
    }
}

@Composable
private fun PlayerButton(label: String, text: String = "", enabled: Boolean = true, icon: ImageVector? = null, emphasized: Boolean = false, onClick: () -> Unit) {
    val contentColor = if (emphasized) playerAccent else playerInk
    // 提示留在控件条内，避免被相邻的原生视频子窗口遮挡。
    WorkbenchTooltip(label, besideControl = true) {
        Button(onClick = onClick, enabled = enabled, contentPadding = PaddingValues(0.dp),
            colors = ButtonDefaults.buttonColors(
                containerColor = if (emphasized) playerAccent.copy(alpha = .12f) else Color.Transparent,
                contentColor = contentColor,
                disabledContainerColor = Color.Transparent,
                disabledContentColor = playerMuted.copy(alpha = .45f),
            ),
            hoverColor = playerInk.copy(alpha = .10f), pressedColor = playerInk.copy(alpha = .18f),
            modifier = Modifier.width(38.dp).height(36.dp).reportControlBounds("video.$label").semantics { contentDescription = label }) {
            if (icon != null) Icon(icon, null, modifier = Modifier.size(20.dp))
            else Text(text, fontSize = 12.sp)
        }
    }
}

@Composable
private fun PlayerRate(rate: Double, enabled: Boolean, onSelect: (Double) -> Unit) {
    var expanded by remember { mutableStateOf(false) }
    Box {
        TextButton(onClick = { expanded = true }, enabled = enabled, hoverColor = playerInk.copy(alpha = .10f), pressedColor = playerInk.copy(alpha = .18f), modifier = Modifier.reportControlBounds("video.rate").semantics { contentDescription = "播放倍速" }) { Text("${rate}×", color = if (enabled) playerAccent else playerMuted.copy(alpha = .45f), fontSize = 12.sp) }
        DropdownMenu(expanded, { expanded = false }) {
            listOf(.5, .75, 1.0, 1.25, 1.5, 2.0, 2.5, 3.0).forEach { value ->
                DropdownMenuItem(text = { Text("${value}×") }, modifier = Modifier.reportControlBounds("video.rate.$value"), onClick = { expanded = false; onSelect(value) })
            }
        }
    }
}

@Composable
private fun PlayerTracks(label: String, tracks: List<PlayerTrackDto>, allowOff: Boolean, enabled: Boolean, onSelect: (String) -> Unit) {
    var expanded by remember { mutableStateOf(false) }
    Box {
        TextButton(onClick = { expanded = true }, enabled = enabled && tracks.isNotEmpty(), hoverColor = playerInk.copy(alpha = .10f), pressedColor = playerInk.copy(alpha = .18f), modifier = Modifier.reportControlBounds("video.$label"), contentPadding = PaddingValues(horizontal = 5.dp)) { Text(label, color = if (enabled && tracks.isNotEmpty()) playerInk else playerMuted.copy(alpha = .45f), fontSize = 11.sp) }
        DropdownMenu(expanded, { expanded = false }) {
            if (allowOff) DropdownMenuItem(text = { Text("关闭字幕") }, modifier = Modifier.reportControlBounds("video.$label.no"), onClick = { expanded = false; onSelect("no") })
            tracks.forEach { track ->
                val name = listOf(track.title, track.language).filter { it.isNotBlank() }.joinToString(" · ").ifBlank { "$label ${track.id}" }
                DropdownMenuItem(text = { Text((if (track.selected) "✓ " else "") + name) }, modifier = Modifier.reportControlBounds("video.$label.${track.id}"), onClick = { expanded = false; onSelect(track.id) })
            }
        }
    }
}
