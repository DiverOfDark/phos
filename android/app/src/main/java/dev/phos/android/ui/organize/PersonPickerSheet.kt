package dev.phos.android.ui.organize

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
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
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import dev.phos.android.domain.model.Person

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

    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .navigationBarsPadding()
                .padding(horizontal = 16.dp),
        ) {
            Text(title, style = MaterialTheme.typography.titleMedium)
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
            // Offered only when nothing is named exactly this, so the row cannot be
            // used to make a second "Anna" by accident.
            val canCreate = trimmed.isNotEmpty() &&
                people.none { it.name.equals(trimmed, ignoreCase = true) }

            if (isLoading) {
                Row(
                    modifier = Modifier.fillMaxWidth().padding(24.dp),
                    horizontalArrangement = Arrangement.Center,
                ) {
                    CircularProgressIndicator()
                }
            } else {
                LazyColumn(modifier = Modifier.heightIn(max = 420.dp)) {
                    if (canCreate) {
                        item {
                            ListItem(
                                leadingContent = { Icon(Icons.Default.Add, contentDescription = null) },
                                headlineContent = { Text("Create \"$trimmed\"") },
                                supportingContent = { Text("New person, assigned right away") },
                                modifier = Modifier.clickable { onCreate(trimmed) },
                            )
                            HorizontalDivider()
                        }
                    }

                    items(matches, key = { it.id }) { person ->
                        ListItem(
                            leadingContent = {
                                Icon(Icons.Default.Person, contentDescription = null)
                            },
                            headlineContent = { Text(person.name ?: "Unnamed") },
                            supportingContent = {
                                Text("${person.shotCount} shots · ${person.faceCount} faces")
                            },
                            modifier = Modifier.clickable { onPick(person.id) },
                        )
                    }

                    if (matches.isEmpty() && !canCreate) {
                        item {
                            Text(
                                "No people yet.",
                                style = MaterialTheme.typography.bodyMedium,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                                modifier = Modifier.padding(vertical = 24.dp),
                            )
                        }
                    }
                }
            }

            Spacer(Modifier.height(8.dp))
        }
    }
}

/** Shared by the sheets below: a row that is only tappable when it makes sense. */
@Composable
internal fun SheetHeader(title: String, subtitle: String?) {
    Column(modifier = Modifier.fillMaxWidth()) {
        Text(title, style = MaterialTheme.typography.titleMedium)
        if (subtitle != null) {
            Spacer(Modifier.height(4.dp))
            Text(
                subtitle,
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        Spacer(Modifier.height(12.dp))
    }
}

/** Vertically centred row used for the sheets' empty and loading states. */
@Composable
internal fun CenteredNotice(text: String) {
    Row(
        modifier = Modifier.fillMaxWidth().padding(24.dp),
        horizontalArrangement = Arrangement.Center,
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            text,
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}
