package com.hlsdownloader.desktop

import java.io.File
import kotlin.test.Test
import kotlin.test.assertTrue

/**
 * 「透明底的按钮没有按下/悬停反馈」这一类缺陷的守卫。
 *
 * 第三十九轮用 `press` 动作量出来：`Color.Transparent` 当容器色时，
 * `container.blendToward(Color.White, .08f)` 是**从无到有的 8%**，
 * 叠在深色底上每个通道只动一两个数，量出来 hover / press 与静止态差 **0 像素**。
 * 现象是所有"文字按钮"——`TextButton`（DialogSecondary 一路）、以及非 primary 的
 * `ToolbarButton`——鼠标悬停和按住时**完全没有任何反馈**，而同屏其它控件应有尽有。
 *
 * 修法（沿用 [IconButton] 的现成约定）：给这类调用显式一档可见底色
 * `hoverColor = surface3` + `pressedColor = pressedSurface`（令牌即 `surface3.blendToward(ink, .05f)`，见 Main.kt 顶部）。
 * [Button] / [TextButton] 因此各新增两个可空覆盖参数，不传时行为与从前完全一致。
 *
 * 一处重要的**反向约束**：`TextButton` 自己不默认给 hoverColor。分段控件（GET/POST、
 * 详情页 tab）复用它，底色由调用方 `.background(segmentBackground(...))` 提供；
 * 若在 TextButton 里默认铺一档 surface3，悬停时会盖掉选中段的颜色——选中态反被抹掉。
 * 所以判据是：**透明容器要么显式给出 hoverColor，要么自己用 modifier 提供了可见底色**，
 * 但"自己提供底色"只豁免悬停档位，**不豁免按压**：分段按钮还必须给出比 .985f 更大的
 * `pressScale`——窄目标约 24dp 高，1.5% 不足一个像素，同样量出差分 0。
 * 选中段固定把选中色当 hover/press 色传进去（等于不变色），未选中段才给 surface3 档，
 * 这样"选中态压过悬停态"的既有约定才成立。
 */
class TransparentButtonFeedbackTest {
    private val roots = listOf(
        "src/main/kotlin/com/hlsdownloader/desktop/WorkbenchComponents.kt",
        "src/main/kotlin/com/hlsdownloader/desktop/Main.kt",
        "src/main/kotlin/com/hlsdownloader/desktop/SettingsV7.kt",
    )

    /**
     * 取到该调用的完整片段，而不是单行。
     *
     * 第十六轮才发现这条守卫有个**结构性的漏洞**：原先只取单行，
     * 于是所有多行书写（`TextButton(` 单独一行、参数换行）的调用点被整段跳过，
     * 而下一轮在 SettingsV7.kt 里就是因此漏掉了两个分段控件——
     * `V7Choice` 的选项芯片（容器透明、无 hoverColor、无 pressScale）和星期芯片（无 pressScale）。
     * 这是本守卫本该抓的那一类缺陷，却被格式化方式藏住了。
     */
    private fun callSites(path: String): List<Pair<Int, String>> {
        val lines = File(path).readText().split("\n")
        val out = mutableListOf<Pair<Int, String>>()
        lines.forEachIndexed { index, line ->
            val isButtonCall = line.contains("Button(") && !line.contains("fun ")
            if (!isButtonCall) return@forEachIndexed
            val segment = StringBuilder(line)
            var cursor = index + 1
            while (cursor < lines.size && (segment.count { it == '(' } > segment.count { it == ')' })) {
                segment.append(' ').append(lines[cursor].trim())
                cursor += 1
                if (cursor - index > 40) break
            }
            out += (index + 1) to segment.toString()
        }
        return out
    }

    @Test
    fun every_button_with_a_transparent_container_has_a_visible_hover_step() {
        val voice = mutableListOf<String>()
        for (path in roots) {
            for ((line, segment) in callSites(path)) {
                if (!segment.contains("Color.Transparent")) continue
                val hasExplicitStep = segment.contains("hoverColor")
                val suppliesOwnBackground = segment.contains(".background(")
                if (!hasExplicitStep && !suppliesOwnBackground) {
                    voice += "$path:$line"
                }
            }
        }
        assertTrue(
            voice.isEmpty(),
            "这些按钮容器色透明、又没有自己提供底色，悬停档位不可见（量出来 hover/press 差 0 像素）：" +
                voice.joinToString("; "),
        )
    }

