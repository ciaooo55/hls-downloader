package com.hlsdownloader.desktop

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.MutableTransitionState
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.hoverable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsHoveredAsState
import androidx.compose.foundation.interaction.collectIsFocusedAsState
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicText
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ColorFilter
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.TransformOrigin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.ProgressBarRangeInfo
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.progressBarRangeInfo
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.setProgress
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.IntRect
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Popup
import androidx.compose.ui.window.PopupPositionProvider
import androidx.compose.ui.window.PopupProperties
import kotlinx.coroutines.delay

private val LocalControlContentColor = staticCompositionLocalOf { Color.Unspecified }
private val desktopFont = FontFamily.SansSerif

// 工具提示是刻意的"深色浮标"样式（浅色主题下也是深底白字），不跟随主题。
private val tooltipSurface = Color(0xEE20242A)

/**
 * 排版刻度。
 *
 * 此前界面里散落着 9 级字号（9/10/11/12/13/14/15/16/17sp），其中 10sp 与 11sp 合计占 70%，
 * 在 100% DPI 下中文明显偏小。收敛为 5 级语义刻度：只按"这段文字是什么角色"取值，
 * 不再允许"随手写一个数"。
 */
internal object TypeScale {
    /** 辅助提示、元信息、表头 —— 允许的最小级别 */
    val micro = 11.sp

    /** 次级标签、徽标、计数 */
    val caption = 12.sp

    /** 正文与列表主文本 */
    val body = 13.sp

    /** 区块标题、弹窗副标题 */
    val title = 15.sp

    /** 弹窗主标题、大号数值 */
    val display = 17.sp
}

/**
 * 圆角刻度。原来有 10 种取值（2/3/4/5/6/7/8/9/10/12dp），
 * 其中 7dp 占 2/3 已是事实标准，这里把它定为默认值，其余归位。
 */
internal object Radius {
    /** 细进度条、骨架块、柱状图 —— 视觉上等同胶囊 */
    val tiny = 3.dp

    /** 小控件：复选框、分段按钮、工具提示、细进度条 */
    val sm = 6.dp

    /** 默认：按钮、输入框、列表项、面板、卡片、菜单 */
    val md = 7.dp

    /** 大容器：弹窗外壳、拖放浮层 */
    val lg = 10.dp

    /** 百分比圆角，用于胶囊与圆形（`RoundedCornerShape(percent)` 重载） */
    const val pill = 50
}

/**
 * 阴影刻度。原来有 6 种取值（1/2/4/6/8/10/14/16dp），收敛为三级。
 */
internal object Elevation {
    /** 贴边细分隔：滑块拇指等 */
    val e1 = 2.dp

    /** 浮层：菜单、工具提示、悬浮卡片 */
    val e2 = 6.dp

    /** 弹窗与全屏浮层 */
    val e3 = 12.dp
}

@Composable
internal fun Text(
    text: String,
    modifier: Modifier = Modifier,
    color: Color = Color.Unspecified,
    fontSize: TextUnit = TypeScale.body,
    fontWeight: FontWeight? = null,
    maxLines: Int = Int.MAX_VALUE,
    overflow: TextOverflow = TextOverflow.Clip,
    lineHeight: TextUnit = TextUnit.Unspecified,
) {
    val inherited = LocalControlContentColor.current
    BasicText(
        text = text,
        modifier = modifier,
        style = TextStyle(
            color = when {
                color != Color.Unspecified -> color
                inherited != Color.Unspecified -> inherited
                else -> ink
            },
            fontSize = fontSize,
            fontWeight = fontWeight,
            fontFamily = desktopFont,
            lineHeight = lineHeight,
            letterSpacing = 0.sp,
        ),
        maxLines = maxLines,
        overflow = overflow,
    )
}

@Composable
internal fun Icon(
    imageVector: ImageVector,
    contentDescription: String?,
    modifier: Modifier = Modifier,
    tint: Color = Color.Unspecified,
) {
    val inherited = LocalControlContentColor.current
    val resolved = when {
        tint != Color.Unspecified -> tint
        inherited != Color.Unspecified -> inherited
        else -> ink
    }
    Image(
        imageVector = imageVector,
        contentDescription = contentDescription,
        modifier = modifier,
        colorFilter = ColorFilter.tint(resolved),
    )
}

