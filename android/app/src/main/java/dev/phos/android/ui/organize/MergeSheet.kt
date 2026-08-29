package dev.phos.android.ui.organize

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
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.border
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
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
import dev.phos.android.domain.model.SimilarShot
import dev.phos.android.ui.common.PhosColors
import dev.phos.android.ui.common.PhosSheet
import dev.phos.android.ui.common.PhosSheetHeader
import dev.phos.android.ui.common.PhosSheetRow

/**
 * "Is one of these the same picture?"
 *
 * Lists what the server considers visually similar, nearest first, and folds the
 * chosen one into the shot on screen: its files move across and it disappears.
 *
 * The confirmation is not ceremony. A merge deletes a shot, it cannot be undone from
 * here, and the two candidates often belong to *different people* — which is exactly
 * the case where a mis-tap is expensive, so the dialog names who the loser belongs to.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun MergeSheet(
    candidates: List<SimilarShot>,
    isLoading: Boolean,
    isBusy: Boolean,
    thumbnailUrl: (path: String) -> String,
    onDismiss: () -> Unit,
    onMerge: (sourceShotId: String) -> Unit,
) {
    var pending by remember { mutableStateOf<SimilarShot?>(null) }
    val c = PhosColors.current

    PhosSheet(onDismiss = onDismiss) {
        PhosSheetHeader(
            title = "Merge into this shot",
            subtitle = "The shot you pick is absorbed and deleted.",
            onDismiss = onDismiss,
        )
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .navigationBarsPadding(),
        ) {
            when {
                isLoading -> CenteredNotice("finding similar shots…")

                candidates.isEmpty() -> CenteredNotice("nothing similar enough to merge")

                else -> LazyColumn(modifier = Modifier.heightIn(max = 420.dp)) {
                    items(candidates, key = { it.id }) { candidate ->
                        PhosSheetRow(
                            label = candidate.personName ?: "unsorted",
                            // Distance is the whole basis of the suggestion, so it
                            // is shown rather than hidden behind "similar".
                            meta = "${candidate.fileCount} file(s) · " +
                                if (candidate.distance == 0) "identical" else "Δ ${candidate.distance}",
                            metaColor = if (candidate.distance == 0) c.ready else c.textTertiary,
                            leading = {
                                AsyncImage(
                                    model = thumbnailUrl(candidate.thumbnailUrl),
                                    contentDescription = null,
                                    contentScale = ContentScale.Crop,
                                    modifier = Modifier
                                        .size(48.dp)
                                        .clip(RoundedCornerShape(4.dp))
                                        .border(1.dp, c.line, RoundedCornerShape(4.dp)),
                                )
                            },
                            onClick = { if (!isBusy) pending = candidate },
                        )
                    }
                }
            }

            Spacer(Modifier.height(8.dp))
        }
    }

    val target = pending
    if (target != null) {
        AlertDialog(
            onDismissRequest = { pending = null },
            containerColor = c.overlay,
            title = { Text("Merge and delete?") },
            text = {
                Text(
                    "The shot from ${target.personName ?: "an unassigned person"} loses " +
                        "its ${target.fileCount} file(s) to this one and is deleted. " +
                        "This can't be undone."
                )
            },
            confirmButton = {
                TextButton(
                    onClick = {
                        pending = null
                        onMerge(target.id)
                    },
                ) {
                    Text("Merge", color = c.error)
                }
            },
            dismissButton = {
                TextButton(onClick = { pending = null }) { Text("Cancel") }
            },
        )
    }
}
