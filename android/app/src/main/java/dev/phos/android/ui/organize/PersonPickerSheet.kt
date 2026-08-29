package dev.phos.android.ui.organize

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.unit.dp
import coil3.compose.AsyncImage
import dev.phos.android.domain.model.Person
import dev.phos.android.ui.common.PhosAvatarBox
import dev.phos.android.ui.common.PhosColors
import dev.phos.android.ui.common.PhosMonoText
import dev.phos.android.ui.common.PhosSearchField
import dev.phos.android.ui.common.PhosSheet
import dev.phos.android.ui.common.PhosSheetHeader
import dev.phos.android.ui.common.PhosSheetRow
import dev.phos.android.ui.common.SignalDot

/**
 * "Which person?" — the picker behind every reassignment.
 *
 * Shared by the single-shot move and the grid's batch move, because the two are the
 * same question and a phone-sized list of people is worth getting right once: search
 * first (a library has hundreds of people and no room for a scroll), and creating a
 * new person is a row *in* the list rather than a separate dialog, so "this is
 * someone new" stays one gesture.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun PersonPickerSheet(
    people: List<Person>,
    isLoading: Boolean,
    title: String,
    onDismiss: () -> Unit,
    onPick: (personId: String) -> Unit,
    onCreate: (name: String) -> Unit,
) {
    var query by remember { mutableStateOf("") }
    val c = PhosColors.current

    PhosSheet(onDismiss = onDismiss) {
        PhosSheetHeader(title = title, onDismiss = onDismiss)

        Column(
            modifier = Modifier
                .fillMaxWidth()
                .navigationBarsPadding(),
        ) {
            Box(modifier = Modifier.padding(horizontal = 16.dp, vertical = 12.dp)) {
                PhosSearchField(
                    value = query,
                    onValueChange = { query = it },
                    placeholder = "Search or type a new name…",
                )
            }

            val trimmed = query.trim()
            val matches = remember(people, trimmed) {
                if (trimmed.isEmpty()) people
                else people.filter { it.name?.contains(trimmed, ignoreCase = true) == true }
            }
            // Offered only when nothing is named exactly this, so the row cannot be
            // used to make a second "Anna" by accident.
            val canCreate = trimmed.isNotEmpty() &&
                people.none { it.name.equals(trimmed, ignoreCase = true) }

            if (isLoading) {
                Row(
                    modifier = Modifier.fillMaxWidth().padding(24.dp),
                    horizontalArrangement = Arrangement.Center,
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    SignalDot(color = c.building, size = 6.dp, pulsing = true)
                    Spacer(Modifier.padding(4.dp))
                    PhosMonoText("loading people…", color = c.textSecondary)
                }
            } else {
                LazyColumn(modifier = Modifier.heightIn(max = 420.dp)) {
                    if (canCreate) {
                        item {
                            PhosSheetRow(
                                label = "Create \"$trimmed\"",
                                labelColor = c.signal,
                                meta = "new person",
                                leading = { PhosAvatarBox(size = 32.dp) { PhosMonoText("+", color = c.signal) } },
                                onClick = { onCreate(trimmed) },
                            )
                        }
                    }

                    items(matches, key = { it.id }) { person ->
                        PhosSheetRow(
                            label = person.name ?: "unnamed cluster",
                            meta = "${person.shotCount} shots",
                            leading = { PersonAvatar(person) },
                            onClick = { onPick(person.id) },
                        )
                    }

                    if (matches.isEmpty() && !canCreate) {
                        item {
                            Text(
                                text = "No people yet.",
                                style = MaterialTheme.typography.bodyMedium,
                                color = c.textSecondary,
                                modifier = Modifier.padding(24.dp),
                            )
                        }
                    }
                }
            }

            Spacer(Modifier.height(8.dp))
        }
    }
}

/** A person's square avatar: their cover crop, or their initial. */
@Composable
internal fun PersonAvatar(person: Person, size: androidx.compose.ui.unit.Dp = 32.dp) {
    val c = PhosColors.current
    PhosAvatarBox(size = size) {
        val url = person.coverShotThumbnailUrl ?: person.thumbnailUrl
        if (url != null && url.startsWith("http")) {
            AsyncImage(
                model = url,
                contentDescription = null,
                contentScale = ContentScale.Crop,
                modifier = Modifier.fillMaxSize(),
            )
        } else {
            PhosMonoText((person.name ?: "?").take(1).uppercase(), color = c.textTertiary)
        }
    }
}

/** Shared by the sheets below: a title with an optional explanatory line. */
@Composable
internal fun SheetHeader(title: String, subtitle: String?) {
    val c = PhosColors.current
    Column(modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 12.dp)) {
        Text(title, style = MaterialTheme.typography.titleMedium, color = c.textPrimary)
        if (subtitle != null) {
            Spacer(Modifier.height(4.dp))
            Text(
                text = subtitle,
                style = MaterialTheme.typography.bodySmall,
                color = c.textSecondary,
            )
        }
    }
}

/** Vertically centred row used for the sheets' empty and loading states. */
@Composable
internal fun CenteredNotice(text: String) {
    val c = PhosColors.current
    Row(
        modifier = Modifier.fillMaxWidth().padding(24.dp),
        horizontalArrangement = Arrangement.Center,
        verticalAlignment = Alignment.CenterVertically,
    ) {
        PhosMonoText(text, color = c.textSecondary, maxLines = 2)
    }
}
