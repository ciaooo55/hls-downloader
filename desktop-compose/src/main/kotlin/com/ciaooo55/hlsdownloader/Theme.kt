package com.ciaooo55.hlsdownloader

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ColorScheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

val DesktopControlShape = RoundedCornerShape(5.dp)
val DesktopPanelShape = RoundedCornerShape(8.dp)

val DarkColors = darkColorScheme(
    primary = Color(0xFF55A8D8),
    onPrimary = Color(0xFF071F2D),
    primaryContainer = Color(0xFF163D54),
    onPrimaryContainer = Color(0xFFD6F0FF),
    secondary = Color(0xFF88C9A8),
    onSecondary = Color(0xFF092116),
    background = Color(0xFF1D1E21),
    onBackground = Color(0xFFE7E8EB),
    surface = Color(0xFF242529),
    onSurface = Color(0xFFE7E8EB),
    surfaceVariant = Color(0xFF2C2E33),
    onSurfaceVariant = Color(0xFFB5B7BE),
    outline = Color(0xFF44464E),
    outlineVariant = Color(0xFF35373D),
    error = Color(0xFFFF8A86),
    onError = Color(0xFF3C0808),
)

val LightColors = lightColorScheme(
    primary = Color(0xFF286F9A),
    onPrimary = Color.White,
    primaryContainer = Color(0xFFD5EEFC),
    onPrimaryContainer = Color(0xFF0B344A),
    secondary = Color(0xFF327657),
    onSecondary = Color.White,
    background = Color(0xFFF5F6F7),
    onBackground = Color(0xFF202124),
    surface = Color(0xFFFFFFFF),
    onSurface = Color(0xFF202124),
    surfaceVariant = Color(0xFFEEF0F2),
    onSurfaceVariant = Color(0xFF565A61),
    outline = Color(0xFFC5C8CE),
    outlineVariant = Color(0xFFE0E2E6),
    error = Color(0xFFB42324),
    onError = Color.White,
)

private val DesktopTypography = Typography(
    bodyLarge = TextStyle(fontFamily = FontFamily.SansSerif, fontSize = 14.sp, lineHeight = 20.sp),
    bodyMedium = TextStyle(fontFamily = FontFamily.SansSerif, fontSize = 13.sp, lineHeight = 18.sp),
    bodySmall = TextStyle(fontFamily = FontFamily.SansSerif, fontSize = 11.sp, lineHeight = 15.sp),
    titleLarge = TextStyle(fontFamily = FontFamily.SansSerif, fontSize = 18.sp, lineHeight = 24.sp, fontWeight = FontWeight.SemiBold),
    titleMedium = TextStyle(fontFamily = FontFamily.SansSerif, fontSize = 15.sp, lineHeight = 20.sp, fontWeight = FontWeight.SemiBold),
    titleSmall = TextStyle(fontFamily = FontFamily.SansSerif, fontSize = 13.sp, lineHeight = 18.sp, fontWeight = FontWeight.SemiBold),
    labelLarge = TextStyle(fontFamily = FontFamily.SansSerif, fontSize = 12.sp, fontWeight = FontWeight.Medium),
    labelMedium = TextStyle(fontFamily = FontFamily.SansSerif, fontSize = 11.sp, fontWeight = FontWeight.Medium),
)

@Composable
fun HlsTheme(dark: Boolean = isSystemInDarkTheme(), content: @Composable () -> Unit) {
    MaterialTheme(
        colorScheme = if (dark) DarkColors else LightColors,
        typography = DesktopTypography,
        shapes = MaterialTheme.shapes.copy(
            extraSmall = RoundedCornerShape(3),
            small = DesktopControlShape,
            medium = RoundedCornerShape(7),
            large = DesktopPanelShape,
        ),
        content = content,
    )
}

fun ColorScheme.statusColor(status: String): Color = when (status) {
    "done" -> Color(0xFF53B987)
    "failed", "unsupported" -> error
    "paused", "pausing", "merging" -> Color(0xFFE0A84B)
    "remuxing", "parsing" -> Color(0xFFA78ADD)
    "downloading", "downloading_m3u8", "downloading_segments" -> primary
    else -> onSurfaceVariant
}
