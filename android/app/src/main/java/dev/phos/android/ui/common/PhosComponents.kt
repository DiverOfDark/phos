package dev.phos.android.ui.common

import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp

/**
 * AppBahn primitives.
 *
 * The design system has a small vocabulary — a signal dot, an uppercase mono
 * label, an outlined status tag, a 1px-bordered row — and everything on screen
 * is built from it. Keeping them here stops each screen from inventing its own.
 */

/**
 * "2m ago" for a timestamp, "never" for none.
 *
 * Relative while it is recent is the whole point of the sync line: an absolute
 * clock time makes the reader do the subtraction.
 */
fun relativeSince(epochMillis: Long?): String {
    if (epochMillis == null) return "never"
    val mins = (System.currentTimeMillis() - epochMillis) / 60_000
    return when {
        mins < 1 -> "just now"
        mins < 60 -> "${mins}m ago"
        mins < 60 * 24 -> "${mins / 60}h ago"
        else -> "${mins / (60 * 24)}d ago"
    }
}

/** A signal light. Colour is the meaning; size is 8dp unless it is a headline. */
@Composable
fun SignalDot(
    color: Color,
    modifier: Modifier = Modifier,
    size: Dp = 8.dp,
    pulsing: Boolean = false,
) {
    val alpha = if (pulsing) {
        val transition = androidx.compose.animation.core.rememberInfiniteTransition(label = "signal")
        val v by transition.animateFloat(
            initialValue = 1f,
            targetValue = 0.35f,
            animationSpec = infiniteRepeatable(tween(700), RepeatMode.Reverse),
            label = "signal_alpha",
        )
        v
    } else {
        1f
    }
    Box(
        modifier = modifier
            .size(size)
            .alpha(alpha)
            .clip(CircleShape)
            .background(color),
    )
}

/** UPPERCASE mono section label — the railway-schedule register. */
@Composable
fun PhosLabel(
    text: String,
    modifier: Modifier = Modifier,
    color: Color = PhosColors.current.textTertiary,
) {
    Text(
        text = text.uppercase(),
        style = MonoLabel,
        color = color,
        modifier = modifier,
    )
}

/** Mono data text: filenames, counts, distances, ids. */
@Composable
fun PhosMonoText(
    text: String,
    modifier: Modifier = Modifier,
    color: Color = PhosColors.current.textTertiary,
    style: androidx.compose.ui.text.TextStyle = MonoSmall,
    maxLines: Int = 1,
) {
    Text(
        text = text,
        style = style,
        color = color,
        maxLines = maxLines,
        overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis,
        modifier = modifier,
    )
}

/** An outlined UPPERCASE status tag: READY, PENDING, VIDEO, MASTER. */
@Composable
fun PhosTag(
    text: String,
    color: Color,
    modifier: Modifier = Modifier,
    background: Color = Color.Transparent,
) {
    Box(
        modifier = modifier
            .clip(RoundedCornerShape(2.dp))
            .background(background)
            .border(1.dp, color, RoundedCornerShape(2.dp))
            .padding(horizontal = 6.dp, vertical = 1.dp),
    ) {
        Text(text = text.uppercase(), style = MonoLabel, color = color)
    }
}

/**
 * A bordered secondary button. Hover has no meaning on a phone, so the whole
 * affordance is the 1px outline plus the label.
 */
@Composable
fun PhosOutlinedButton(
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
    contentColor: Color = PhosColors.current.textSecondary,
    content: @Composable RowScope.() -> Unit,
) {
    val c = PhosColors.current
    Row(
        modifier = modifier
            .clip(RoundedCornerShape(4.dp))
            .border(1.dp, if (enabled) c.lineStrong else c.line, RoundedCornerShape(4.dp))
            .clickable(enabled = enabled, onClick = onClick)
            .padding(horizontal = 12.dp, vertical = 8.dp)
            .alpha(if (enabled) 1f else 0.4f),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(6.dp),
        content = content,
    )
}

/** The primary action: solid signal amber, dark label. */
@Composable
fun PhosPrimaryButton(
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
    content: @Composable RowScope.() -> Unit,
) {
    val c = PhosColors.current
    Row(
        modifier = modifier
            .clip(RoundedCornerShape(4.dp))
            .background(c.signal)
            .clickable(enabled = enabled, onClick = onClick)
            .padding(horizontal = 16.dp, vertical = 12.dp)
            .alpha(if (enabled) 1f else 0.4f),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.Center,
        content = content,
    )
}

/**
 * A 56dp top bar with a hairline under it — the app's only chrome.
 *
 * One composable, not a bar plus a stray divider: Scaffold's topBar slot places
 * every child it is given at the same origin, so siblings would stack on top of
 * each other. The status-bar inset is the bar's own because the app draws
 * edge to edge.
 */
@Composable
fun PhosTopBar(
    modifier: Modifier = Modifier,
    content: @Composable RowScope.() -> Unit,
) {
    val c = PhosColors.current
    androidx.compose.foundation.layout.Column(
        modifier = modifier
            .fillMaxWidth()
            .background(c.base)
            .statusBarsPadding(),
    ) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .height(56.dp)
                .padding(horizontal = 16.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
            content = content,
        )
        PhosDivider()
    }
}

/** The 1px track line that carries layout weight instead of a shadow. */
@Composable
fun PhosDivider(modifier: Modifier = Modifier, strong: Boolean = false) {
    val c = PhosColors.current
    Box(
        modifier = modifier
            .fillMaxWidth()
            .height(1.dp)
            .background(if (strong) c.lineStrong else c.line),
    )
}

