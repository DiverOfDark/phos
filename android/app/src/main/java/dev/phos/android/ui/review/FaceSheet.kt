package dev.phos.android.ui.review

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.AutoAwesome
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material.icons.filled.Person
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.unit.dp
import coil3.compose.AsyncImage
import dev.phos.android.domain.model.Face
import dev.phos.android.domain.model.FaceSuggestion
import dev.phos.android.domain.model.Person

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

    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .navigationBarsPadding()
                .padding(horizontal = 16.dp),
        ) {
            Text(
                text = face.personName?.let { "This face is currently $it" }
                    ?: "This face is unassigned",
                style = MaterialTheme.typography.titleMedium,
            )
            Spacer(Modifier.height(12.dp))

            OutlinedTextField(
                value = query,
                onValueChange = { query = it },
                label = { Text("Search or type a new name") },
                singleLine = true,
                modifier = Modifier.fillMaxWidth(),
            )
            Spacer(Modifier.height(8.dp))

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

            LazyColumn(modifier = Modifier.heightIn(max = 400.dp)) {
                if (showSuggestions) {
                    if (suggestionsLoading) {
                        item {
                            ListItem(
                                leadingContent = {
                                    CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
                                },
                                headlineContent = { Text("Looking for a match…") },
                            )
                        }
                    }

                    items(suggestions, key = { "suggestion-${it.personId}" }) { suggestion ->
                        ListItem(
                            leadingContent = {
                                if (suggestion.thumbnailUrl != null) {
                                    AsyncImage(
                                        model = thumbnailUrl(suggestion.thumbnailUrl),
                                        contentDescription = null,
                                        contentScale = ContentScale.Crop,
                                        modifier = Modifier.size(40.dp).clip(CircleShape),
                                    )
                                } else {
                                    Icon(Icons.Default.AutoAwesome, contentDescription = null)
                                }
                            },
                            headlineContent = { Text(suggestion.personName ?: "Unnamed") },
                            supportingContent = { Text("Suggested match") },
                            modifier = Modifier.clickable {
                                onAssign(suggestion.personId, suggestion.personName)
                            },
                        )
                    }

                    if (suggestions.isNotEmpty()) {
                        item { HorizontalDivider(modifier = Modifier.padding(vertical = 4.dp)) }
                    }
                }

                if (canCreate) {
                    item {
                        ListItem(
                            leadingContent = { Icon(Icons.Default.Add, contentDescription = null) },
                            headlineContent = { Text("Create \"$trimmed\"") },
                            modifier = Modifier.clickable { onCreate(trimmed) },
                        )
                    }
                }

                if (peopleLoading) {
                    item {
                        ListItem(
                            leadingContent = {
                                CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
                            },
                            headlineContent = { Text("Loading people…") },
                        )
                    }
                }

                items(matches, key = { "person-${it.id}" }) { person ->
                    ListItem(
                        leadingContent = { Icon(Icons.Default.Person, contentDescription = null) },
                        headlineContent = { Text(person.name ?: "Unnamed") },
                        supportingContent = { Text("${person.faceCount} faces") },
                        modifier = Modifier.clickable { onAssign(person.id, person.name) },
                    )
                }

                item {
                    HorizontalDivider(modifier = Modifier.padding(vertical = 4.dp))
                    // For the boxes the detector got wrong — a face on a poster, a
                    // pattern on a shirt. Deleting the box is not deleting the photo.
                    ListItem(
                        leadingContent = {
                            Icon(
                                Icons.Default.Delete,
                                contentDescription = null,
                                tint = MaterialTheme.colorScheme.error,
                            )
                        },
                        headlineContent = {
                            Text("Not a face", color = MaterialTheme.colorScheme.error)
                        },
                        supportingContent = { Text("Removes the box, keeps the photo") },
                        modifier = Modifier.clickable { onDeleteFace() },
                    )
                }
            }

            Spacer(Modifier.height(8.dp))
        }
    }
}