@Composable
internal fun Surface(
    modifier: Modifier = Modifier,
    shape: Shape = RoundedCornerShape(0.dp),
    color: Color = Color.Transparent,
    contentColor: Color = Color.Unspecified,
    shadowElevation: Dp = 0.dp,
    tonalElevation: Dp = 0.dp,
    border: BorderStroke? = null,
    content: @Composable BoxScope.() -> Unit,
) {
    Box(
        modifier
            .then(if (shadowElevation > 0.dp) Modifier.shadow(shadowElevation, shape, clip = false) else Modifier)
            .clip(shape)
            .background(color)
            .then(if (border != null) Modifier.border(border, shape) else Modifier),
    ) {
        CompositionLocalProvider(LocalControlContentColor provides contentColor, content = { content() })
    }
}

/**
 * 列表项 / 卡片 / 导航行的悬停与按压反馈。
 *
 * 为什么不复用 [Button]：这些目标的布局差异太大（整行、整卡、带角标、带左侧色条），
 * 包装组件会把差异全部吞掉。这里只统一「悬停换底色、按压缩一点」这一层；
 * 点击语义（clickable / selectable / toggleable）仍由调用方自己挂，因为 Role 各不相同。
 *
 * 全部时长走 [motionDurationMillis]：reduce_motion 打开时立即到位。
 *
 * 注意 [hoverColor] 的选取有对比度约束：选中态底色**不要**在悬停时再往强调色方向推。
 * 深色主题的 `blue` 是浅蓝、浅色主题是深蓝，把 `selectedSurface` 往 `blue` 混合会同时
 * 压低两种主题下"强调色文字压选中底"的比值（实测 4.51 → 4.18，跌破 4.5:1）。
 * 选中项一律沿用 TaskRow 的既有约定——选中态压过悬停态，只保留按压缩放作为反馈。
 */
@Composable
internal fun rememberPressFeedback(
    restColor: Color = Color.Transparent,
    hoverColor: Color = surface3,
    pressedColor: Color = Color.Unspecified,
    enabled: Boolean = true,
    pressScale: Float = .985f,
    hoverMillis: Int = 130,
    pressMillis: Int = 90,
): PressFeedback {
    val interaction = remember { MutableInteractionSource() }
    val hovered by interaction.collectIsHoveredAsState()
    val pressed by interaction.collectIsPressedAsState()
    val reduceMotion by MotionPreferences.reduceMotion
    val background by animateColorAsState(
        targetValue = when {
            !enabled -> restColor
            // 整行通宽的横向缩放在视觉上像"菜单在抖"，所以这类目标用底色档位表达按压，
            // 此时 pressScale 传 1f，按压反馈完全由 pressedColor 承担。
            pressed && pressedColor != Color.Unspecified -> pressedColor
            hovered -> hoverColor
            else -> restColor
        },
        animationSpec = tween(motionDurationMillis(reduceMotion, hoverMillis)),
        label = "press-feedback-background",
    )
    val scale by animateFloatAsState(
        if (pressed && enabled) pressScale else 1f,
        animationSpec = tween(motionDurationMillis(reduceMotion, pressMillis)),
        label = "press-feedback-scale",
    )
    return PressFeedback(interaction, hovered, pressed, background, scale)
}

internal class PressFeedback(
    val interaction: MutableInteractionSource,
    val hovered: Boolean,
    val pressed: Boolean,
    val background: Color,
    val scale: Float,
)

internal data class ButtonColors(
    val container: Color,
    val content: Color,
    val disabledContainer: Color,
    val disabledContent: Color,
)

internal object ButtonDefaults {
    fun buttonColors(
        containerColor: Color = Color.Transparent,
        contentColor: Color = Color.Unspecified,
        disabledContainerColor: Color = containerColor.copy(alpha = .45f),
        disabledContentColor: Color = contentColor.copy(alpha = .45f),
    ) = ButtonColors(containerColor, contentColor, disabledContainerColor, disabledContentColor)
}

