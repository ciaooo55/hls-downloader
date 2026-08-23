package com.hlsdownloader.desktop

import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.animateFloatAsState
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
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ColorFilter
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.SolidColor
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

@Composable
internal fun Text(
    text: String,
    modifier: Modifier = Modifier,
    color: Color = Color.Unspecified,
    fontSize: TextUnit = 12.sp,
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
    shape: Shape = RoundedCornerShape(7.dp),
    border: BorderStroke? = null,
    contentPadding: PaddingValues = PaddingValues(horizontal = 12.dp, vertical = 8.dp),
    colors: ButtonColors = ButtonDefaults.buttonColors(blue, Color.White),
    content: @Composable RowScope.() -> Unit,
) {
    val interaction = remember { MutableInteractionSource() }
    val hovered by interaction.collectIsHoveredAsState()
    val focused by interaction.collectIsFocusedAsState()
    val pressed by interaction.collectIsPressedAsState()
    val container = if (enabled) colors.container else colors.disabledContainer
    val contentColor = if (enabled) colors.content else colors.disabledContent
    val animatedContainer by animateColorAsState(
        targetValue = if (hovered && enabled) container.blendToward(Color.White, .08f) else container,
        label = "button-container",
    )
    val pressScale by animateFloatAsState(if (pressed && enabled) .985f else 1f, label = "button-press")
    Row(
        modifier
            .defaultMinSize(minHeight = 34.dp)
            .clip(shape)
            .graphicsLayer { scaleX = pressScale; scaleY = pressScale }
            .background(animatedContainer)
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
    shape = RoundedCornerShape(6.dp),
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
    val hovered by interaction.collectIsHoveredAsState()
    val pressed by interaction.collectIsPressedAsState()
    val animatedSurface by animateColorAsState(if (hovered && enabled) surface3 else Color.Transparent, label = "icon-button-surface")
    val pressScale by animateFloatAsState(if (pressed && enabled) .9f else 1f, label = "icon-button-press")
    Box(
        modifier
            .defaultMinSize(34.dp, 34.dp)
            .clip(RoundedCornerShape(6.dp))
            .graphicsLayer { scaleX = pressScale; scaleY = pressScale }
            .background(animatedSurface)
            .hoverable(interaction)
            .clickable(interactionSource = interaction, indication = null, enabled = enabled, role = Role.Button, onClick = onClick)
            .alpha(if (enabled) 1f else .42f),
        contentAlignment = Alignment.Center,
        content = content,
    )
}

@Composable
internal fun Checkbox(checked: Boolean, onCheckedChange: (Boolean) -> Unit, modifier: Modifier = Modifier, accessibilityLabel: String? = null) {
    Box(
        modifier
            .defaultMinSize(18.dp, 18.dp)
            .clip(RoundedCornerShape(4.dp))
            .background(if (checked) blue else rail)
            .border(1.dp, if (checked) blue else border, RoundedCornerShape(4.dp))
            .semantics {
                stateDescription = if (checked) "已选中" else "未选中"
                if (accessibilityLabel != null) contentDescription = accessibilityLabel
            }
            .toggleable(value = checked, role = Role.Checkbox, onValueChange = onCheckedChange),
        contentAlignment = Alignment.Center,
    ) {
        if (checked) androidx.compose.foundation.Canvas(Modifier.size(12.dp)) {
            val stroke = 1.8.dp.toPx()
            drawLine(Color.White, androidx.compose.ui.geometry.Offset(size.width * .18f, size.height * .52f), androidx.compose.ui.geometry.Offset(size.width * .42f, size.height * .76f), stroke)
            drawLine(Color.White, androidx.compose.ui.geometry.Offset(size.width * .42f, size.height * .76f), androidx.compose.ui.geometry.Offset(size.width * .84f, size.height * .25f), stroke)
        }
    }
}

@Composable
internal fun RadioButton(selected: Boolean, onClick: () -> Unit, modifier: Modifier = Modifier, accessibilityLabel: String? = null) {
    val animatedFill by animateColorAsState(if (selected) selectedSurface else rail, label = "radio-fill")
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
    val animatedTrack by animateColorAsState(if (checked) blue else surface3, label = "switch-track")
    val knobFraction by animateFloatAsState(if (checked) 1f else 0f, label = "switch-position")
    Box(
        modifier
            .width(36.dp)
            .height(20.dp)
            .clip(CircleShape)
            .background(animatedTrack)
            .border(1.dp, if (checked) blue else border, CircleShape)
            .semantics {
                stateDescription = if (checked) "已开启" else "已关闭"
                if (accessibilityLabel != null) contentDescription = accessibilityLabel
            }
            .toggleable(value = checked, role = Role.Switch, onValueChange = onCheckedChange)
            .padding(2.dp),
    ) {
        Box(
            Modifier
                .align(Alignment.CenterStart)
                .offset(x = 16.dp * knobFraction)
                .size(14.dp)
                .clip(CircleShape)
                .background(Color.White)
                .shadow(1.dp, CircleShape),
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
    val value by animateFloatAsState(progress().coerceIn(0f, 1f), label = "progress-value")
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
    androidx.compose.foundation.Canvas(modifier) {
        drawArc(color, -90f, 270f, false, style = Stroke(strokeWidth.toPx()))
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
    BoxWithConstraints(
        modifier
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
                detectTapGestures {
                    onValueChange(valueRange.start + (it.x / size.width).coerceIn(0f, 1f) * span)
                    onValueChangeFinished()
                }
            }
            .pointerInput(valueRange) {
                detectDragGestures(
                    onDragEnd = onValueChangeFinished,
                    onDragCancel = onValueChangeFinished,
                ) { change, _ ->
                    change.consume()
                    onValueChange(valueRange.start + (change.position.x / size.width).coerceIn(0f, 1f) * span)
                }
            },
        contentAlignment = Alignment.CenterStart,
    ) {
        Box(Modifier.fillMaxWidth().height(4.dp).clip(CircleShape).background(colors.inactiveTrack)) {
            Box(Modifier.fillMaxHeight().fillMaxWidth(fraction).background(colors.activeTrack))
        }
        Box(
            Modifier
                .offset(x = (maxWidth - 14.dp) * fraction)
                .size(14.dp)
                .shadow(2.dp, CircleShape)
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
    shape: Shape = RoundedCornerShape(7.dp),
    containerColor: Color = dialogSurface,
    tonalElevation: Dp = 0.dp,
    shadowElevation: Dp = 7.dp,
    content: @Composable ColumnScope.() -> Unit,
) {
    if (!expanded) return
    Popup(
        alignment = Alignment.TopEnd,
        offset = IntOffset(0, 36),
        onDismissRequest = onDismissRequest,
        properties = PopupProperties(focusable = true),
    ) {
        Surface(modifier.widthIn(min = 170.dp, max = 260.dp), shape, containerColor, shadowElevation = shadowElevation, border = BorderStroke(1.dp, border)) {
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
    Row(
        modifier.fillMaxWidth().heightIn(min = 34.dp).clickable(role = Role.Button, onClick = onClick).padding(horizontal = 11.dp),
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
    shape: Shape = RoundedCornerShape(7.dp),
    label: (@Composable () -> Unit)? = null,
    placeholder: (@Composable () -> Unit)? = null,
    supportingText: (@Composable () -> Unit)? = null,
    visualTransformation: VisualTransformation = VisualTransformation.None,
) {
    val interaction = remember { MutableInteractionSource() }
    val hovered by interaction.collectIsHoveredAsState()
    val focused by interaction.collectIsFocusedAsState()
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
            textStyle = TextStyle(color = ink, fontSize = 12.sp, fontFamily = desktopFont, letterSpacing = 0.sp),
            cursorBrush = SolidColor(blue),
            interactionSource = interaction,
            visualTransformation = visualTransformation,
            modifier = Modifier
                .fillMaxWidth()
                .then(if (focusRequester != null) Modifier.focusRequester(focusRequester) else Modifier)
                .defaultMinSize(minHeight = if (singleLine) 36.dp else 64.dp)
                .clip(shape)
                .background(rail)
                .border(
                    if (focused) 2.dp else 1.dp,
                    if (isError) Color(0xFFDC2626) else if (focused) blue else if (hovered) blue.copy(alpha = .75f) else border,
                    shape,
                )
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
            CompositionLocalProvider(LocalControlContentColor provides if (isError) Color(0xFFDC2626) else faint) {
                Box(Modifier.padding(start = 2.dp, top = 4.dp)) { supportingText() }
            }
        }
    }
}

@Composable
internal fun WorkbenchTooltip(text: String, content: @Composable () -> Unit) {
    val interaction = remember { MutableInteractionSource() }
    val hovered by interaction.collectIsHoveredAsState()
    var visible by remember { mutableStateOf(false) }
    LaunchedEffect(hovered) {
        if (hovered) {
            delay(450)
            visible = true
        } else {
            visible = false
        }
    }
    Box(Modifier.hoverable(interaction)) {
        content()
        if (visible) Popup(popupPositionProvider = WorkbenchTooltipPositionProvider, properties = PopupProperties(focusable = false)) {
            Surface(shape = RoundedCornerShape(5.dp), color = Color(0xEE20242A), shadowElevation = 4.dp) {
                Text(text, Modifier.padding(horizontal = 8.dp, vertical = 5.dp), color = Color.White, fontSize = 10.sp)
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

private fun Color.blendToward(target: Color, amount: Float): Color = Color(
    red = red + (target.red - red) * amount,
    green = green + (target.green - green) * amount,
    blue = blue + (target.blue - blue) * amount,
    alpha = alpha,
)
