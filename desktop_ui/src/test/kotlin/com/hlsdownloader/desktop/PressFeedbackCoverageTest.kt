package com.hlsdownloader.desktop

import java.io.File
import kotlin.test.Test
import kotlin.test.assertTrue

/**
 * 「按下态必须留下痕迹」这一类缺陷的守卫。
 *
 * 第三十七轮用新加的 `press` 动作量出两处真缺陷：任务行和表头都自己手写 interaction
 * （TaskRow 要同时吃 hover / 双击 / 右键 / 拖动，所以绕过了 rememberPressFeedback），
 * 都只收了 hover 态——按住与悬停态像素级相同（差 0）。两处都是"先量到 0、修完再量到非 0"。
 *
 * 这里把判据从像素下沉到源码结构：**每一个 clickable 家族调用点，它的 interactionSource
 * 必须最终喂给一个会读按下态的对象**。三类合法来源：
 *  1. `feedback.interaction` —— rememberPressFeedback 的产物（绝大多数面）；
 *  2. `interaction` —— WorkbenchButton 由调用方注入的共享源，函数体内同样喂给了 rememberPressFeedback；
 *  3. 自己 `remember { MutableInteractionSource() }` —— 只允许是弹窗遮罩（点它只为关闭，
 *     没有可见反馈是设计如此），因此额外要求同一调用里出现 `onClick = onDismiss`。
 * 将来谁再手写一个既不读按下态、也不是遮罩的可点面，这条会直接失败。
 */
class PressFeedbackCoverageTest {
    private fun sources(): List<String> = listOf(
        "src/main/kotlin/com/hlsdownloader/desktop/Main.kt",
        "src/main/kotlin/com/hlsdownloader/desktop/WorkbenchComponents.kt",
        "src/main/kotlin/com/hlsdownloader/desktop/SettingsV7.kt",
    )

    private fun callSites(): List<Triple<String, Int, String>> {
        val out = mutableListOf<Triple<String, Int, String>>()
        for (path in sources()) {
            val lines = File(path).readText().split("\n")
            lines.forEachIndexed { index, line ->
                if (!line.contains(".clickable(") && !line.contains(".combinedClickable(") &&
                    !line.contains(".selectable(") && !line.contains(".toggleable(")
                ) {
                    return@forEachIndexed
                }
                // 取到这一调用的第一个右括号为止：参数可能换行，但不会换出 14 行。
                val segment = StringBuilder(line)
                var cursor = index + 1
                while (cursor < lines.size && !segment.contains(")")) {
                    segment.append(' ').append(lines[cursor].trim())
                    cursor += 1
                    if (cursor - index > 14) break
                }
                // 调用之后再带上两行：遮罩的 clearAndSetSemantics 就挂在 clickable 的下一行，
                // 只看调用本身会漏掉它。
                val tail = lines.drop(index + 1).take(2).joinToString(" ")
                out += Triple(path, index + 1, segment.toString() + " " + tail)
            }
        }
        return out
    }

    @Test
    fun every_clickable_surface_feeds_an_interaction_source_that_reads_the_pressed_state() {
        val sites = callSites()
        assertTrue(sites.size >= 15, "expected at least 15 clickable-family call sites, found ${sites.size}")
        val violations = mutableListOf<String>()
        for ((path, line, segment) in sites) {
            val source = Regex("interactionSource\\s*=\\s*([A-Za-z0-9_.{}() ]+?)[,)]")
                .find(segment)?.groupValues?.get(1)?.trim()
            if (source == null) {
                violations += "$path:$line (no interactionSource argument)"
                continue
            }
            when {
                source == "feedback.interaction" -> Unit          // rememberPressFeedback 的产物
                source == "interaction" -> Unit                    // WorkbenchButton 由调用方注入，函数体内仍喂给 rememberPressFeedback
                source.startsWith("remember") -> {
                    // 只许是遮罩：自己造一个源、只为点它关闭、且对无障碍不可见。
                    if (!segment.contains("onClick = onDismiss")) violations += "$path:$line ($source)"
                }
                else -> {
                    // 自定义源（TaskRow 的 rowInteraction 这类）：要求同一个函数里
                    // **从这个源上读出了按下态**，否则按住它不会有任何反馈。
                    val text = File(path).readText()
                    val before = text.split("\n").take(line)
                    val window = before.takeLast(70).joinToString("\n")
                    if (!window.contains("$source.collectIsPressedAsState()")) {
                        violations += "$path:$line ($source 上没有读到按下态)"
                    }
                }
            }
        }
        assertTrue(
            violations.isEmpty(),
            "这些可点面按住时不会有任何反馈：${violations.joinToString("; ")}",
        )
        // 负向对照：遮罩那处必须仍然带着 onClick = onDismiss，否则上面的豁免就不再成立。
        val scrim = sites.firstOrNull { it.third.contains("onClick = onDismiss") }
        assertTrue(scrim != null, "dialog scrim call site not found; the exemption above may be stale")
        assertTrue(scrim!!.third.contains("clearAndSetSemantics"), "scrim must stay invisible to accessibility")
    }
}
