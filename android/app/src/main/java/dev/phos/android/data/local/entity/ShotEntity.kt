package dev.phos.android.data.local.entity

import androidx.room.Entity
import androidx.room.Index
import androidx.room.PrimaryKey

@Entity(
    tableName = "shots",
    indices = [Index("primaryPersonId")]
)
data class ShotEntity(
    @PrimaryKey val id: String,
    val timestamp: String?,
    val primaryPersonId: String?,
    val reviewStatus: String?,
    val updatedAt: String?,
)