    @Test
    fun segment_buttons_give_both_a_hover_step_and_a_visible_press_scale() {
        // 第十六轮：这条原先只扫 Main.kt，于是 SettingsV7.kt 的两个分段控件
        // （V7Choice 的选项芯片 + 星期芯片）**只有自己提供底色这一项被检查过**，
        // 按压档位压根没人看——而它们全都没有 pressScale（默认 .985f 在 24dp 上不足一像素，
        // 量出来 press 差分就是 0）。现在三个源文件全扫。
        val bad = mutableListOf<String>()
        var total = 0
        for (path in roots) {
            val src = File(path).readText()
            // 只认真正的调用点：必须同时有 onClick。
            // （我加这条时先踩了一次——WorkbenchComponents.kt 里 TextButton 的**实现注释**
            //   同时含 "TextButton(" 和 "segmentBackground("，被当成一个调用点报了出来。）
            // 用 `split("TextButton(")` 取**调用点参数**（这样 head 一定从 onClick 开始，
            // 不会把外层 Row/forEach 的 ") {" 卷进来——按行累加会犯这个错，
            // 单行书写的整条 Row 里第一个 ") {" 出现在修饰链早段，把 hoverColor 全截没了）。
            // 唯一要排掉的是 TextButton 自己的函数声明：它以命名形参 `onClick:` 开头，
            // 而函数体里又有转发用的 `onClick = onClick`，注释里还写着 segmentBackground(...)。
            val segments = src.split("TextButton(").drop(1).filter {
                it.contains("segmentBackground(") && !it.trimStart().startsWith("onClick:")
            }
            total += segments.size
            for ((index, seg) in segments.withIndex()) {
                val head = seg.substringBefore(") {")
                if (!head.contains("hoverColor")) bad += "$path 分段#${index + 1} 缺 hoverColor"
                if (!head.contains("pressScale")) bad += "$path 分段#${index + 1} 缺 pressScale"
                if (!head.contains("SEGMENT_SCALE")) bad += "$path 分段#${index + 1} 未用 SEGMENT_SCALE"
                if (!seg.contains("selectedSurface") && !seg.contains("rail")) {
                    bad += "$path 分段#${index + 1} 的选中分支要把选中色当 hover/press 色，否则选中态会被悬停档盖掉"
                }
            }
        }
        assertTrue(total > 0, "没有找到分段按钮调用点，守卫可能已经失效")
        assertTrue(bad.isEmpty(), "分段按钮的反馈仍然不可见：${bad.joinToString("；")}")
    }

    @Test
    fun text_button_itself_stays_transparent_by_default() {
        val src = File("src/main/kotlin/com/hlsdownloader/desktop/WorkbenchComponents.kt").readText()
        val body = src.substringAfter("internal fun TextButton(").substringBefore("internal fun IconButton(")
        assertTrue(
            body.contains("hoverColor = hoverColor,"),
            "TextButton 只是转发覆盖参数，绝不能自己默认铺一档底色：分段控件靠调用方的 " +
                "segmentBackground 提供选中色，默认铺底会在悬停时把选中态盖掉",
        )
        assertTrue(
            body.contains("colors = ButtonDefaults.buttonColors(Color.Transparent"),
            "TextButton 的默认底色仍然必须是透明，改了这个就动了所有文字按钮的观感",
        )
    }

    @Test
    fun text_button_passes_its_hover_and_press_step() {
        val src = File("src/main/kotlin/com/hlsdownloader/desktop/Main.kt").readText()
        val body = src.substringAfter("internal fun DialogSecondary(").substringBefore("\n")
        assertTrue(
            body.contains("hoverColor = surface3"),
            "DialogSecondary 必须显式给出可见的悬停档位（透明底 + 默认 blendToward 量出来是 0 像素）",
        )
        assertTrue(
            body.contains("pressedColor = pressedSurface"),
            "DialogSecondary 还必须给一档按压底色（统一走 pressedSurface 令牌），否则按住只缩 1.5%（窄目标几乎看不出来）",
        )
        // 令牌本身必须仍是一档"朝墨色推 5%"的可见底色——这一条是上一行的后盾：
        // 若只钉 pressedColor = pressedSurface，把令牌改成 Color.Transparent 也能整条绿过去，
        // 而那正是这个测试要防的事。DialogSecondary 是单行写法，取的 body 里没有令牌定义，所以去 src 上取。
        val token = src.substringAfter("val pressedSurface: Color").substringBefore("\n")
        assertTrue(
            token.contains("surface3.blendToward(ink, .05f)"),
            "pressedSurface 必须继续由 surface3 朝墨色推 5% 派生，不能改成透明或实色",
        )
    }

    @Test
    fun button_keeps_its_old_behaviour_when_no_override_is_passed() {
        val src = File("src/main/kotlin/com/hlsdownloader/desktop/WorkbenchComponents.kt").readText()
        val body = src.substringAfter("internal fun Button(").substringBefore("internal fun TextButton(")
        assertTrue(
            body.contains("hoverColor ?: container.blendToward(Color.White, .08f)"),
            "不显式传 hoverColor 时必须沿用原来的\"朝白推 8%\"，否则改了所有按钮的悬停表现",
        )
        assertTrue(
            body.contains("pressScale ?: .985f") &&
            body.contains("pressedColor ?: Color.Unspecified"),
            "pressedColor 的可空哨兵必须是 Color.Unspecified——rememberPressFeedback 用它判断按压档位是否存在",
        )
    }
}