@Composable
internal fun Button(
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
    shape: Shape = RoundedCornerShape(Radius.md),
    border: BorderStroke? = null,
    contentPadding: PaddingValues = PaddingValues(horizontal = 12.dp, vertical = 8.dp),
    colors: ButtonColors = ButtonDefaults.buttonColors(blue, onBlue),
    content: @Composable RowScope.() -> Unit,
) {
    val interaction = remember { MutableInteractionSource() }
    val focused by interaction.collectIsFocusedAsState()
    val container = if (enabled) colors.container else colors.disabledContainer
    val contentColor = if (enabled) colors.content else colors.disabledContent
    val feedback = rememberPressFeedback(
        restColor = container,
        hoverColor = container.blendToward(Color.White, .08f),
        enabled = enabled,
        pressScale = .985f,
        hoverMillis = 140,
    )
    Row(
        modifier
            .defaultMinSize(minHeight = 34.dp)
            .clip(shape)
            .graphicsLayer { scaleX = feedback.scale; scaleY = feedback.scale }
            .background(feedback.background)
            .then(if (focused) Modifier.border(2.dp, blue, shape) else if (border != null) Modifier.border(border, shape) else Modifier)
            .hoverable(interaction)
            .clickable(interactionSource = interaction, indication = null, enabled = enabled, role = Role.Button, onClick = onClick)
            .padding(contentPadding),
        horizontalArrangement = Arrangement.Center,
        verticalAlignment = Alignment.CenterVertically,
    ) {
        CompositionLocalProvider(LocalControlContentColor provides contentColor) { content() }
    }
}

@Composable
internal fun TextButton(
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
    contentPadding: PaddingValues = PaddingValues(horizontal = 10.dp, vertical = 7.dp),
    content: @Composable RowScope.() -> Unit,
) = Button(
    onClick = onClick,
    modifier = modifier,
    enabled = enabled,
    shape = RoundedCornerShape(Radius.sm),
    contentPadding = contentPadding,
    colors = ButtonDefaults.buttonColors(Color.Transparent, if (enabled) blue else faint),
    content = content,
)

@Composable
internal fun IconButton(
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
    content: @Composable BoxScope.() -> Unit,
) {
    val interaction = remember { MutableInteractionSource() }
    val feedback = rememberPressFeedback(
        hoverColor = surface3,
        enabled = enabled,
        pressScale = .9f,
        hoverMillis = 120,
    )
    Box(
        modifier
            .defaultMinSize(34.dp, 34.dp)
            .clip(RoundedCornerShape(Radius.sm))
            .graphicsLayer { scaleX = feedback.scale; scaleY = feedback.scale }
            .background(feedback.background)
            .hoverable(interaction)
            .clickable(interactionSource = interaction, indication = null, enabled = enabled, role = Role.Button, onClick = onClick)
            .alpha(if (enabled) 1f else .42f),
        contentAlignment = Alignment.Center,
        content = content,
    )
}

