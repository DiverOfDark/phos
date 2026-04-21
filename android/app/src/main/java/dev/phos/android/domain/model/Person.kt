package dev.phos.android.domain.model

data class Person(
    val id: String,
    val name: String?,
    val faceCount: Int,
    val thumbnailUrl: String?,
    val shotCount: Int,
    val pendingCount: Int,
    val updatedAt: String?,
    val coverShotThumbnailUrl: String?,
)
