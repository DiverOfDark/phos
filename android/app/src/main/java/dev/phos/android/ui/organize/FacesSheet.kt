package dev.phos.android.ui.organize

import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.unit.dp
import coil3.compose.AsyncImage
import dev.phos.android.domain.model.Face
import dev.phos.android.ui.common.PhosColors
import dev.phos.android.ui.common.PhosSheet
import dev.phos.android.ui.common.PhosSheetHeader
import dev.phos.android.ui.common.PhosSheetRow

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
    val c = PhosColors.current

    PhosSheet(onDismiss = onDismiss) {
        PhosSheetHeader(
            title = when {
                isLoading -> "Faces"
                faces.isEmpty() -> "No faces here"
                faces.size == 1 -> "1 face in this shot"
                else -> "${faces.size} faces in this shot"
            },
            subtitle = if (faces.isEmpty() && !isLoading) {
                "The detector found nobody in this shot."
            } else {
                "Tap a face to say who it is, or to drop a wrong detection."
            },
            onDismiss = onDismiss,
        )

        Column(
            modifier = Modifier
                .fillMaxWidth()
                .navigationBarsPadding(),
        ) {
            if (isLoading) CenteredNotice("looking for faces…")

            LazyColumn(modifier = Modifier.heightIn(max = 400.dp)) {
                items(faces, key = { it.id }) { face ->
                    PhosSheetRow(
                        label = face.personName ?: "unassigned",
                        labelColor = if (face.personId == null) c.signal else c.textPrimary,
                        meta = if (face.personId == null) "unknown" else "tap to correct",
                        leading = {
                            AsyncImage(
                                model = faceThumbnailUrl(face.id),
                                contentDescription = null,
                                contentScale = ContentScale.Crop,
                                modifier = Modifier
                                    .size(40.dp)
                                    .clip(RoundedCornerShape(4.dp))
                                    .border(1.dp, c.line, RoundedCornerShape(4.dp)),
                            )
                        },
                        onClick = { onPick(face) },
                    )
                }
            }

            Spacer(Modifier.height(8.dp))
        }
    }
}
