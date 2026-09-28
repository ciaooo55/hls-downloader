package com.hlsdownloader.desktop

import java.io.File
import kotlin.test.Test
import kotlin.test.assertTrue

/**
 * 「控件自己上报窗口内边框」(Modifier.reportControlBounds) 这条能力的守卫。
 * 覆盖面：对话框、工具栏、任务行、设备行——上一轮(第十一轮)曾把工具栏整排注释成
 * 「无法全量验证」，原因就是这个能力还只接在新建对话框上。
 *
 * 为什么要有它：窄控件 / 对话框内部控件的反馈，此前的测法是**从截图的对话框分层图里估坐标**。
 * 那套办法在实践中连续失败了很多次——窗口尺寸一变、对话框内容一变坐标就全废，
 * 而且失败时的现象只是「差分 0 像素」，根本分不清是**没修好**还是**根本没测到那个东西**。
 *
 * `Modifier.reportControlBounds("名字")` 让控件自己用 `onGloballyPositioned` 上报边框，
 * `/state` 直接给出 `controlBounds`，测量脚本按名字取中心点。实测收益：
 * 新建下载对话框的 HTTP 方法分段按钮拿到真实边框 GET [361,356,601,390] /
 * POST [601,356,840,390] / HEAD [840,356,1079,390]，第一次把分段按钮量出
 * **hover 2,065 / press 2,080 像素**（此前怎么量都是 0）。
 *
 * 守卫的判据是两条：能力本身存在（UiTestState / snapshot / modifier），
 * 而且**窄目标这一类的代表调用点真的用上了**——否则能力有了却没人接，照样回到估坐标。
 */
class ControlBoundsReportingTest {
    private fun api() = File("src/main/kotlin/com/hlsdownloader/desktop/UiTestApi.kt").readText()
    private fun main() = File("src/main/kotlin/com/hlsdownloader/desktop/Main.kt").readText()

    @Test
    fun the_test_state_has_a_bounds_map_and_the_snapshot_exposes_it() {
        val api = api()
        assertTrue(api.contains("fun reportControlBounds"), "UiTestState 需要 reportControlBounds")
        assertTrue(
            api.contains("val controlBounds: Map<String, List<Int>>"),
            "UiSelectionSnapshot 必须把 controlBounds 带给 /state，否则外面读不到",
        )
        assertTrue(
            api.contains("controlBounds,"),
            "snapshot() 要把 controlBounds 传进去——只加字段不传，等于没有",
        )
    }

    @Test
    fun a_control_can_opt_in_with_a_composable_modifier() {
        val api = api()
        assertTrue(
            api.contains("internal fun Modifier.reportControlBounds("),
            "需要 Modifier.reportControlBounds，否则每个调用点都要手写 onGloballyPositioned",
        )
        assertTrue(
            api.contains("onGloballyPositioned"),
            "边框只能来自 onGloballyPositioned；用别的方式拿不到窗口内坐标",
        )
    }





    /**
     * 折叠态图标栏（`SidebarRail` / [RailItem]）也必须上报边框。
     * 源码注释里自己写了「320~360dp 下点不点得中不是肉眼能断定的，所以单击反馈不该靠目测」——
     * 那就更该用同一把尺量。1000x700 下实测：12 项全部上报边框（38~40dp 见方），
     * 10 项逐个量过，选中项 0（约定），其余 1,173 像素。
     */
    @Test
    fun the_collapsed_rail_items_report_their_bounds() {
        val main = main()
        assertTrue(
            main.contains("reportControlBounds(\"rail.status.\${item.label}\")"),
            "折叠栏的任务状态图标要上报边框",
        )
        assertTrue(
            main.contains("reportControlBounds(\"rail.category.\${item.label}\")"),
            "折叠栏的分类图标要上报边框",
        )
        assertTrue(
            main.contains("reportControlBounds(\"rail.queue.\${profile.name}\")"),
            "折叠栏的队列图标要上报边框",
        )
        assertTrue(
            main.contains("reportControlBounds(\"rail.manage\")"),
            "折叠栏底部的『管理队列』要上报边框",
        )
        assertTrue(
            main.contains("hoverColor = if (active) selectedSurface else surface3"),
            "折叠栏与展开态 NavRow 同一约定：选中态压过悬停 -> 悬停差分 0 是对的",
        )
    }


