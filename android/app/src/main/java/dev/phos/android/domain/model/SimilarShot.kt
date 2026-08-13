package dev.phos.android.domain.model

/**
 * A shot the server thinks looks like the one being viewed — a merge candidate.
 *
 * The API groups these by the person each candidate is currently assigned to;
 * [personName] carries that grouping down flattened, because on a phone a single
 * distance-ordered list is easier to pick from than a set of sections, and who the
 * candidate currently belongs to is the one fact that makes a merge surprising.
 */
data class SimilarShot(
    val id: String,
    val thumbnailUrl: String,
    val fileCount: Int,
    /** Perceptual-hash hamming distance. Lower is more alike; 0 is pixel-identical. */
    val distance: Int,
    /** Who this candidate is currently assigned to, null when it is unassigned. */
    val personName: String?,
    val reviewStatus: String?,
)
