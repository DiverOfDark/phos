package dev.phos.android.ui.organize

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import dev.phos.android.ui.common.PhosColors
import dev.phos.android.ui.common.PhosSheet
import dev.phos.android.ui.common.PhosSheetHeader
import dev.phos.android.ui.common.PhosSheetRow

/** What the user can do to the shot they are looking at. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ShotActionsSheet(
    fileCount: Int,
    currentFileIsOriginal: Boolean,
    onDismiss: () -> Unit,
    onMoveToPerson: () -> Unit,
    onFaces: () -> Unit,
    onSplit: () -> Unit,
    onMerge: () -> Unit,
    onDeleteVariant: () -> Unit,
    onDeleteShot: () -> Unit,
) {
    val c = PhosColors.current

    PhosSheet(onDismiss = onDismiss) {
        PhosSheetHeader(title = "Shot actions", onDismiss = onDismiss)

        Column(
            modifier = Modifier
                .fillMaxWidth()
                .navigationBarsPadding()
                .padding(bottom = 8.dp),
        ) {
            Action(
                label = "Move to another person",
                detail = "Reassign this shot. Its review status is left alone.",
                onClick = onMoveToPerson,
            )

            // The face-level fix, which is the one that sticks: reassigning the
            // shot moves this photo, correcting the face changes what the next
            // scan clusters — and a box the detector got wrong can go entirely.
            Action(
                label = "Faces in this shot",
                detail = "Say who a face is, or delete one the detector got wrong.",
                onClick = onFaces,
            )

            // A one-file shot has nothing to split off, and the server would reject
            // the request anyway — so the action is not offered rather than offered
            // and then refused.
            if (fileCount > 1) {
                Action(
                    label = "Split shot",
                    detail = "Move some of the $fileCount files into a shot of their own.",
                    onClick = onSplit,
                )
            }

            Action(
                label = "Merge with a similar shot",
                detail = "Absorb a duplicate into this one.",
                onClick = onMerge,
            )

            // Only the non-original copies can go individually; deleting the original
            // means deleting the shot, which is the action below.
            if (fileCount > 1 && !currentFileIsOriginal) {
                Action(
                    label = "Delete this variant",
                    detail = "Removes the file you are looking at, keeps the shot.",
                    color = c.error,
                    onClick = onDeleteVariant,
                )
            }

            Action(
                label = "Delete shot",
                detail = if (fileCount > 1) {
                    "Deletes all $fileCount files. Can't be undone."
                } else {
                    "Deletes the file. Can't be undone."
                },
                color = c.error,
                onClick = onDeleteShot,
            )

            Spacer(Modifier.height(8.dp))
        }
    }
}

/** One action: the verb, and one line saying exactly what it does to the data. */
@Composable
private fun Action(
    label: String,
    detail: String,
    onClick: () -> Unit,
    color: Color? = null,
) {
    val c = PhosColors.current
    PhosSheetRow(
        label = label,
        labelColor = color ?: c.textPrimary,
        onClick = onClick,
    )
    androidx.compose.material3.Text(
        text = detail,
        style = androidx.compose.material3.MaterialTheme.typography.bodySmall,
        color = c.textSecondary,
        modifier = Modifier.padding(start = 16.dp, end = 16.dp, top = 4.dp, bottom = 12.dp),
    )
}
