package com.hlsdownloader.desktop

// 关窗隐藏窗口需要 Window 引用，而 onCloseRequest 参数位于 WindowScope 之外，经此中转。
// 托盘图标和菜单由 resident native Core 唯一持有；这里不再创建 AWT SystemTray。
internal object WorkbenchWindow {
    @Volatile var awtWindow: java.awt.Window? = null

    fun hideToTray() {
        awtWindow?.isVisible = false
    }

    fun show() {
        java.awt.EventQueue.invokeLater {
            val window = awtWindow ?: return@invokeLater
            window.isVisible = true
            (window as? java.awt.Frame)?.let {
                it.extendedState = it.extendedState and java.awt.Frame.ICONIFIED.inv()
            }
            window.toFront()
            window.requestFocus()
        }
    }
}
