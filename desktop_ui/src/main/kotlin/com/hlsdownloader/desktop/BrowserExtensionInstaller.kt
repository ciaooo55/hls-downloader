package com.hlsdownloader.desktop

import java.io.File
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.StandardCopyOption
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
    val root = Path.of(
        System.getenv("LOCALAPPDATA") ?: System.getProperty("user.home"),
        "HLS Downloader",
        "browser-extension",
    )
    Files.createDirectories(root)
    val stamp = "${Files.size(archive)}-${Files.getLastModifiedTime(archive).toMillis()}"
    val target = root.resolve("Chromium-${Product.version}-$stamp")
    if (isCurrentChromiumExtension(target)) return target
    val stage = Files.createTempDirectory(root, ".chromium-")
    unpackChromiumExtension(archive, stage)
    check(isCurrentChromiumExtension(stage)) { "Chromium 插件包缺少有效的 ${Product.version} manifest.json" }
    return runCatching {
        Files.move(stage, target, StandardCopyOption.ATOMIC_MOVE)
    }.recoverCatching {
        Files.move(stage, target)
    }.getOrThrow()
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
    ZipInputStream(Files.newInputStream(archive)).use { zip ->
        while (true) {
            val entry = zip.nextEntry ?: break
            val target = root.resolve(entry.name.replace('\\', '/')).normalize()
            require(target.startsWith(root)) { "插件包包含越界路径：${entry.name}" }
            if (entry.isDirectory) {
                Files.createDirectories(target)
            } else {
                target.parent?.let(Files::createDirectories)
                Files.copy(zip, target, StandardCopyOption.REPLACE_EXISTING)
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
