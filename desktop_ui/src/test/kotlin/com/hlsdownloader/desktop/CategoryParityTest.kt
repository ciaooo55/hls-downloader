package com.hlsdownloader.desktop

import java.io.File
import kotlin.test.Test
import kotlin.test.assertTrue

/**
 * 下载分类表的**跨语言**守卫。
 *
 * 这张表在 Rust（`native_shell/src/category.rs::download_category`）和 Kotlin
 * （`Main.kt::downloadCategory`）各有一份。历史上它们分叉过：Kotlin 那份漏了
 * ts / m4v / m4a / jpg / png / gif / webp，多了 Core 不认识的 apk / dmg / pkg，
 * 又少了 iso，还没有 hls/dash/live 的媒体短路。症状是同一个 .ts 文件在接管弹窗里
 * 归"媒体"、在任务栏里归"其他"。
 *
 * 判据直接读 Rust 源码里的字符串字面量，与 Kotlin 的集合逐一比对。任何一边单独
 * 改表，这条都会失败——逼着改的人一次改完。
 */
class CategoryParityTest {
    private fun categorySource(): List<String> =
        File("../native_shell/src/category.rs").readText().split("\n")

    private fun recognizeSource(): String =
        File("../native_shell/src/recognize.rs").readText()

    /**
     * 按行扫 `download_category` 的函数体：形如扩展名的字面量进暂存区，
     * 遇到"这一行只有一个分类名"就结算给上一个分支。
     * `return "media";` 这种短路直接跳过——它不带扩展表。
     */
    private fun rustBranches(): Map<String, Set<String>> {
        val lines = categorySource()
        val start = lines.indexOfFirst { it.startsWith("pub fn download_category(") }
        assertTrue(start >= 0, "download_category not found in category.rs")
        // 函数体到下一个顶层声明为止（第一个匹配的 "}" 是 if matches! 那个块的收尾，
        // 不是函数本身的右括号）。
        var end = start + 1
        while (end < lines.size && !lines[end].startsWith("pub fn ") && !lines[end].startsWith("impl ")) {
            end += 1
        }
        assertTrue(end < lines.size, "download_category 后面的顶层声明没找到")
        val pending = mutableSetOf<String>()
        val result = mutableMapOf<String, Set<String>>()
        for (raw in lines.subList(start + 1, end)) {
            val line = raw.trim()
            if (line.isEmpty() || line.startsWith("//")) continue
            // 短路分支先跳过：它只有 return、没有扩展表。
            if (line.startsWith("return ")) continue
            val literals = QUOTED_LITERAL.findAll(line).map { it.groupValues[1] }.toList()
            val only = literals.singleOrNull { it in CATEGORY_NAMES }
            if (only != null && literals.size == 1) {
                result[only] = pending.toSet()
                pending.clear()
                continue
            }
            pending += literals.filter { it !in CATEGORY_NAMES && it.length <= 5 }
        }
        return result
    }

    @Test
    fun kotlin_category_table_matches_the_rust_source() {
        // 直接拿生产代码里的三张表来比，不另抄一份——初版曾经拿测试侧的拷贝去比，
        // 负向对照一击就穿了（Rust 有 ts、Kotlin 没有时这条照样绿）。
        val rust = rustBranches()
        assertTrue(rust.size == 4, "没能从 category.rs 解析出四个分支：$rust")
        assertTrue(rust["other"]!!.isEmpty(), "other 分支不该带扩展名：${rust["other"]}")
        assertTrue(rust["media"] == catMEDIA_EXTENSIONS, "media 表与 Rust 不一致，缺：${rust["media"]!! - catMEDIA_EXTENSIONS}，多：${catMEDIA_EXTENSIONS - rust["media"]!!}")
        assertTrue(rust["program"] == catPROGRAM_EXTENSIONS, "program 表与 Rust 不一致，缺：${rust["program"]!! - catPROGRAM_EXTENSIONS}，多：${catPROGRAM_EXTENSIONS - rust["program"]!!}")
        assertTrue(rust["archive"] == catARCHIVE_EXTENSIONS, "archive 表与 Rust 不一致，缺：${rust["archive"]!! - catARCHIVE_EXTENSIONS}，多：${catARCHIVE_EXTENSIONS - rust["archive"]!!}")
    }

    @Test
    fun the_media_short_circuit_matches_the_rust_resource_kinds() {
        // Rust：ResourceKind::Hls | Dash | Live 一律算媒体，不看扩展名。
        val body = recognizeSource().substringAfter("pub fn classify_url(")
        for (kind in listOf("Hls", "Dash", "Live")) {
            assertTrue(
                body.contains("ResourceKind::$kind"),
                "Rust 的 ResourceKind 找不到 $kind，短路规则该重新对了",
            )
        }
        for (kind in listOf("hls", "dash", "live")) {
            assertTrue(
                downloadCategory("index.bin", kind) == TaskCategory.MEDIA,
                "$kind 现在不是媒体分类了——Rust 侧仍然是",
            )
        }
        // Core 的 ResourceKind 没有 media 变体；Kotlin 保留它只为兼容旧接管报文。
        // 把它钉住，免得有人以为真有这么一个类型、顺手再加一个。
        assertTrue(
            downloadCategory("index.bin", "media") == TaskCategory.MEDIA,
            "media 这个兼容值被删了",
        )
    }

    @Test
    fun the_fork_that_made_the_same_file_two_categories_stays_deleted() {
        // 分叉期的症状：.ts 段文件在任务栏里是"其他"、在接管弹窗里是"媒体"。
        assertTrue(downloadCategory("segment.ts", "file") == TaskCategory.MEDIA, ".ts 必须是媒体")
        assertTrue(downloadCategory("cover.jpg", "file") == TaskCategory.MEDIA, ".jpg 必须是媒体")
        assertTrue(downloadCategory("cover.png", "file") == TaskCategory.MEDIA, ".png 必须是媒体")
        // Core 不认识的扩展名不该再被 Kotlin 单方面算成"程序"。
        assertTrue(downloadCategory("android.apk", "file") == TaskCategory.OTHER, ".apk Core 不认")
        // Rust 有的 iso 不能再丢。
        assertTrue(downloadCategory("image.iso", "file") == TaskCategory.ARCHIVE, ".iso 必须是压缩包")
        // 大小写不敏感，与 Rust 的 to_ascii_lowercase 对齐。
        assertTrue(downloadCategory("CLIP.MP4", "file") == TaskCategory.MEDIA, "大写扩展名")
    }
}

private val QUOTED_LITERAL = Regex("\"([a-z0-9]+)\"")
private val CATEGORY_NAMES = setOf("media", "program", "archive", "other")