/** A tappable list row: 48dp minimum, hairline underneath, arrow at the end. */
@Composable
fun PhosRow(
    onClick: (() -> Unit)? = null,
    modifier: Modifier = Modifier,
    content: @Composable RowScope.() -> Unit,
) {
    Row(
        modifier = modifier
            .fillMaxWidth()
            .then(if (onClick != null) Modifier.clickable(onClick = onClick) else Modifier)
            .heightIn(min = 48.dp)
            .padding(horizontal = 16.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(16.dp),
        content = content,
    )
    PhosDivider()
}

/**
 * The square avatar slot. 4dp radius, never a circle — the design system has no
 * round avatars, and a square reads as "a crop of a photo" rather than "a user".
 */
@Composable
fun PhosAvatarBox(
    modifier: Modifier = Modifier,
    size: Dp = 48.dp,
    dashed: Boolean = false,
    content: @Composable () -> Unit,
) {
    val c = PhosColors.current
    Box(
        modifier = modifier
            .size(size)
            .clip(RoundedCornerShape(4.dp))
            .background(if (dashed) Color.Transparent else c.raised)
            .border(1.dp, if (dashed) c.lineStrong else c.line, RoundedCornerShape(4.dp)),
        contentAlignment = Alignment.Center,
        content = { content() },
    )
}

/**
 * A bottom sheet in AppBahn dress: overlay fill, a hairline top border, 4dp
 * corners and no drag handle pill — the sheet's own header carries the ✕.
 */
@OptIn(androidx.compose.material3.ExperimentalMaterial3Api::class)
@Composable
fun PhosSheet(
    onDismiss: () -> Unit,
    modifier: Modifier = Modifier,
    content: @Composable androidx.compose.foundation.layout.ColumnScope.() -> Unit,
) {
    val c = PhosColors.current
    androidx.compose.material3.ModalBottomSheet(
        onDismissRequest = onDismiss,
        containerColor = c.overlay,
        contentColor = c.textPrimary,
        scrimColor = Color.Black.copy(alpha = 0.5f),
        shape = RoundedCornerShape(topStart = 4.dp, topEnd = 4.dp),
        dragHandle = { Box(modifier = Modifier.fillMaxWidth().height(1.dp).background(c.lineStrong)) },
        modifier = modifier,
        content = content,
    )
}

/** A sheet's title row: name on the left, a destructive verb and ✕ on the right. */
@Composable
fun PhosSheetHeader(
    title: String,
    onDismiss: () -> Unit,
    modifier: Modifier = Modifier,
    subtitle: String? = null,
    destructiveLabel: String? = null,
    onDestructive: (() -> Unit)? = null,
) {
    val c = PhosColors.current
    Row(
        modifier = modifier
            .fillMaxWidth()
            .padding(horizontal = 16.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        androidx.compose.foundation.layout.Column(modifier = Modifier.weight(1f)) {
            Text(
                text = title,
                style = androidx.compose.material3.MaterialTheme.typography.titleMedium,
                color = c.textPrimary,
            )
            if (subtitle != null) {
                Text(
                    text = subtitle,
                    style = androidx.compose.material3.MaterialTheme.typography.bodySmall,
                    color = c.textSecondary,
                )
            }
        }
        if (destructiveLabel != null && onDestructive != null) {
            Text(
                text = destructiveLabel,
                style = MonoSmall,
                color = c.error,
                modifier = Modifier.clickable(onClick = onDestructive).padding(8.dp),
            )
        }
        Text(
            text = "✕",
            style = MonoSmall,
            color = c.textTertiary,
            modifier = Modifier.clickable(onClick = onDismiss).padding(8.dp),
        )
    }
    PhosDivider()
}

/** One choice in a sheet: square initial or thumbnail, label, mono meta. */
@Composable
fun PhosSheetRow(
    label: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    labelColor: Color? = null,
    meta: String? = null,
    metaColor: Color? = null,
    leading: (@Composable () -> Unit)? = null,
) {
    val c = PhosColors.current
    Row(
        modifier = modifier
            .fillMaxWidth()
            .clickable(onClick = onClick)
            .heightIn(min = 48.dp)
            .padding(horizontal = 16.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        leading?.invoke()
        Text(
            text = label,
            style = androidx.compose.material3.MaterialTheme.typography.bodyMedium,
            color = labelColor ?: c.textPrimary,
            modifier = Modifier.weight(1f),
        )
        if (meta != null) {
            Text(text = meta, style = MonoSmall, color = metaColor ?: c.textTertiary)
        }
    }
    PhosDivider()
}

/** The plain 2dp-radius search field the sheets share. */
@Composable
fun PhosSearchField(
    value: String,
    onValueChange: (String) -> Unit,
    placeholder: String,
    modifier: Modifier = Modifier,
) {
    val c = PhosColors.current
    val style = androidx.compose.material3.MaterialTheme.typography.bodyMedium.copy(color = c.textPrimary)
    androidx.compose.foundation.text.BasicTextField(
        value = value,
        onValueChange = onValueChange,
        singleLine = true,
        textStyle = style,
        cursorBrush = androidx.compose.ui.graphics.SolidColor(c.signal),
        modifier = modifier
            .fillMaxWidth()
            .background(c.base, RoundedCornerShape(2.dp))
            .border(1.dp, c.line, RoundedCornerShape(2.dp))
            .padding(horizontal = 12.dp, vertical = 10.dp),
        decorationBox = { inner ->
            if (value.isEmpty()) Text(placeholder, style = style.copy(color = c.textTertiary))
            inner()
        },
    )
}
