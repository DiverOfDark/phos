package dev.phos.android.ui.organize

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.CallSplit
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material.icons.filled.Merge
import androidx.compose.material.icons.filled.Person
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.ListItem
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.unit.dp

/** What the user can do to the shot they are looking at. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ShotActionsSheet(
    fileCount: Int,
    currentFileIsOriginal: Boolean,
    onDismiss: () -> Unit,
    onMoveToPerson: () -> Unit,
    onSplit: () -> Unit,
    onMerge: () -> Unit,
    onDeleteVariant: () -> Unit,
    onDeleteShot: () -> Unit,
) {
    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .navigationBarsPadding()
                .padding(bottom = 8.dp),
        ) {
            Action(
                icon = Icons.Default.Person,
                label = "Move to another person",
                detail = "Reassign this shot. Its review status is left alone.",
                onClick = onMoveToPerson,
            )

            // A one-file shot has nothing to split off, and the server would reject
            // the request anyway — so the action is not offered rather than offered
            // and then refused.
            if (fileCount > 1) {
                Action(
                    icon = Icons.Default.CallSplit,
                    label = "Split shot",
                    detail = "Move some of the $fileCount files into a shot of their own.",
                    onClick = onSplit,
                )
            }

            Action(
                icon = Icons.Default.Merge,
                label = "Merge with a similar shot",
                detail = "Absorb a duplicate into this one.",
                onClick = onMerge,
            )

            HorizontalDivider(modifier = Modifier.padding(vertical = 8.dp))

            // Only the non-original copies can go individually; deleting the original
            // means deleting the shot, which is the action below.
            if (fileCount > 1 && !currentFileIsOriginal) {
                Action(
                    icon = Icons.Default.Delete,
                    label = "Delete this variant",
                    detail = "Removes the file you are looking at, keeps the shot.",
                    destructive = true,
                    onClick = onDeleteVariant,
                )
            }

            Action(
                icon = Icons.Default.Delete,
                label = "Delete shot",
                detail = if (fileCount > 1) {
                    "Deletes all $fileCount files. Can't be undone."
                } else {
                    "Deletes the file. Can't be undone."
                },
                destructive = true,
                onClick = onDeleteShot,
            )

            Spacer(Modifier.height(8.dp))
        }
    }
}

@Composable
private fun Action(
    icon: ImageVector,
    label: String,
    detail: String,
    destructive: Boolean = false,
    onClick: () -> Unit,
) {
    val tint = if (destructive) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurface
    ListItem(
        leadingContent = { Icon(icon, contentDescription = null, tint = tint) },
        headlineContent = { Text(label, color = tint) },
        supportingContent = { Text(detail, style = MaterialTheme.typography.bodySmall) },
        colors = ListItemDefaults.colors(),
        modifier = Modifier.clickable(onClick = onClick),
    )
}
