package com.ciaooo55.hlsdownloader

import java.awt.Desktop
import java.io.File
import java.net.URI
import java.nio.file.Files
import java.nio.file.Path
import javax.swing.JFileChooser
import javax.swing.filechooser.FileNameExtensionFilter

object NativeDialogs {
    fun chooseFolder(initial: String = ""): String? {
        val picker = JFileChooser(initial.takeIf { it.isNotBlank() }?.let(::File))
        picker.dialogTitle = "选择文件夹"
        picker.fileSelectionMode = JFileChooser.DIRECTORIES_ONLY
        picker.isAcceptAllFileFilterUsed = false
        return if (picker.showOpenDialog(null) == JFileChooser.APPROVE_OPTION) picker.selectedFile.absolutePath else null
    }

    fun chooseTorrent(): Path? {
        val picker = JFileChooser()
        picker.dialogTitle = "导入种子文件"
        picker.fileFilter = FileNameExtensionFilter("BitTorrent 文件 (*.torrent)", "torrent")
        return if (picker.showOpenDialog(null) == JFileChooser.APPROVE_OPTION) picker.selectedFile.toPath() else null
    }

    fun openUri(value: String) {
        if (Desktop.isDesktopSupported()) Desktop.getDesktop().browse(URI.create(value))
    }

    fun openFolder(path: Path) {
        if (Files.exists(path) && Desktop.isDesktopSupported()) Desktop.getDesktop().open(path.toFile())
    }
}
