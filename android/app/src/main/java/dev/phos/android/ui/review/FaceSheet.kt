package dev.phos.android.ui.review

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
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.material3.ExperimentalMaterial3Api
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
import dev.phos.android.domain.model.Face
import dev.phos.android.domain.model.FaceSuggestion
import dev.phos.android.domain.model.Person
import dev.phos.android.ui.common.PhosAvatarBox
import dev.phos.android.ui.common.PhosColors
import dev.phos.android.ui.common.PhosLabel
import dev.phos.android.ui.common.PhosMonoText
import dev.phos.android.ui.common.PhosSearchField
import dev.phos.android.ui.common.PhosSheet
import dev.phos.android.ui.common.PhosSheetHeader
import dev.phos.android.ui.common.PhosSheetRow
import dev.phos.android.ui.common.SignalDot

/**
 * "Who is this?" for one face.
 *
 * The suggestions sit above the full list because they are the reason reviewing on a
 * phone is bearable: the server has already compared this face's embedding against
 * every known person, and the right answer is nearly always the first row. Scrolling
 * a list of hundreds is the fallback, not the path.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun FaceSheet(
    face: Face,
    suggestions: List<FaceSuggestion>,
    suggestionsLoading: Boolean,
    people: List<Person>,
    peopleLoading: Boolean,
    thumbnailUrl: (path: String) -> String,
    onDismiss: () -> Unit,
    onAssign: (personId: String, personName: String?) -> Unit,
    onCreate: (name: String) -> Unit,
    onDeleteFace: () -> Unit,
) {
    var query by remember { mutableStateOf("") }
    val c = PhosColors.current

    PhosSheet(onDismiss = onDismiss) {
        PhosSheetHeader(
            title = "Who is this?",
            subtitle = face.personName?.let { "currently $it" } ?: "unassigned",
            onDismiss = onDismiss,
            // For the boxes the detector got wrong — a face on a poster, a pattern
            // on a shirt. Deleting the box is not deleting the photo.
            destructiveLabel = "delete face",
            onDestructive = onDeleteFace,
        )

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
            val canCreate = trimmed.isNotEmpty() &&
                people.none { it.name.equals(trimmed, ignoreCase = true) }
            // Suggestions are hidden once the user starts typing: they have told us
            // who it is, and a "did you mean" list under a search box is noise.
            val showSuggestions = trimmed.isEmpty()

            LazyColumn(modifier = Modifier.heightIn(max = 420.dp)) {
                if (showSuggestions) {
                    if (suggestionsLoading) {
                        item {
                            Row(
                                modifier = Modifier.fillMaxWidth().padding(16.dp),
                                verticalAlignment = Alignment.CenterVertically,
                                horizontalArrangement = Arrangement.spacedBy(8.dp),
                            ) {
                                SignalDot(color = c.building, size = 6.dp, pulsing = true)
                                PhosMonoText("looking for a match…", color = c.textSecondary)
                            }
                        }
                    }

                    if (suggestions.isNotEmpty()) {
                        item {
                            PhosLabel(
                                text = "Suggested",
                                modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
                            )
                        }
                    }

                    itemsIndexed(suggestions, key = { _, s -> "suggestion-${s.personId}" }) { index, suggestion ->
                        PhosSheetRow(
                            label = suggestion.personName ?: "unnamed cluster",
                            // The closest match is the one the reviewer confirms
                            // nine times out of ten; colour says which that is.
                            meta = "Δ %.2f".format(suggestion.distance),
                            metaColor = if (index == 0) c.ready else c.textTertiary,
                            leading = {
                                PhosAvatarBox(size = 32.dp) {
                                    if (suggestion.thumbnailUrl != null) {
                                        AsyncImage(
                                            model = thumbnailUrl(suggestion.thumbnailUrl),
                                            contentDescription = null,
                                            contentScale = ContentScale.Crop,
                                            modifier = Modifier.fillMaxSize(),
                                        )
                                    } else {
                                        PhosMonoText(
                                            (suggestion.personName ?: "?").take(1).uppercase(),
                                            color = c.textTertiary,
                                        )
                                    }
                                }
                            },
                            onClick = { onAssign(suggestion.personId, suggestion.personName) },
                        )
                    }
                }

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

                if (peopleLoading) {
                    item {
                        Row(
                            modifier = Modifier.fillMaxWidth().padding(16.dp),
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(8.dp),
                        ) {
                            SignalDot(color = c.building, size = 6.dp, pulsing = true)
                            PhosMonoText("loading people…", color = c.textSecondary)
                        }
                    }
                }

                items(matches, key = { "person-${it.id}" }) { person ->
                    PhosSheetRow(
                        label = person.name ?: "unnamed cluster",
                        meta = "${person.faceCount} faces",
                        leading = {
                            PhosAvatarBox(size = 32.dp) {
                                PhosMonoText((person.name ?: "?").take(1).uppercase(), color = c.textTertiary)
                            }
                        },
                        onClick = { onAssign(person.id, person.name) },
                    )
                }
            }

            Spacer(Modifier.height(8.dp))
        }
    }
}
