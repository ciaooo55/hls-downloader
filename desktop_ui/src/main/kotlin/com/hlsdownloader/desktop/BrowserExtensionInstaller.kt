package com.hlsdownloader.desktop

import java.io.File
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.StandardCopyOption
import java.util.Locale
import java.util.zip.ZipInputStream
import javax.swing.filechooser.FileSystemView

internal const val FIREFOX_ADDON_URL = "https://addons.mozilla.org/zh-CN/firefox/addon/hls_downloader/"

internal data class BrowserExtensionInstallResult(
    val ok: Boolean,
    val path: String = "",
    val browserOpened: Boolean = false,
    val error: String = "",
)

internal fun openChromiumExtensionInstaller(): BrowserExtensionInstallResult = runCatching {
    val extension = locateChromiumExtension() ?: extractChromiumExtension()
    val browserOpened = openChromiumExtensionsPage()
    ProcessBuilder("explorer.exe", extension.toString()).start()
    BrowserExtensionInstallResult(true, extension.toString(), browserOpened)
}.getOrElse { BrowserExtensionInstallResult(false, error = it.message ?: "无法打开 Chromium 插件安装工具") }

internal fun openFirefoxAddonPage(): Boolean =
    runCatching { ProcessBuilder("explorer.exe", FIREFOX_ADDON_URL).start() }.isSuccess

private fun locateChromiumExtension(): Path? {
    val resources = System.getProperty("compose.application.resources.dir")?.takeIf(String::isNotBlank)?.let(Path::of)
    resources?.parent?.parent?.resolve("extensions")?.resolve("Chromium")
        ?.takeIf(::isCurrentChromiumExtension)?.let { return it }
    val desktop = FileSystemView.getFileSystemView().homeDirectory.toPath()
    val working = File(System.getProperty("user.dir")).toPath()
    return listOf(
        desktop.resolve("HLSDownloader-${Product.version}-Chromium"),
        desktop.resolve("HLSDownloader-Chromium"),
        working.resolve("extension/.output/chrome-mv3"),
    ).firstOrNull(::isCurrentChromiumExtension)
}

private fun extractChromiumExtension(): Path {
    val archive = chromiumArchives().firstOrNull(Files::isRegularFile)
        ?: error("安装目录和桌面都没有 HLS Downloader ${Product.version} Chromium 插件包")
    val root = WorkbenchPaths.uiDirectory.resolve("browser-extension")
    Files.createDirectories(root)
    val stamp = "${Files.size(archive)}-${Files.getLastModifiedTime(archive).toMillis()}"
    val target = root.resolve("Chromium-${Product.version}-$stamp")
    if (isCurrentChromiumExtension(target)) return target
    val stage = Files.createTempDirectory(root, ".chromium-")
    try {
        unpackChromiumExtension(archive, stage)
        check(isCurrentChromiumExtension(stage)) { "Chromium 插件包缺少有效的 ${Product.version} manifest.json" }
        if (Files.exists(target)) deleteRecursively(target)
        return runCatching {
            Files.move(stage, target, StandardCopyOption.ATOMIC_MOVE)
        }.recoverCatching {
            Files.move(stage, target, StandardCopyOption.REPLACE_EXISTING)
        }.getOrThrow()
    } catch (error: Throwable) {
        deleteRecursively(stage)
        throw error
    }
}

private fun deleteRecursively(root: Path) {
    if (!Files.exists(root)) return
    runCatching {
        Files.walk(root).use { stream ->
            stream.sorted(Comparator.reverseOrder()).forEach { path ->
                runCatching { Files.deleteIfExists(path) }
            }
        }
    }
}

