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
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.background
import androidx.compose.foundation.border
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
import androidx.compose.ui.draw.clip
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.unit.dp
import coil3.compose.AsyncImage
import dev.phos.android.domain.model.MediaFile
import dev.phos.android.ui.common.MonoSmall
import dev.phos.android.ui.common.PhosColors
import dev.phos.android.ui.common.PhosOutlinedButton
import dev.phos.android.ui.common.PhosPrimaryButton
import dev.phos.android.ui.common.PhosSheet
import dev.phos.android.ui.common.PhosSheetHeader

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
    val c = PhosColors.current

    PhosSheet(onDismiss = onDismiss) {
        PhosSheetHeader(
            title = "Split shot",
            subtitle = "At least one file has to stay here.",
            onDismiss = onDismiss,
        )
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .navigationBarsPadding()
                .padding(16.dp),
        ) {

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
                            .clip(RoundedCornerShape(2.dp))
                            .border(
                                1.dp,
                                if (isSelected) c.signal else c.line,
                                RoundedCornerShape(2.dp),
                            )
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
                        // A 12dp square, checked in signal amber: the whole tile is
                        // the hit target, so this only has to say what the state is.
                        Box(
                            modifier = Modifier
                                .align(Alignment.TopStart)
                                .padding(4.dp)
                                .size(14.dp)
                                .clip(RoundedCornerShape(2.dp))
                                .background(if (isSelected) c.signal else c.base.copy(alpha = 0.7f))
                                .border(
                                    1.dp,
                                    if (isSelected) c.signal else c.lineStrong,
                                    RoundedCornerShape(2.dp),
                                ),
                            contentAlignment = Alignment.Center,
                        ) {
                            if (isSelected) Text("✓", style = MonoSmall, color = c.signalFg)
                        }
                        if (file.isOriginal) {
                            Box(
                                modifier = Modifier
                                    .align(Alignment.BottomEnd)
                                    .padding(4.dp)
                                    .background(c.base, RoundedCornerShape(2.dp))
                                    .padding(horizontal = 3.dp),
                            ) {
                                Text("MASTER", style = MonoSmall, color = c.signal)
                            }
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
                color = if (selected.isNotEmpty() && !leavesOneBehind) c.degraded else c.textSecondary,
            )

            Spacer(Modifier.height(12.dp))

            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                PhosOutlinedButton(onClick = onDismiss, enabled = !isBusy) {
                    Text("Cancel", style = MaterialTheme.typography.bodySmall, color = c.textSecondary)
                }
                PhosPrimaryButton(
                    onClick = { onSplit(selected.toList()) },
                    enabled = leavesOneBehind && !isBusy,
                    modifier = Modifier.weight(1f),
                ) {
                    Text(
                        text = "Split into new shot",
                        style = MaterialTheme.typography.labelLarge,
                        color = c.signalFg,
                    )
                }
            }

            Spacer(Modifier.height(8.dp))
        }
    }
}