    /**
     * 「整行通宽」的一条硬约束：`pressScale = 1f` 必须同时给 `pressedColor`。
     *
     * `rememberPressFeedback` 的注释写明了原因：整行的横向缩放在视觉上像「菜单在抖」，
     * 所以这类目标用**底色档位**表达按压——而 pressScale 传 1f 意味着几何变化为 0，
     * 按压反馈**完全**由 pressedColor 承担。两者都缺 = 按住时画面一动不动（差分 0 像素）。
     *
     * 第十八轮手工核了 19 处调用点（Main/SettingsV7/WorkbenchComponents），无一违反；
     * 把这条核过的结论固化成守卫，后人把某个 1f 的 pressedColor 删掉时会直接失败。
     */
    @Test
    fun full_width_rows_with_no_scale_must_supply_a_pressed_color() {
        val roots = listOf(
            "src/main/kotlin/com/hlsdownloader/desktop/Main.kt",
            "src/main/kotlin/com/hlsdownloader/desktop/SettingsV7.kt",
            "src/main/kotlin/com/hlsdownloader/desktop/WorkbenchComponents.kt",
        )
        val bad = mutableListOf<String>()
        var total = 0
        for (path in roots) {
            val src = File(path).readText()
            src.split("rememberPressFeedback(").drop(1).forEachIndexed { index, part ->
                total += 1
                // 取到本调用配平为止，避免把下一个调用的参数算进来
                var depth = 1
                var end = 0
                for (k in part.indices) {
                    if (part[k] == '(') depth += 1
                    if (part[k] == ')') {
                        depth -= 1
                        if (depth == 0) { end = k; break }
                    }
                }
                val segment = part.substring(0, end)
                val oneToOne = Regex("pressScale\\s*=\\s*1[fF]").containsMatchIn(segment)
                if (oneToOne && !segment.contains("pressedColor")) {
                    bad += "$path 第${index + 1}处：pressScale = 1f 但没有 pressedColor，按压不可见"
                }
            }
        }
        assertTrue(total >= 15, "rememberPressFeedback 调用点只有 $total 处，守卫可能漏扫了文件")
        assertTrue(bad.isEmpty(), "这些调用点按压时画面不会变：${bad.joinToString("; ")}")
    }


    /**
     * 图标按钮与复选框必须带内容描述。
     *
     * 第十九轮补测滚动条时发现：**按行 grep 会漏掉多行调用点**（第十六轮的守卫、第二十轮的滚动条清点
     * 都栽在同一件事上）。所以这里取"整个调用片段"——`IconButton(` 的实参括号配平之后，
     * 还要把尾随 lambda `{ Icon(...) }` 一起吃进去，否则 `Icon` 根本不在片段里，会全部误报成"没有 Icon"。
     *
     * 当前状态是干净的：19 处 IconButton 全部有描述，9 处 Checkbox 全部有 accessibilityLabel。
     * 这道守卫的作用是**别退回去**——新增一个只有图标、没有描述的按钮时直接失败。
     */
    @Test
    fun icon_buttons_and_checkboxes_must_carry_a_content_description() {
        val roots = listOf(
            "src/main/kotlin/com/hlsdownloader/desktop/Main.kt",
            "src/main/kotlin/com/hlsdownloader/desktop/SettingsV7.kt",
            "src/main/kotlin/com/hlsdownloader/desktop/WorkbenchComponents.kt",
        )

        /** 取 name(...) 连同尾随 lambda 的完整片段；只看一行会漏掉多行调用点。 */
        fun fullCall(src: String, start: Int, name: String): String? {
            var depth = 0
            var end = -1
            for (k in start + name.length until src.length) {
                when (src[k]) {
                    '(' -> depth += 1
                    ')' -> {
                        depth -= 1
                        if (depth == 0) { end = k; break }
                    }
                }
            }
            if (end < 0) return null
            var k = end + 1
            while (k < src.length && (src[k] == ' ' || src[k] == '\n' || src[k] == '\t')) k += 1
            if (k < src.length && src[k] == '{') {
                var d = 0
                for (m in k until src.length) {
                    when (src[m]) {
                        '{' -> d += 1
                        '}' -> {
                            d -= 1
                            if (d == 0) return src.substring(start, m + 1)
                        }
                    }
                }
            }
            return src.substring(start, end + 1)
        }

        val bad = mutableListOf<String>()
        var iconButtons = 0
        var checkboxes = 0
        for (path in roots) {
            val src = File(path).readText()
            Regex("\\bIconButton\\(").findAll(src).forEach { m ->
                val head = src.substring(maxOf(0, m.range.first - 60), m.range.first)
                if (Regex("(internal |private )?fun\\s*$").containsMatchIn(head)) return@forEach
                val segment = fullCall(src, m.range.first, "IconButton") ?: return@forEach
                iconButtons += 1
                val icon = Regex("Icon\\(\\s*(.+?)\\s*,\\s*(.+?)\\s*[,)]", RegexOption.DOT_MATCHES_ALL).find(segment)
                if (icon == null) {
                    bad += "$path 第${src.substring(0, m.range.first).count { it == '\n' } + 1}处：IconButton 里没有 Icon(...)"
                } else if (icon.groupValues[2].trim().trimEnd(',') in setOf("null", "\"\"")) {
                    bad += "$path 第${src.substring(0, m.range.first).count { it == '\n' } + 1}处：Icon 的内容描述是 null 或空"
                }
            }
            Regex("\\bCheckbox\\(").findAll(src).forEach { m ->
                val head = src.substring(maxOf(0, m.range.first - 60), m.range.first)
                if (Regex("(internal |private )?fun\\s*$").containsMatchIn(head)) return@forEach
                val segment = fullCall(src, m.range.first, "Checkbox") ?: return@forEach
                checkboxes += 1
                if (!segment.contains("accessibilityLabel")) {
                    bad += "$path 第${src.substring(0, m.range.first).count { it == '\n' } + 1}处：Checkbox 没有 accessibilityLabel"
                }
            }
        }
        assertTrue(iconButtons >= 15, "IconButton 只数到 $iconButtons 处，守卫可能漏扫了文件")
        assertTrue(checkboxes >= 5, "Checkbox 只数到 $checkboxes 处，守卫可能漏扫了文件")
        assertTrue(bad.isEmpty(), "这些控件读屏时读不出是什么：${bad.joinToString("; ")}")
    }

