package com.ciaooo55.hlsdownloader

import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.isCtrlPressed
import androidx.compose.ui.input.key.isShiftPressed
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.type
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.unit.DpSize
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Tray
import androidx.compose.ui.window.Window
import androidx.compose.ui.window.application
import androidx.compose.ui.window.rememberWindowState
import kotlinx.coroutines.runBlocking
import java.util.prefs.Preferences
import java.awt.Dimension
import javax.swing.JOptionPane

fun main(args: Array<String>) {
    val bootstrap = runCatching { runBlocking { DesktopRuntime.bootstrap() } }.getOrElse { reason ->
        JOptionPane.showMessageDialog(null, reason.message ?: "桌面程序启动失败", "HLS Downloader 启动失败", JOptionPane.ERROR_MESSAGE)
        return
    }
    if (bootstrap.existingInstance) return
    val background = args.any { it == "--background" || it == "--native-host" }
    val preferences = Preferences.userRoot().node("com/ciaooo55/hlsdownloader")

    application {
        var visible by remember { mutableStateOf(!background) }
        var dark by remember { mutableStateOf(preferences.getBoolean("darkTheme", true)) }
        var keys by remember { mutableStateOf(KeyboardSelection()) }
        var shuttingDown by remember { mutableStateOf(false) }
        // Compose/AWT can treat the requested DpSize as physical pixels on a
        // scaled Windows desktop. A wider default keeps both toolbar groups and
        // the task columns visible at 125–150% DPI.
        val windowState = rememberWindowState(size = DpSize(1440.dp, 840.dp))

        fun quit() {
            if (shuttingDown) return
            shuttingDown = true
        }

        val state = remember {
            AppState(
                bootstrap,
                onActivate = { visible = true },
                onExit = { quit() },
            )
        }
        LaunchedEffect(Unit) { state.start() }
        DisposableEffect(Unit) { onDispose { state.dispose() } }
        LaunchedEffect(shuttingDown) {
            if (shuttingDown) {
                state.dispose()
                DesktopRuntime.shutdown(bootstrap)
                exitApplication()
            }
        }

        HlsTheme(dark) {
            val appIcon = painterResource("app-icon.png")
            Window(
                onCloseRequest = { visible = false },
                visible = visible,
                title = "HLS Downloader",
                icon = appIcon,
                state = windowState,
                onPreviewKeyEvent = { event ->
                    keys = KeyboardSelection(event.isCtrlPressed, event.isShiftPressed)
                    when {
                        event.type == KeyEventType.KeyDown && event.isCtrlPressed && event.key == Key.F -> { state.query = state.query; false }
                        event.type == KeyEventType.KeyDown && event.key == Key.Escape -> false
                        else -> false
                    }
                },
            ) {
                window.minimumSize = Dimension(1100, 650)
                MainContent(state, dark, onToggleTheme = {
                    dark = !dark
                    preferences.putBoolean("darkTheme", dark)
                }, keys = keys)
            }
            Tray(
                icon = appIcon,
                tooltip = "HLS Downloader",
                onAction = { visible = true },
                menu = {
                    Item("打开下载器", onClick = { visible = true })
                    Separator()
                    Item("退出", onClick = { quit() })
                },
            )
            state.handoffs.forEach { handoff ->
                key(handoff.id) { HandoffWindow(state, handoff, dark) }
            }
        }
    }
}
