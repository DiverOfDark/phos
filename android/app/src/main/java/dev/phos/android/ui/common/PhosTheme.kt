package dev.phos.android.ui.common

import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Shapes
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.phos.android.R

/**
 * AppBahn — Bauhaus Engineering.
 *
 * Dark is the only mode: this is a tool you open at night next to a photo
 * library. Dynamic color is deliberately off — status colour is the app's only
 * carrier of meaning, so the wallpaper does not get a vote.
 */

// --- Palette (oklch values from the design system, converted to sRGB) ---
private val BgBase = Color(0xFF0D0F14)
private val BgSurface = Color(0xFF14161B)
private val BgRaised = Color(0xFF201F1B)
private val BgOverlay = Color(0xFF25262C)

private val TextPrimary = Color(0xFFEDEAE4)
private val TextSecondary = Color(0xFF919AA6)
private val TextTertiary = Color(0xFF6B727C)

private val Signal = Color(0xFFF0B429)
private val SignalHover = Color(0xFFFFC44D)
private val SignalMuted = Color(0xFF6B4E12)
private val SignalFg = Color(0xFF1F1A0E)

private val StatusReady = Color(0xFF4FBF7B)
private val StatusDegraded = Color(0xFFE0A93A)
private val StatusError = Color(0xFFE05A4E)
private val StatusPending = Color(0xFF7E97C9)
private val StatusStopped = Color(0xFF6B727C)
private val StatusBuilding = Color(0xFF8AA2E0)

private val Line = Color(0xFF23262D)
private val LineStrong = Color(0xFF3A3E46)

/** Tokens Material 3 has no slot for. Read through [PhosColors]. */
@Immutable
data class PhosColorTokens(
    val base: Color = BgBase,
    val surface: Color = BgSurface,
    val raised: Color = BgRaised,
    val overlay: Color = BgOverlay,
    val textPrimary: Color = TextPrimary,
    val textSecondary: Color = TextSecondary,
    val textTertiary: Color = TextTertiary,
    val signal: Color = Signal,
    val signalHover: Color = SignalHover,
    val signalMuted: Color = SignalMuted,
    val signalFg: Color = SignalFg,
    val ready: Color = StatusReady,
    val degraded: Color = StatusDegraded,
    val error: Color = StatusError,
    val pending: Color = StatusPending,
    val stopped: Color = StatusStopped,
    val building: Color = StatusBuilding,
    val line: Color = Line,
    val lineStrong: Color = LineStrong,
)

val PhosColors = staticCompositionLocalOf { PhosColorTokens() }

// --- Type ---
private val Geist = FontFamily(
    Font(R.font.geist_400, FontWeight.Normal),
    Font(R.font.geist_500, FontWeight.Medium),
    Font(R.font.geist_600, FontWeight.SemiBold),
    Font(R.font.geist_700, FontWeight.Bold),
)

private val Inter = FontFamily(
    Font(R.font.inter_300, FontWeight.Light),
    Font(R.font.inter_400, FontWeight.Normal),
    Font(R.font.inter_500, FontWeight.Medium),
    Font(R.font.inter_600, FontWeight.SemiBold),
)

/** IDs, counts, filenames, timestamps — the railway-schedule register. */
val PhosMono = FontFamily(
    Font(R.font.jetbrains_mono_400, FontWeight.Normal),
    Font(R.font.jetbrains_mono_500, FontWeight.Medium),
)

val PhosHeading = Geist

