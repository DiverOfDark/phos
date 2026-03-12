package dev.phos.android.data.local.entity

import androidx.room.Entity
import androidx.room.PrimaryKey

@Entity(tableName = "people")
data class PersonEntity(
    @PrimaryKey val id: String,
    val name: String?,
    val faceCount: Int,
    val thumbnailUrl: String?,
    val shotCount: Int,
    val pendingCount: Int,
    val updatedAt: String?,
    val coverShotThumbnailUrl: String?,
)