@Composable
internal fun Checkbox(checked: Boolean, onCheckedChange: (Boolean) -> Unit, modifier: Modifier = Modifier, accessibilityLabel: String? = null) {
    // 对勾画在 blue 填充上，必须用 onBlue：深色主题的 blue 是浅蓝，白勾只有 2.65:1，
    // 连 3:1 的图形底线都不到（勾是承担"已选中"信息的图形，不是装饰）。
    val checkColor = onBlue
    val shape = RoundedCornerShape(Radius.sm)
    val reduceMotion by MotionPreferences.reduceMotion
    // 悬停给一层灰底（surface3），不把边框染成 blue —— 后者会被读成"已选中"。
    val feedback = rememberPressFeedback(
        restColor = if (checked) blue else rail,
        hoverColor = if (checked) blue.blendToward(Color.White, .08f) else surface3,
        pressScale = .9f,
    )
    val animatedBorder by animateColorAsState(
        if (checked) blue else border,
        animationSpec = tween(motionDurationMillis(reduceMotion, 130)),
        label = "checkbox-border",
    )
    // 对勾的描画进度 0→1：前半段先画到 45%，后半段接着画完。
    // 存在条件是进度 > 0 而不是 `checked`，否则取消勾选时对勾会瞬间消失、看不见回缩。
    val checkProgress by animateFloatAsState(
        if (checked) 1f else 0f,
        animationSpec = tween(motionDurationMillis(reduceMotion, 170)),
        label = "checkbox-check",
    )
    Box(
        modifier
            .defaultMinSize(18.dp, 18.dp)
            .graphicsLayer { scaleX = feedback.scale; scaleY = feedback.scale }
            .clip(shape)
            .background(feedback.background)
            .border(1.dp, animatedBorder, shape)
            .semantics {
                stateDescription = if (checked) "已选中" else "未选中"
                if (accessibilityLabel != null) contentDescription = accessibilityLabel
            }
            .hoverable(feedback.interaction)
            .toggleable(
                value = checked,
                interactionSource = feedback.interaction,
                indication = null,
                role = Role.Checkbox,
                onValueChange = onCheckedChange,
            ),
        contentAlignment = Alignment.Center,
    ) {
        if (checkProgress > 0f) androidx.compose.foundation.Canvas(Modifier.size(12.dp)) {
            val stroke = 1.8.dp.toPx()
            // 两段都沿原方向截短，角度不变，所以"描画中"和"画完"是同一条折线。
            val first = Offset(size.width * .18f, size.height * .52f)
            val elbow = Offset(size.width * .42f, size.height * .76f)
            val last = Offset(size.width * .84f, size.height * .25f)
            val firstFraction = (checkProgress / .45f).coerceIn(0f, 1f)
            val secondFraction = ((checkProgress - .45f) / .55f).coerceIn(0f, 1f)
            if (firstFraction > 0f) drawLine(checkColor, first, first + (elbow - first) * firstFraction, stroke)
            if (secondFraction > 0f) drawLine(checkColor, elbow, elbow + (last - elbow) * secondFraction, stroke)
        }
    }
}

