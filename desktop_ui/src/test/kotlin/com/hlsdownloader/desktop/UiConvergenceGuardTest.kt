package com.hlsdownloader.desktop

import java.io.File
import kotlin.test.Test
import kotlin.test.assertTrue

/**
 * 「同一个视觉规则只准写一次」的守卫。
 *
 * 一百多轮增量打补丁之后，桌面端留下三类"一半收敛"的写法：
 *  - 按压缩放的状态在 `rememberPressFeedback`，**应用**它却是三行一模一样的修饰手抄 9 处；
 *  - 竖向滚动条外观抄了 4 份，还有 3 处干脆不传 style（用 Compose 自带灰条）；
 *  - 字号早就走 `TypeScale` 了，行高还是 24 处裸字面量（16 / 17 / 19sp 三档）。
 *
 * 这里把判据下沉到源码结构，谁再抄一份，这条直接失败。
 *
 * 判据里的"禁用字样"在测试中是运行时拼接的，避免自己违反自己。
 */
class UiConvergenceGuardTest {
    private fun sources(): List<String> {
        val root = File("src/main/kotlin/com/hlsdownloader/desktop")
        return root.listFiles { file -> file.extension == "kt" }
            ?.map { it.path.replace('\\', '/') }
            ?.sorted()
            ?: emptyList()
    }

    private fun linesOf(path: String): List<String> = File(path).readText().split("\n")

    private val feedbackChain: List<String> = listOf(
        "graphicsLayer { scaleX = feedback." + "scale; scaleY = feedback.scale }",
        ".background(feedback." + "background)",
        ".hoverable(feedback." + "interaction)",
    )

    @Test
    fun press_feedback_is_applied_through_the_shared_modifier_only() {
        // 指纹判据取"三行连在一起"：抄修饰链的人一定是连着抄的。
        // 中间插了 border / clip / semantics 的那几处不算抄——那些位置上修饰顺序真的有语义，
        // 硬塞进同一个扩展函数反而会改变绘制顺序。
        val leaks = mutableListOf<String>()
        for (path in sources()) {
            val lines = linesOf(path)
            // 定义处自己要写这三步，跳过它（由下面的反向对照测试单独看管）。
            val definition = lines.indexOfFirst { it.contains("fun Modifier.pressFeedback(") }
            val ownBody = if (definition < 0) -1 else definition + 6
            for (index in 0..lines.size - 3) {
                if (index in definition..<ownBody) continue
                // 行首的 "." 统一去掉再比：判据第一条不带点，源码行都带。
                val window = lines.subList(index, index + 3).map { it.trim().removePrefix(".") }
                if (feedbackChain.map { it.removePrefix(".") }.all { it in window }) {
                    leaks += "$path:${index + 1}"
                }
            }
        }
        assertTrue(
            leaks.isEmpty(),
            "这些地方还在连着三行手抄按压反馈链，请改用 Modifier.pressFeedback(feedback)：${leaks.joinToString("; ")}",
        )
    }

    @Test
    fun the_shared_modifier_still_applies_all_three_feedback_steps_in_order() {
        // 反向对照：抽出来的 Modifier.pressFeedback 必须真的做三件事，
        // 否则上面那条只是"没人手抄"，而反馈本身被掏空了。
        val components = sources().singleOrNull { it.endsWith("WorkbenchComponents.kt") }
            ?: error("WorkbenchComponents.kt not found among ${sources().size} sources")
        val lines = linesOf(components)
        val start = lines.indexOfFirst { it.contains("fun Modifier.pressFeedback(") }
        assertTrue(start >= 0, "Modifier.pressFeedback 不见了——按压缩放的统一入口被删了")
        // 函数体很短，多取几行容得下签名换行。
        val body = lines.drop(start).take(10).joinToString("\n")
        var cursor = 0
        for (needle in feedbackChain) {
            val at = body.indexOf(needle, cursor)
            assertTrue(at >= 0, "Modifier.pressFeedback 里少了这一步或顺序被改动：$needle")
            cursor = at + needle.length
        }
    }

    @Test
    fun every_scrollbar_wears_the_shared_style() {
        val unstyled = mutableListOf<String>()
        var total = 0
        for (path in sources()) {
            val lines = linesOf(path)
            lines.forEachIndexed { index, line ->
                if (!line.contains("VerticalScrollbar(")) return@forEachIndexed
                // `workbenchScrollbarStyle` 的定义本身不含 "VerticalScrollbar("，天然被排除。
                // 取整个调用：参数最多跨 9 行（多行 style 块最长的形态）。
                val call = lines.subList(index, minOf(index + 9, lines.size)).joinToString(" ")
                total += 1
                if (!call.contains("style = workbenchScrollbarStyle()")) unstyled += "$path:${index + 1}"
            }
        }
        assertTrue(total >= 6, "expected at least 6 VerticalScrollbar call sites, found $total")
        assertTrue(
            unstyled.isEmpty(),
            "这些滚动条没走统一样式，同一个控件在不同窗格里长得不一样：${unstyled.joinToString("; ")}",
        )
    }

    @Test
    fun line_heights_come_from_the_type_scale_only() {
        // 运行时拼接字面量；注释里提到旧写法不算违规，所以只查以这些字样开头的行。
        val literals = (16..19).map { "lineHeight = $it." + "sp" }
        val leaks = mutableListOf<String>()
        for (path in sources()) {
            linesOf(path).forEachIndexed { index, line ->
                val text = line.trim()
                if (text.startsWith("//") || text.startsWith("*") || text.startsWith("/*")) {
                    return@forEachIndexed
                }
                for (needle in literals) {
                    if (text.contains(needle)) leaks += "$path:${index + 1} ($needle)"
                }
            }
        }
        assertTrue(
            leaks.isEmpty(),
            "行高还有裸字面量，请改用 TypeScale.lineMicro / lineBody / lineTitle：${leaks.joinToString("; ")}",
        )
        assertTrue(
            linesOf("src/main/kotlin/com/hlsdownloader/desktop/WorkbenchComponents.kt")
                .any { it.contains("val lineMicro =") },
            "TypeScale 上的行高刻度被删了，上面那条规则就没有可用的替代值",
        )
    }
}
