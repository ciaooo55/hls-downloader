package com.hlsdownloader.desktop

import java.awt.GraphicsEnvironment
import java.awt.Rectangle
import java.nio.file.Files
import java.nio.file.Path

/**
 * 记住主窗口的宽高。
 *
 * 只记宽高、不记坐标：换显示器时旧坐标可能落在屏幕外，而宽高只要夹进当前屏幕的可用范围就一定安全。
 *
 * 存放在 `%LOCALAPPDATA%\HLSDownloader\`，与实例锁同一目录。
 * 审计夹具模式（设置了 `HLS_UI_AUDIT_WIDTH` / `HLS_UI_AUDIT_HEIGHT`）下既不读也不写——
 * 否则截图尺寸会被历史记录污染，同一夹具在不同机器上出图不一致。
 */
internal object WindowGeometryStore {
    private const val MIN_WIDTH = 1024
    private const val MIN_HEIGHT = 600
    private const val DEFAULT_WIDTH = 1400
    private const val DEFAULT_HEIGHT = 820

    private val file: Path by lazy {
        val root = System.getenv("LOCALAPPDATA")?.takeIf(String::isNotBlank)?.let(Path::of)
            ?: Path.of(System.getProperty("user.home"), "AppData", "Local")
        root.resolve("HLSDownloader").resolve("window-geometry.properties")
    }

    /** 返回可直接交给 `rememberWindowState` 的宽高，已夹进 [MIN_*, 屏幕可用范围]。 */
    fun load(): Pair<Int, Int> {
        val stored = read()
        return clamp(stored?.first ?: DEFAULT_WIDTH, stored?.second ?: DEFAULT_HEIGHT)
    }

    fun save(width: Int, height: Int) {
        val (clampedWidth, clampedHeight) = clamp(width, height)
        runCatching {
            Files.createDirectories(file.parent)
            Files.writeString(file, "width=$clampedWidth\nheight=$clampedHeight\n")
        }
    }

    private fun read(): Pair<Int, Int>? = runCatching {
        val lines = Files.readString(file).lineSequence().toList()
        fun value(key: String) = lines.firstOrNull { it.startsWith("$key=") }
            ?.removePrefix("$key=")?.trim()?.toIntOrNull()
        val width = value("width")
        val height = value("height")
        if (width == null || height == null) null else width to height
    }.getOrNull()

    /**
     * 有效最小尺寸：**不能超过工作区**。
     *
     * 写死 1024×600 时，工作区比它还小的设备会让窗口比屏幕还大，右下角（含状态栏）永远够不到。
     * 这不是假想：`maximumWindowBounds` 返回的是**逻辑值**（本机 1920×1080@125% 返回 1536×816），
     * 于是下面这些常见档位的工作区都会小于 1024×600——
     *
     * - 1366×768 @150% → 约 911×480
     * - 1600×900 @150% → 约 1067×568（高度不够）
     * - 1920×1080 @200% → 约 960×540
     *
     * 本机只有 125%（工作区 1536×816），这些档位**无法实测**，所以这里只是按逻辑把值夹住：
     * 宁可让窗口比"理想最小值"小，也不要主动把它撑到屏幕外面去。
     */
    internal fun effectiveMinimum(hardMinimum: Int, workAreaSize: Int?): Int =
        if (workAreaSize == null) hardMinimum else minOf(hardMinimum, workAreaSize)

    /** 交给 `window.minimumSize` 的最小尺寸，已按工作区夹过。 */
    internal fun minimumWindowSize(): Pair<Int, Int> = Pair(
        effectiveMinimum(MIN_WIDTH, workArea?.width),
        effectiveMinimum(MIN_HEIGHT, workArea?.height),
    )

    /**
     * 屏幕可用范围取不到时（无图形环境）只夹下限，不夹上限。
     *
     * 上下限都要用 [effectiveMinimum] 推出的值：原来写成 `coerceAtLeast(MIN_WIDTH)`，
     * 在工作区只有 911dp 宽时会得到 `coerceIn(1024, 1024)`，**等于强制把窗口设成比屏幕还宽**。
     */
    private fun clamp(width: Int, height: Int): Pair<Int, Int> =
        clampToWorkArea(width, height, workArea?.let { it.width to it.height })

    /**
     * [clamp] 的纯函数版本：工作区尺寸由参数传入，便于用本机取不到的档位（更小的桌面）验证。
     * [workAreaSize] 为 null 表示无图形环境——此时只夹下限，不夹上限。
     */
    internal fun clampToWorkArea(width: Int, height: Int, workAreaSize: Pair<Int, Int>?): Pair<Int, Int> {
        val areaWidth = workAreaSize?.first
        val areaHeight = workAreaSize?.second
        val minWidth = effectiveMinimum(MIN_WIDTH, areaWidth)
        val minHeight = effectiveMinimum(MIN_HEIGHT, areaHeight)
        val maxWidth = areaWidth?.coerceAtLeast(minWidth) ?: Int.MAX_VALUE
        val maxHeight = areaHeight?.coerceAtLeast(minHeight) ?: Int.MAX_VALUE
        return width.coerceIn(minWidth, maxWidth) to height.coerceIn(minHeight, maxHeight)
    }

    private val workArea: Rectangle? by lazy {
        runCatching { GraphicsEnvironment.getLocalGraphicsEnvironment().maximumWindowBounds }.getOrNull()
    }

    /**
     * 把窗口摆到工作区正中，返回可直接交给 `rememberWindowState` 的 dp 坐标；无图形环境时返回 null。
     *
     * 为什么必须自己摆：Windows 把新窗口放在层叠偏移处（实测恒为 (48,48)，与窗口大小无关）。
     * 本机 125% 缩放下工作区只有 816dp 高，而窗口高度上限就是 816dp，
     * 于是 48 + 816 = 864 超出工作区底边，**底部 48px 连同 28dp 的状态栏一起落到任务栏下面**。
     * 凡是工作区高度小于约 868dp 的设备（1366×768、1080p@125%）都会中招。
     *
     * 按工作区（而不是屏幕）居中，配合 [load] 的高度上限，可保证窗口完整落在工作区内：
     * 高度 ≤ 工作区高度 ⇒ 居中后的 y ≥ 工作区顶边 ⇒ 底边 ≤ 工作区底边。
     */
    fun centeredPosition(width: Int, height: Int): Pair<Int, Int>? {
        val bounds = workArea ?: return null
        return (bounds.x + (bounds.width - width).coerceAtLeast(0) / 2) to
            (bounds.y + (bounds.height - height).coerceAtLeast(0) / 2)
    }
}
