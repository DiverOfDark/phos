package dev.phos.android.data.local.entity

import androidx.room.Entity
import androidx.room.Index
import androidx.room.PrimaryKey

@Entity(
    tableName = "files",
    indices = [Index("shotId")]
)
data class FileEntity(
    @PrimaryKey val id: String,
    val shotId: String,
    val mimeType: String?,
    val isOriginal: Boolean,
    val fileSize: Long?,
    val width: Int?,
    val height: Int?,
    val durationMs: Long?,
    val thumbnailUrl: String?,
    val updatedAt: String?,
)