private fun chromiumArchives(): List<Path> {
    val desktop = FileSystemView.getFileSystemView().homeDirectory.toPath()
    val working = File(System.getProperty("user.dir")).toPath()
    val resources = System.getProperty("compose.application.resources.dir")
        ?.takeIf(String::isNotBlank)
        ?.let(Path::of)
    val name = "HLSDownloader-${Product.version}-Chromium.zip"
    return listOfNotNull(
        resources?.resolve("extensions/$name"),
        working.resolve("app/resources/extensions/$name"),
        working.resolve("extensions/$name"),
        desktop.resolve("HLSDownloader-Chromium.zip"),
        desktop.resolve(name),
    )
}

internal fun unpackChromiumExtension(archive: Path, destination: Path) {
    val root = destination.toAbsolutePath().normalize()
    val maxBytes = 32L * 1024 * 1024
    var expandedBytes = 0L
    var entries = 0
    val paths = mutableSetOf<String>()
    val buffer = ByteArray(8192)
    val deviceName = Regex("(?i)(CON|PRN|AUX|NUL|COM[1-9¹²³]|LPT[1-9¹²³])")
    ZipInputStream(Files.newInputStream(archive)).use { zip ->
        while (true) {
            val entry = zip.nextEntry ?: break
            require(++entries <= 2048) { "插件包条目数量超过限制" }
            val name = entry.name.replace('\\', '/').let { if (entry.isDirectory) it.removeSuffix("/") else it }
            require(name.split('/').all { part ->
                part.isNotEmpty() && part != "." && part != ".." &&
                    !part.endsWith(' ') && !part.endsWith('.') &&
                    part.none { it.isISOControl() || it in "<>:\"|?*" } &&
                    !deviceName.matches(part.substringBefore('.'))
            }) { "插件包包含无效路径：${entry.name}" }
            require(paths.add(name.lowercase(Locale.ROOT))) { "插件包包含重复路径：${entry.name}" }
            require(entry.size <= maxBytes - expandedBytes) { "插件包解压大小超过限制" }
            val target = root.resolve(name).normalize()
            require(target.startsWith(root)) { "插件包包含越界路径：${entry.name}" }
            if (entry.isDirectory) {
                Files.createDirectories(target)
            } else {
                target.parent?.let(Files::createDirectories)
            }
            val output = if (entry.isDirectory) null else Files.newOutputStream(target)
            output.use { stream ->
                // Count actual bytes, including directory bodies; ZIP sizes may be unknown or forged.
                while (true) {
                    val count = zip.read(buffer, 0, minOf(buffer.size.toLong(), maxBytes - expandedBytes + 1).toInt())
                    if (count < 0) break
                    expandedBytes += count
                    require(expandedBytes <= maxBytes) { "插件包解压大小超过限制" }
                    stream?.write(buffer, 0, count)
                }
            }
            zip.closeEntry()
        }
    }
}

private fun isCurrentChromiumExtension(path: Path): Boolean {
    val manifest = path.resolve("manifest.json")
    if (!Files.isRegularFile(manifest)) return false
    val text = runCatching { Files.readString(manifest) }.getOrNull() ?: return false
    return Regex(""""version"\s*:\s*"${Regex.escape(Product.version)}"""").containsMatchIn(text) &&
        Regex(""""default_popup"\s*:\s*"popup\.html"""").containsMatchIn(text)
}

private fun openChromiumExtensionsPage(): Boolean {
    val programFiles = listOfNotNull(System.getenv("PROGRAMFILES(X86)"), System.getenv("PROGRAMFILES"))
    val localAppData = System.getenv("LOCALAPPDATA")
    val candidates = buildList {
        programFiles.forEach {
            add(File(it, "Microsoft/Edge/Application/msedge.exe") to "edge://extensions")
            add(File(it, "Google/Chrome/Application/chrome.exe") to "chrome://extensions")
        }
        if (localAppData != null) add(File(localAppData, "Google/Chrome/Application/chrome.exe") to "chrome://extensions")
    }
    return candidates.firstOrNull { it.first.isFile }?.let { (browser, page) ->
        runCatching { ProcessBuilder(browser.absolutePath, page).start() }.isSuccess
    } == true
}
