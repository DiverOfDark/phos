package dev.phos.android.domain.model

/** One entry in the review backlog. */
data class PendingShot(
    val id: String,
    val thumbnailUrl: String?,
    /** Who the clustering *thinks* this is, which is the guess under review. */
    val personName: String?,
    val fileCount: Int,
    val timestamp: String?,
    /** The shot's main file was made by a workflow, not a camera. */
    val synthetic: Boolean = false,
)

/**
 * A shot with everything the review screen needs to judge it.
 *
 * [width] and [height] matter as much as the pixels: face boxes come back in the
 * original image's pixel coordinates, and without the image's size there is no way
 * to place them over a scaled-to-fit view.
 */
data class ShotDetail(
    val id: String,
    val width: Int?,
    val height: Int?,
    val primaryPersonId: String?,
    val primaryPersonName: String?,
    val reviewStatus: String?,
    val timestamp: String?,
    val files: List<MediaFile>,
    val faces: List<Face>,
    /** Other people the server found in this shot, beyond the primary one. */
    val alsoContains: List<String>,
)

/**
 * One detected face, in the pixel coordinates of the file it was found in.
 *
 * [fileId] is not decoration: a shot can hold several files and the boxes only line
 * up with the one they were detected in, so the screen draws the faces belonging to
 * whichever file it is showing.
 */
data class Face(
    val id: String,
    val fileId: String,
    val personId: String?,
    val personName: String?,
    val x1: Float,
    val y1: Float,
    val x2: Float,
    val y2: Float,
)

/**
 * Who the server thinks a face is, by embedding distance.
 *
 * This is what makes reviewing on a phone fast: the right answer is usually the
 * first suggestion, one tap away, instead of somewhere in a list of hundreds.
 */
data class FaceSuggestion(
    val personId: String,
    val personName: String?,
    val distance: Float,
    val thumbnailUrl: String?,
)