private val PhosTypography = Typography(
    // Headings: Geist, hierarchy through weight rather than colour.
    headlineLarge = TextStyle(fontFamily = Geist, fontWeight = FontWeight.Bold, fontSize = 28.sp, lineHeight = 34.sp, letterSpacing = (-0.4).sp),
    headlineMedium = TextStyle(fontFamily = Geist, fontWeight = FontWeight.Bold, fontSize = 22.sp, lineHeight = 27.sp, letterSpacing = (-0.3).sp),
    headlineSmall = TextStyle(fontFamily = Geist, fontWeight = FontWeight.SemiBold, fontSize = 18.sp, lineHeight = 22.sp),
    titleLarge = TextStyle(fontFamily = Geist, fontWeight = FontWeight.Bold, fontSize = 16.sp, lineHeight = 20.sp),
    titleMedium = TextStyle(fontFamily = Geist, fontWeight = FontWeight.SemiBold, fontSize = 16.sp, lineHeight = 20.sp),
    titleSmall = TextStyle(fontFamily = Inter, fontWeight = FontWeight.Medium, fontSize = 14.sp, lineHeight = 20.sp),

    // Body: Inter. Secondary text is Light, not a lighter colour.
    bodyLarge = TextStyle(fontFamily = Inter, fontWeight = FontWeight.Normal, fontSize = 15.sp, lineHeight = 22.sp),
    bodyMedium = TextStyle(fontFamily = Inter, fontWeight = FontWeight.Normal, fontSize = 14.sp, lineHeight = 21.sp),
    bodySmall = TextStyle(fontFamily = Inter, fontWeight = FontWeight.Light, fontSize = 13.sp, lineHeight = 19.sp),

    labelLarge = TextStyle(fontFamily = Inter, fontWeight = FontWeight.Medium, fontSize = 14.sp, lineHeight = 20.sp),
    labelMedium = TextStyle(fontFamily = Inter, fontWeight = FontWeight.Medium, fontSize = 12.sp, lineHeight = 16.sp),
    labelSmall = TextStyle(fontFamily = PhosMono, fontWeight = FontWeight.Normal, fontSize = 11.sp, lineHeight = 15.sp),
)

/** Data text: filenames, counts, distances, ids. */
val MonoSmall = TextStyle(fontFamily = PhosMono, fontWeight = FontWeight.Normal, fontSize = 11.sp, lineHeight = 15.sp)
val MonoBody = TextStyle(fontFamily = PhosMono, fontWeight = FontWeight.Normal, fontSize = 12.sp, lineHeight = 17.sp)

/** UPPERCASE mono section label, letter-spaced. Use with [String.uppercase]. */
val MonoLabel = TextStyle(
    fontFamily = PhosMono,
    fontWeight = FontWeight.Medium,
    fontSize = 11.sp,
    lineHeight = 15.sp,
    letterSpacing = 0.9.sp,
)

// Radii: inputs 2dp, everything else 4dp. Nothing pill-shaped.
private val PhosShapes = Shapes(
    extraSmall = RoundedCornerShape(2.dp),
    small = RoundedCornerShape(4.dp),
    medium = RoundedCornerShape(4.dp),
    large = RoundedCornerShape(4.dp),
    extraLarge = RoundedCornerShape(4.dp),
)

private val PhosColorScheme = darkColorScheme(
    primary = Signal,
    onPrimary = SignalFg,
    primaryContainer = SignalMuted,
    onPrimaryContainer = TextPrimary,
    secondary = TextSecondary,
    onSecondary = BgBase,
    secondaryContainer = BgRaised,
    onSecondaryContainer = TextPrimary,
    tertiary = StatusBuilding,
    onTertiary = BgBase,
    background = BgBase,
    onBackground = TextPrimary,
    surface = BgSurface,
    onSurface = TextPrimary,
    surfaceVariant = BgRaised,
    onSurfaceVariant = TextSecondary,
    surfaceContainer = BgSurface,
    surfaceContainerHigh = BgRaised,
    surfaceContainerHighest = BgOverlay,
    surfaceContainerLow = BgSurface,
    surfaceContainerLowest = BgBase,
    inverseSurface = TextPrimary,
    inverseOnSurface = BgBase,
    error = StatusError,
    onError = TextPrimary,
    errorContainer = BgSurface,
    onErrorContainer = StatusError,
    outline = LineStrong,
    outlineVariant = Line,
    scrim = Color(0x80000000),
)

@Composable
fun PhosTheme(
    @Suppress("UNUSED_PARAMETER") darkTheme: Boolean = true,
    content: @Composable () -> Unit,
) {
    MaterialTheme(
        colorScheme = PhosColorScheme,
        typography = PhosTypography,
        shapes = PhosShapes,
        content = content,
    )
}
