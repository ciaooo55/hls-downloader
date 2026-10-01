package com.hlsdownloader.desktop

import java.io.File
import kotlin.test.Test
import kotlin.test.assertTrue

/**
 * 下载分类表的**跨语言**守卫。
 *
 * 这张表在 Rust（`native_shell/src/media_ext.rs::MEDIA_FOLDER_EXTENSIONS` 等）和
 * Kotlin（`Main.kt::downloadCategory`）各有一份。历史上它们分叉过：Kotlin 那份漏了
 * ts / m4v / m4a / jpg / png / gif / webp，多了 Core 不认识的 apk / dmg / pkg，
 * 又少了 iso，还没有 hls/dash/live 的媒体短路。症状是同一个 .ts 文件在接管弹窗里
 * 归"媒体"、在任务栏里归"其他"。
 *
 * 判据按**常量名**从 Rust 源码取表，与 Kotlin 的集合逐一比对。任何一边单独改表，
 * 这条都会失败——逼着改的人一次改完。
 *
 * 扫描源曾经是逐行扫 `category.rs::download_category` 的函数体。后来 Rust 侧把
 * media / program 两张表搬进了 `media_ext.rs`，逐行扫就再也找不到字面量，Rust 侧
 * 被读成空集，于是它把 Kotlin 那 28 项全判成"多"——那是扫描器的锅，不是两边真分叉。
 * 所以改成按常量名取表：既精确，也不怕以后再搬。
 */
class CategoryParityTest {
    private fun mediaExtSource(): String =
        File("../native_shell/src/media_ext.rs").readText()

    private fun categorySource(): String =
        File("../native_shell/src/category.rs").readText()

    private fun recognizeSource(): String =
        File("../native_shell/src/recognize.rs").readText()

    /**
     * 从 `media_ext.rs` 的 `pub const NAME: &[&str] = &[ ... ];` 里按名字取表。
     */
    private fun rustTable(constant: String): Set<String> {
        val source = mediaExtSource()
        val declaration = source.indexOf("pub const $constant")
        assertTrue(declaration >= 0, "$constant 在 media_ext.rs 里找不到")
        val open = source.indexOf('[', declaration)
        val close = source.indexOf("];", open)
        assertTrue(declaration < open && open < close, "$constant 的数组没闭合")
        return QUOTED_LITERAL.findAll(source.substring(open, close))
            .map { it.groupValues[1] }
            .toSet()
    }

    /**
     * archive 表仍留在 `category.rs` 里（没有搬），取 `"archive"` 之前最近的那个
     * `matches!` 块。media / program 已搬去 [rustTable]。
     */
    private fun rustArchiveBranch(): Set<String> {
        val source = categorySource()
        val marker = source.indexOf("\"archive\"")
        assertTrue(marker > 0, "category.rs 里找不到 archive 分支")
        val block = source.substring(0, marker)
        val matches = block.lastIndexOf("matches!(")
        assertTrue(matches >= 0, "archive 分支前面的 matches! 块没找到")
        return QUOTED_LITERAL.findAll(block.substring(matches))
            .map { it.groupValues[1] }
            .toSet()
    }

    private fun rustBranches(): Map<String, Set<String>> = mapOf(
        "media" to rustTable("MEDIA_FOLDER_EXTENSIONS"),
        "program" to rustTable("EXECUTABLE_EXTENSIONS"),
        "archive" to rustArchiveBranch(),
        // category.rs 的 else 分支就是"其它"，不带扩展表；这里显式给空集，
        // 好让"四个分支"这个结构断言与下载分类的枚举成员一一对应。
        "other" to emptySet(),
    )

    @Test
    fun kotlin_category_table_matches_the_rust_source() {
        // 直接拿生产代码里的三张表来比，不另抄一份——初版曾经拿测试侧的拷贝去比，
        // 负向对照一击就穿了（Rust 有 ts、Kotlin 没有时这条照样绿）。
        val rust = rustBranches()
        assertTrue(rust.size == 4, "没能解析出四个分支：$rust")
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

    @Test
    fun playable_media_never_lands_in_the_other_folder() {
        // Core 的 completed_actions 按 MEDIA_PLAYABLE_EXTENSIONS 给"播放 / 投屏"动作。
        // 它们必须同时归"媒体"文件夹，否则同一个文件在动作菜单里是媒体、在分类筛选和
        // 保存目录里却是"其他"——这正是本轮修掉的自相矛盾。Rust 侧有
        // playable_is_subset_of_folder 盯着两张表的关系，但两张表都在 Rust 里；
        // 这条负责跨语言那一侧：只改 Rust 的 playable 表、不改 Kotlin 的 folder 表，
        // 也必须红。
        val playable = rustTable("MEDIA_PLAYABLE_EXTENSIONS")
        assertTrue(
            playable.all { it in catMEDIA_EXTENSIONS },
            "这些扩展名可播放却不在媒体分类表里：${playable - catMEDIA_EXTENSIONS}",
        )
    }
}

private val QUOTED_LITERAL = Regex("\"([a-z0-9]+)\"")
