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
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.unit.dp
import coil3.compose.AsyncImage
import dev.phos.android.domain.model.Face

/**
 * The faces detected in the shot on screen.
 *
 * The browser shows the photo, not boxes over it: the image is zoomable and
 * pannable, and an overlay that has to follow that transform is a lot of
 * machinery for a panel opened occasionally. A list of crops answers the same two
 * questions — who did the clustering think this is, and is it even a face — and
 * each row leads to the sheet that fixes either one.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun FacesSheet(
    faces: List<Face>,
    isLoading: Boolean,
    faceThumbnailUrl: (faceId: String) -> String,
    onDismiss: () -> Unit,
    onPick: (Face) -> Unit,
) {
    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .navigationBarsPadding()
                .padding(horizontal = 16.dp),
        ) {
            Text(
                text = when {
                    isLoading -> "Faces"
                    faces.isEmpty() -> "No faces here"
                    faces.size == 1 -> "1 face in this shot"
                    else -> "${faces.size} faces in this shot"
                },
                style = MaterialTheme.typography.titleMedium,
            )
            Spacer(Modifier.height(4.dp))
            Text(
                text = if (faces.isEmpty() && !isLoading) {
                    "The detector found nobody in this shot."
                } else {
                    "Tap a face to say who it is, or to drop a wrong detection."
                },
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Spacer(Modifier.height(8.dp))

            if (isLoading) {
                ListItem(
                    leadingContent = {
                        CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
                    },
                    headlineContent = { Text("Looking for faces…") },
                )
            }

            LazyColumn(modifier = Modifier.heightIn(max = 400.dp)) {
                items(faces, key = { it.id }) { face ->
                    ListItem(
                        leadingContent = {
                            AsyncImage(
                                model = faceThumbnailUrl(face.id),
                                contentDescription = null,
                                contentScale = ContentScale.Crop,
                                modifier = Modifier
                                    .size(48.dp)
                                    .clip(RoundedCornerShape(8.dp)),
                            )
                        },
                        headlineContent = { Text(face.personName ?: "Unassigned") },
                        supportingContent = {
                            Text(
                                if (face.personId == null) {
                                    "Nobody claimed this face yet"
                                } else {
                                    "Tap to correct or remove"
                                }
                            )
                        },
                        modifier = Modifier.clickable { onPick(face) },
                    )
                }
            }

            Spacer(Modifier.height(8.dp))
        }
    }
}
