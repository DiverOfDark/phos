package dev.phos.android.ui.organize

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.Checkbox
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.unit.dp
import coil3.compose.AsyncImage
import dev.phos.android.domain.model.MediaFile

/**
 * "Which of these files don't belong here?"
 *
 * A shot is a group of files the scanner decided were the same picture. When it gets
 * that wrong, splitting moves the odd ones out into a shot of their own.
 *
 * The server refuses to move *every* file out — that would leave an empty shot — so
 * the confirm button enforces the same rule client-side and says why, rather than
 * letting the user assemble a selection that can only fail.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SplitSheet(
    files: List<MediaFile>,
    thumbnailUrl: (fileId: String) -> String,
    isBusy: Boolean,
    onDismiss: () -> Unit,
    onSplit: (fileIds: List<String>) -> Unit,
) {
    var selected by remember { mutableStateOf(emptySet<String>()) }

    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .navigationBarsPadding()
                .padding(horizontal = 16.dp),
        ) {
            SheetHeader(
                title = "Split shot",
                subtitle = "Pick the files to move out. They become a new, unreviewed " +
                    "shot; at least one file has to stay here.",
            )

            LazyVerticalGrid(
                columns = GridCells.Adaptive(minSize = 96.dp),
                horizontalArrangement = Arrangement.spacedBy(6.dp),
                verticalArrangement = Arrangement.spacedBy(6.dp),
                modifier = Modifier.heightIn(max = 340.dp),
            ) {
                items(files, key = { it.id }) { file ->
                    val isSelected = file.id in selected
                    Box(
                        modifier = Modifier
                            .aspectRatio(1f)
                            .clip(RoundedCornerShape(8.dp))
                            .clickable(enabled = !isBusy) {
                                selected = if (isSelected) selected - file.id else selected + file.id
                            },
                    ) {
                        AsyncImage(
                            model = thumbnailUrl(file.id),
                            contentDescription = null,
                            contentScale = ContentScale.Crop,
                            modifier = Modifier.fillMaxWidth().aspectRatio(1f),
                        )
                        Checkbox(
                            checked = isSelected,
                            // Null so the whole tile is the hit target — a checkbox
                            // that also handles taps double-fires on a fast tap.
                            onCheckedChange = null,
                            modifier = Modifier.align(Alignment.TopStart),
                        )
                        if (file.isOriginal) {
                            Text(
                                "original",
                                style = MaterialTheme.typography.labelSmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                                modifier = Modifier
                                    .align(Alignment.BottomEnd)
                                    .padding(4.dp),
                            )
                        }
                    }
                }
            }

            Spacer(Modifier.height(12.dp))

            val leavesOneBehind = selected.isNotEmpty() && selected.size < files.size
            Text(
                when {
                    selected.isEmpty() -> "Nothing selected."
                    !leavesOneBehind -> "Leave at least one file in this shot."
                    else -> "${selected.size} of ${files.size} files will move to a new shot."
                },
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )

            Spacer(Modifier.height(8.dp))

            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.End,
            ) {
                TextButton(onClick = onDismiss, enabled = !isBusy) { Text("Cancel") }
                Spacer(Modifier.height(8.dp))
                Button(
                    onClick = { onSplit(selected.toList()) },
                    enabled = leavesOneBehind && !isBusy,
                ) {
                    Text("Split")
                }
            }

            Spacer(Modifier.height(8.dp))
        }
    }
}