    /**
     * 「选中态压过悬停态」是本产品一致的约定：**选中项的悬停差分是 0，这是对的**。
     * 三处独立实测都对上了：
     *   - 新建下载的 HTTP 方法分段：选中的 GET hover 0，未选中的 POST hover 2,065
     *   - 侧栏筛选：选中的 `全部` hover 0，其余 5 个 ~5,980
     *   - 对话框主按钮：禁用 0 → 启用 2,646（同类命题的另一面：禁用不给反馈）
     * 这条守卫的目的是防止别人"顺手修一下那个 0"，把选中态改成悬停可辨——
     * 那会让选中项看起来在动，反而更难看出当前选的是谁。
     */
    @Test
    fun selection_deliberately_beats_hover() {
        val main = main()
        // NavRow：选中时 rest 与 hover 必须是同一个颜色
        assertTrue(
            main.contains("restColor = if (active) selectedSurface else Color.Transparent"),
            "NavRow 选中态的 rest 底色是 selectedSurface",
        )
        assertTrue(
            main.contains("hoverColor = if (active) selectedSurface else surface3"),
            "NavRow 选中态的 hover 底色与 rest 相同——同一颜色 => 悬停差分 0 => 这是约定，不是漏改",
        )
        // 分段按钮同理：选中段 hover 与 rest 都落在 rail / transitively transparent
        assertTrue(
            main.contains("segmentBackground(tab == item, rail, Color.Transparent)"),
            "页签分段沿用同一约定",
        )
    }

    /**
     * 表格行 / 设备行是自己手写 interaction 的自定义行，最容易被漏掉，也最该接上。
     * 实测拿到 14 行任务、2 行设备的真实边框，于是一次扫完 16 行。
     */
    @Test
    fun custom_rows_report_their_bounds() {
        val main = main()
        assertTrue(
            main.contains("reportControlBounds(\"taskrow.\${task.id}\")"),
            "TaskRow 要上报边框——手写 interaction 的行是全产品最高频的交互，反馈必须可测",
        )
        assertTrue(
            main.contains("reportControlBounds(\"device.\${device.id}\")"),
            "设备行要上报边框——设备选择是靠悬停认出来的，不能只是手写 interaction",
        )
    }

    /**
     * 共用组件必须上报边框，否则「全量按钮反馈清单」要退回估坐标。
     * 现在按名字就能扫：`/state` 的 controlBounds 给出 toolbar.* / dialog.*，
     * 于是 12 个工具栏按钮、3 个对话框按钮的悬停反馈一次性全量量完。
     */
    @Test
    fun shared_button_components_report_their_bounds() {
        val main = main()
        assertTrue(
            main.contains("reportControlBounds(\"dialog.primary.\$label\")"),
            "DialogPrimary 要上报边框——对话框主按钮是反馈链上最关键的一个",
        )
        assertTrue(
            main.contains("reportControlBounds(\"dialog.secondary.\$label\")"),
            "DialogSecondary 要上报边框",
        )
        assertTrue(
            main.contains("reportControlBounds(\"toolbar.\$label\")"),
            "ToolbarButton 要上报边框",
        )
        assertTrue(
            main.contains("reportControlBounds(\"toolbar.\$text\")"),
            "ToolbarIcon 要上报边框——窄尺寸下工具栏整排都渲染成图标，没有它就没有反馈清单",
        )
    }

    @Test
    fun narrow_targets_actually_report_their_bounds() {
        val main = main()
        // 分段按钮只有约 34dp 高，投影仪/截图估坐标最难对上的就是它们
        assertTrue(
            main.contains("reportControlBounds(\"newtask.method.\$value\")"),
            "HTTP 方法分段按钮要上报边框，否则这一类的反馈仍要靠估坐标",
        )
        // 页签也是「不切过去就看不见」的目标：方法行藏在『请求』页签后面，
        // 之前几次量不到就是因为没人知道要先点哪个页签、点在哪里。
        assertTrue(
            main.contains("reportControlBounds(\"newtask.tab.\$item\")"),
            "新建下载的页签要各自上报边框——方法行藏在『请求』页签后面，没有它就只能猜",
        )
    }
}