@Composable
internal fun RadioButton(selected: Boolean, onClick: () -> Unit, modifier: Modifier = Modifier, accessibilityLabel: String? = null) {
    val reduceMotion by MotionPreferences.reduceMotion
    val animatedFill by animateColorAsState(
        if (selected) selectedSurface else rail,
        animationSpec = tween(motionDurationMillis(reduceMotion, 150)),
        label = "radio-fill",
    )
    Box(
        modifier
            .defaultMinSize(18.dp, 18.dp)
            .clip(CircleShape)
            .background(animatedFill)
            .border(1.dp, if (selected) blue else border, CircleShape)
            .semantics {
                stateDescription = if (selected) "已选择" else "未选择"
                if (accessibilityLabel != null) contentDescription = accessibilityLabel
            }
            .selectable(selected = selected, role = Role.RadioButton, onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        if (selected) Box(Modifier.size(9.dp).clip(CircleShape).background(blue))
    }
}

@Composable
internal fun Switch(checked: Boolean, onCheckedChange: (Boolean) -> Unit, modifier: Modifier = Modifier, accessibilityLabel: String? = null) {
    val reduceMotion by MotionPreferences.reduceMotion
    // 悬停方向必须"背离底色"：浅色主题的 ink 是深色、深色主题是浅色，
    // 所以 `blendToward(ink, …)` 在两个主题下都是"更远离轨道底色"的方向，不需要为 hover 另立令牌。
    val feedback = rememberPressFeedback(
        restColor = if (checked) blue else surface3,
        hoverColor = if (checked) blue.blendToward(Color.White, .08f) else surface3.blendToward(ink, .08f),
        pressScale = .96f,
        hoverMillis = 160,
    )
    val knobFraction by animateFloatAsState(
        if (checked) 1f else 0f,
        animationSpec = tween(motionDurationMillis(reduceMotion, 160)),
        label = "switch-position",
    )
    val animatedBorder by animateColorAsState(
        if (checked) blue else border,
        animationSpec = tween(motionDurationMillis(reduceMotion, 160)),
        label = "switch-border",
    )
    Box(
        modifier
            .width(36.dp)
            .height(20.dp)
            .graphicsLayer { scaleX = feedback.scale; scaleY = feedback.scale }
            .clip(CircleShape)
            .background(feedback.background)
            .border(1.dp, animatedBorder, CircleShape)
            .semantics {
                stateDescription = if (checked) "已开启" else "已关闭"
                if (accessibilityLabel != null) contentDescription = accessibilityLabel
            }
            .hoverable(feedback.interaction)
            .toggleable(
                value = checked,
                interactionSource = feedback.interaction,
                indication = null,
                role = Role.Switch,
                onValueChange = onCheckedChange,
            )
            .padding(2.dp),
    ) {
        Box(
            Modifier
                .align(Alignment.CenterStart)
                .offset(x = 16.dp * knobFraction)
                .size(14.dp)
                .clip(CircleShape)
                // 拇指一律取"与轨道相反的亮度"：
                //   浅色 未选中（浅灰轨）→ faint 灰拇指 4.03:1；选中（深蓝轨）→ 白拇指 5.17:1
                //   深色 未选中（深灰轨）→ faint 灰拇指 4.81:1；选中（浅蓝轨）→ onBlue 近黑拇指 6.76:1
                // 原先写死 Color.White：深色选中态只有 2.65:1（和 Checkbox 的对勾同一个坑），
                // 浅色未选中态更是 1.18:1 —— 只靠 e1 阴影分辨拇指边界。两处都不足 3:1。
                .background(if (checked) onBlue else faint)
                .shadow(Elevation.e1, CircleShape),
        )
    }
}

@Composable
internal fun LinearProgressIndicator(
    progress: () -> Float,
    modifier: Modifier = Modifier,
    color: Color = blue,
    trackColor: Color = surface3,
) {
    val reduceMotion by MotionPreferences.reduceMotion
    val value by animateFloatAsState(
        progress().coerceIn(0f, 1f),
        animationSpec = tween(motionDurationMillis(reduceMotion, 180)),
        label = "progress-value",
    )
    Box(modifier.semantics { progressBarRangeInfo = ProgressBarRangeInfo(value, 0f..1f) }.background(trackColor)) {
        Box(Modifier.fillMaxHeight().fillMaxWidth(value).background(color))
    }
}

@Composable
internal fun CircularProgressIndicator(
    modifier: Modifier = Modifier,
    strokeWidth: Dp = 2.dp,
    color: Color = blue,
) {
    val reduceMotion by MotionPreferences.reduceMotion
    if (reduceMotion) {
        androidx.compose.foundation.Canvas(modifier) {
            drawArc(color, -90f, 270f, false, style = Stroke(strokeWidth.toPx()))
        }
    } else {
        val rotation = rememberInfiniteTransition(label = "spinner").animateFloat(
            initialValue = 0f,
            targetValue = 360f,
            animationSpec = infiniteRepeatable(tween(1_100, easing = LinearEasing)),
            label = "spinner-rotation",
        )
        androidx.compose.foundation.Canvas(modifier.graphicsLayer { rotationZ = rotation.value }) {
            drawArc(color, -90f, 270f, false, style = Stroke(strokeWidth.toPx()))
        }
    }
}

internal data class SliderColors(
    val thumb: Color,
    val activeTrack: Color,
    val inactiveTrack: Color,
)

internal object SliderDefaults {
    @Composable
    fun colors(
        thumbColor: Color = blue,
        activeTrackColor: Color = blue,
        inactiveTrackColor: Color = surface3,
        activeTickColor: Color = activeTrackColor,
        inactiveTickColor: Color = inactiveTrackColor,
    ) = SliderColors(thumbColor, activeTrackColor, inactiveTrackColor)
}

@Composable
internal fun Slider(
    value: Float,
    onValueChange: (Float) -> Unit,
    modifier: Modifier = Modifier,
    onValueChangeFinished: () -> Unit = {},
    valueRange: ClosedFloatingPointRange<Float> = 0f..1f,
    colors: SliderColors = SliderDefaults.colors(),
    accessibilityLabel: String? = null,
) {
    val span = (valueRange.endInclusive - valueRange.start).takeIf { it > 0f } ?: 1f
    val fraction = ((value - valueRange.start) / span).coerceIn(0f, 1f)
    val interaction = remember { MutableInteractionSource() }
    val hovered by interaction.collectIsHoveredAsState()
    var pressing by remember { mutableStateOf(false) }
    val reduceMotion by MotionPreferences.reduceMotion
    // 拇指的"按下放大"必须挂在按压状态上而不是 hover 上：拖拽时指针常常滑出滑块边界，
    // 用 hover 判定会让拇指在拖拽中途缩回去。
    val thumbScale by animateFloatAsState(
        when {
            pressing -> 1.25f
            hovered -> 1.12f
            else -> 1f
        },
        animationSpec = tween(motionDurationMillis(reduceMotion, 110)),
        label = "slider-thumb",
    )
    // 轨道加粗同样只改绘制高度：父级高度由 14dp 的拇指决定，所以不会引起布局跳动。
    val trackHeight by animateDpAsState(
        if (hovered || pressing) 6.dp else 4.dp,
        animationSpec = tween(motionDurationMillis(reduceMotion, 110)),
        label = "slider-track",
    )
    BoxWithConstraints(
        modifier
            .hoverable(interaction)
            .semantics {
                if (accessibilityLabel != null) contentDescription = accessibilityLabel
                progressBarRangeInfo = ProgressBarRangeInfo(value.coerceIn(valueRange), valueRange)
                setProgress { target ->
                    onValueChange(target.coerceIn(valueRange))
                    onValueChangeFinished()
                    true
                }
            }
            .pointerInput(valueRange) {
                detectTapGestures(
                    onPress = {
                        pressing = true
                        tryAwaitRelease()
                        pressing = false
                    },
                ) {
                    onValueChange(valueRange.start + (it.x / size.width).coerceIn(0f, 1f) * span)
                    onValueChangeFinished()
                }
            }
            .pointerInput(valueRange) {
                detectDragGestures(
                    onDragStart = { pressing = true },
                    onDragEnd = { pressing = false; onValueChangeFinished() },
                    onDragCancel = { pressing = false; onValueChangeFinished() },
                ) { change, _ ->
                    change.consume()
                    onValueChange(valueRange.start + (change.position.x / size.width).coerceIn(0f, 1f) * span)
                }
            },
        contentAlignment = Alignment.CenterStart,
    ) {
        Box(Modifier.fillMaxWidth().height(trackHeight).clip(CircleShape).background(colors.inactiveTrack)) {
            Box(Modifier.fillMaxHeight().fillMaxWidth(fraction).background(colors.activeTrack))
        }
        Box(
            Modifier
                .offset(x = (maxWidth - 14.dp) * fraction)
                .size(14.dp)
                .graphicsLayer { scaleX = thumbScale; scaleY = thumbScale }
                .shadow(Elevation.e1, CircleShape)
                .clip(CircleShape)
                .background(colors.thumb)
                .border(2.dp, Color.White, CircleShape),
        )
    }
}

@Composable internal fun HorizontalDivider(modifier: Modifier = Modifier, thickness: Dp = 1.dp, color: Color = border) =
    Box(modifier.fillMaxWidth().height(thickness).background(color))

@Composable internal fun VerticalDivider(modifier: Modifier = Modifier, thickness: Dp = 1.dp, color: Color = border) =
    Box(modifier.fillMaxHeight().width(thickness).background(color))

@Composable
internal fun DropdownMenu(
    expanded: Boolean,
    onDismissRequest: () -> Unit,
    modifier: Modifier = Modifier,
    shape: Shape = RoundedCornerShape(Radius.md),
    containerColor: Color = dialogSurface,
    tonalElevation: Dp = 0.dp,
    shadowElevation: Dp = 7.dp,
    content: @Composable ColumnScope.() -> Unit,
) {
    if (!expanded) return
    val reduceMotion by MotionPreferences.reduceMotion
    // 入场动画必须用 Animatable 而不是 animateFloatAsState：后者的初值就是目标值，
    // 首帧即到位，不会有任何动画。这里 Popup 一挂上就播一次。
    // 退出动画刻意不做——`if (!expanded) return` 会让整棵子树随菜单一起卸载，
    // 要做退出就得把"展开/收起"拆成两个状态，代价大于收益。
    val entrance = remember { Animatable(if (reduceMotion) 1f else 0f) }
    LaunchedEffect(Unit) { entrance.animateTo(1f, tween(motionDurationMillis(reduceMotion, 130))) }
    Popup(
        alignment = Alignment.TopEnd,
        offset = IntOffset(0, 36),
        onDismissRequest = onDismissRequest,
        properties = PopupProperties(focusable = true),
    ) {
        Surface(
            modifier.widthIn(min = 170.dp, max = 260.dp).graphicsLayer {
                // 锚点在右上角（TopEnd），所以缩放原点也取右上角，视觉上是"从按钮下拉展开"。
                scaleX = .94f + .06f * entrance.value
                scaleY = .94f + .06f * entrance.value
                alpha = entrance.value
                transformOrigin = TransformOrigin(1f, 0f)
            },
            shape, containerColor, shadowElevation = shadowElevation, border = BorderStroke(1.dp, border),
        ) {
            Column(Modifier.padding(vertical = 5.dp), content = content)
        }
    }
}

@Composable
internal fun DropdownMenuItem(
    text: @Composable () -> Unit,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
) {
    // 菜单项是整行通宽的，横向缩放在视觉上像"菜单在抖"，所以按压反馈走底色档位
    // （悬停 surface2 → 按下 surface3），pressScale 保持 1f。
    val feedback = rememberPressFeedback(
        hoverColor = surface2,
        pressedColor = surface3,
        pressScale = 1f,
        hoverMillis = 120,
    )
    Row(
        modifier
            .fillMaxWidth()
            .heightIn(min = 34.dp)
            .graphicsLayer { scaleX = feedback.scale; scaleY = feedback.scale }
            .background(feedback.background)
            .hoverable(feedback.interaction)
            .clickable(
                interactionSource = feedback.interaction,
                indication = null,
                role = Role.Button,
                onClick = onClick,
            )
            .padding(horizontal = 11.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) { text() }
}

@Composable
internal fun OutlinedTextField(
    value: String,
    onValueChange: (String) -> Unit,
    modifier: Modifier = Modifier,
    focusRequester: FocusRequester? = null,
    enabled: Boolean = true,
    singleLine: Boolean = false,
    minLines: Int = 1,
    maxLines: Int = if (singleLine) 1 else Int.MAX_VALUE,
    isError: Boolean = false,
    shape: Shape = RoundedCornerShape(Radius.md),
    label: (@Composable () -> Unit)? = null,
    placeholder: (@Composable () -> Unit)? = null,
    supportingText: (@Composable () -> Unit)? = null,
    visualTransformation: VisualTransformation = VisualTransformation.None,
) {
    val interaction = remember { MutableInteractionSource() }
    val hovered by interaction.collectIsHoveredAsState()
    val focused by interaction.collectIsFocusedAsState()
    val reduceMotion by MotionPreferences.reduceMotion
    // 边框色与粗细都过渡：聚焦时 1dp→2dp 如果瞬间跳变，会让整个输入框"抖一下"。
    // border 是绘制修饰符（画在布局边界内侧），改粗细不会触发重新测量，所以逐帧动画很便宜。
    val animatedBorderColor by animateColorAsState(
        targetValue = if (isError) errorStrong else if (focused) blue else if (hovered) blue.copy(alpha = .75f) else border,
        animationSpec = tween(motionDurationMillis(reduceMotion, 140)),
        label = "field-border",
    )
    val animatedBorderWidth by animateDpAsState(
        if (focused) 2.dp else 1.dp,
        animationSpec = tween(motionDurationMillis(reduceMotion, 140)),
        label = "field-border-width",
    )
    Column(modifier.alpha(if (enabled) 1f else .5f)) {
        if (label != null) {
            CompositionLocalProvider(LocalControlContentColor provides muted) {
                Box(Modifier.padding(start = 1.dp, bottom = 5.dp)) { label() }
            }
        }
        BasicTextField(
            value = value,
            onValueChange = onValueChange,
            enabled = enabled,
            singleLine = singleLine,
            minLines = minLines,
            maxLines = maxLines,
            textStyle = TextStyle(color = ink, fontSize = TypeScale.body, fontFamily = desktopFont, letterSpacing = 0.sp),
            cursorBrush = SolidColor(blue),
            interactionSource = interaction,
            visualTransformation = visualTransformation,
            modifier = Modifier
                .fillMaxWidth()
                .then(if (focusRequester != null) Modifier.focusRequester(focusRequester) else Modifier)
                .defaultMinSize(minHeight = if (singleLine) 36.dp else 64.dp)
                .clip(shape)
                .background(rail)
                // 显式 hoverable：BasicTextField 的 interactionSource 主要发 Press/Focus，
                // 悬停态不能指望它，否则 `hovered` 分支就是死代码。
                .hoverable(interaction)
                .border(animatedBorderWidth, animatedBorderColor, shape)
                .padding(horizontal = 10.dp, vertical = 9.dp),
            decorationBox = { inner ->
                Box {
                    if (value.isEmpty() && placeholder != null) {
                        CompositionLocalProvider(LocalControlContentColor provides faint) { placeholder() }
                    }
                    inner()
                }
            },
        )
        if (supportingText != null) {
            CompositionLocalProvider(LocalControlContentColor provides if (isError) errorStrong else faint) {
                Box(Modifier.padding(start = 2.dp, top = 4.dp)) { supportingText() }
            }
        }
    }
}

@Composable
internal fun WorkbenchTooltip(text: String, content: @Composable () -> Unit) {
    val interaction = remember { MutableInteractionSource() }
    val hovered by interaction.collectIsHoveredAsState()
    val reduceMotion by MotionPreferences.reduceMotion
    // 450ms 是"确实想停在这里"的判定延迟；出现/消失本身再叠一段淡入淡出。
    // 用 MutableTransitionState 而不是 `if (visible)`：后者会让 Popup 随 visible 一起卸载，
    // 退出动画永远播不到，而且没有"退出播完了没有"这个信号可以拿来做卸载条件。
    val visibility = remember { MutableTransitionState(false) }
    LaunchedEffect(hovered) {
        if (hovered) {
            delay(450)
            visibility.targetState = true
        } else {
            visibility.targetState = false
        }
    }
    Box(Modifier.hoverable(interaction)) {
        content()
        if (visibility.currentState || visibility.targetState) {
            Popup(popupPositionProvider = WorkbenchTooltipPositionProvider, properties = PopupProperties(focusable = false)) {
                AnimatedVisibility(
                    visibleState = visibility,
                    enter = fadeIn(tween(motionDurationMillis(reduceMotion, 110))),
                    exit = fadeOut(tween(motionDurationMillis(reduceMotion, 90))),
                ) {
                    Surface(shape = RoundedCornerShape(Radius.sm), color = tooltipSurface, shadowElevation = Elevation.e2) {
                        Text(text, Modifier.padding(horizontal = 8.dp, vertical = 5.dp), color = Color.White, fontSize = TypeScale.micro)
                    }
                }
            }
        }
    }
}

private object WorkbenchTooltipPositionProvider : PopupPositionProvider {
    override fun calculatePosition(
        anchorBounds: IntRect,
        windowSize: IntSize,
        layoutDirection: LayoutDirection,
        popupContentSize: IntSize,
    ): IntOffset {
        val margin = 6
        val x = (anchorBounds.left + (anchorBounds.width - popupContentSize.width) / 2)
            .coerceIn(margin, (windowSize.width - popupContentSize.width - margin).coerceAtLeast(margin))
        val below = anchorBounds.bottom + margin
        val y = if (below + popupContentSize.height <= windowSize.height - margin) {
            below
        } else {
            (anchorBounds.top - popupContentSize.height - margin).coerceAtLeast(margin)
        }
        return IntOffset(x, y)
    }
}

/**
 * 把颜色朝目标方向混合 `amount` 比例。
 *
 * 用 `internal` 而不是 `private`：悬停反馈在 Main.kt / SettingsV7.kt 里也要用
 * 「往 ink 压一档」表达按压（ink 浅色主题是深色、深色主题是浅色，所以
 * `blendToward(ink, …)` 在两个主题下都是"更远离底色"的正确方向）。
 */
internal fun Color.blendToward(target: Color, amount: Float): Color = Color(
    red = red + (target.red - red) * amount,
    green = green + (target.green - green) * amount,
    blue = blue + (target.blue - blue) * amount,
    alpha = alpha,
)