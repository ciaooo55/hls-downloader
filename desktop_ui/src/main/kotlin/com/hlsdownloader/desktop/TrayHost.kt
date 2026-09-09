package com.hlsdownloader.desktop

import java.awt.EventQueue
import java.awt.Image
import java.awt.MenuItem
import java.awt.PopupMenu
import java.awt.SystemTray
import java.awt.TrayIcon
import javax.imageio.ImageIO

// 关闭主窗口后由托盘承接驻留：后台任务继续，真正退出只通过托盘菜单。
// 暂停/继续全部任务的动作由 AppShell 注册（TrayActions），托盘自身不感知任务状态。
internal object TrayHost {
    @Volatile private var icon: TrayIcon? = null
    @Volatile private var minimizedNoticeShown = false

    fun install(onShow: () -> Unit, onExit: () -> Unit): Boolean {
        if (!SystemTray.isSupported()) return false
        return runCatching {
            val image: Image = ImageIO.read(TrayHost::class.java.classLoader.getResource("app-icon.png"))
                ?: return false
            val menu = PopupMenu().apply {
                fun item(label: String, action: () -> Unit) = MenuItem(label).apply {
                    addActionListener { EventQueue.invokeLater { runCatching(action) } }
                }
                add(item("显示主窗口", onShow))
                addSeparator()
                add(item("全部暂停") { TrayActions.pauseAll?.invoke() })
                add(item("全部继续") { TrayActions.resumeAll?.invoke() })
                addSeparator()
                add(item("退出", onExit))
            }
            val trayIcon = TrayIcon(image, "HLS Downloader ${Product.version}", menu).apply {
                isImageAutoSize = true
                addActionListener { EventQueue.invokeLater { runCatching(onShow) } }
            }
            SystemTray.getSystemTray().add(trayIcon)
            icon = trayIcon
            true
        }.getOrDefault(false)
    }

    fun remove() {
        runCatching {
            icon?.let { SystemTray.getSystemTray().remove(it) }
            icon = null
        }
    }

    fun notifyMinimized() {
        if (minimizedNoticeShown) return
        minimizedNoticeShown = true
        runCatching {
            icon?.displayMessage("HLS Downloader", "已最小化到系统托盘，下载任务继续进行", TrayIcon.MessageType.INFO)
        }
    }
}

internal object TrayActions {
    @Volatile var pauseAll: (() -> Unit)? = null
    @Volatile var resumeAll: (() -> Unit)? = null
}

// 关窗隐藏窗口需要 Window 引用，而 onCloseRequest 参数位于 WindowScope 之外，经此中转。
internal object WorkbenchWindow {
    @Volatile var awtWindow: java.awt.Window? = null
    @Volatile var trayResident = false

    fun hideToTray() {
        awtWindow?.isVisible = false
        TrayHost.notifyMinimized()
    }
}
