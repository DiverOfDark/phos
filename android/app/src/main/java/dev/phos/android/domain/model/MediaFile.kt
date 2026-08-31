package dev.phos.android.domain.model

data class MediaFile(
    val id: String,
    val shotId: String,
    val mimeType: String?,
    val isOriginal: Boolean,
    val fileSize: Long?,
    val thumbnailUrl: String?,
    /** A workflow made this file, not a camera. The screens say so. */
    val synthetic: Boolean = false,
)
