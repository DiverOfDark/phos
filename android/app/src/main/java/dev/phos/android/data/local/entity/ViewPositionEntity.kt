package dev.phos.android.data.local.entity

import androidx.room.Entity
import androidx.room.PrimaryKey

@Entity(tableName = "view_positions")
data class ViewPositionEntity(
    @PrimaryKey val personId: String,
    val shotIndex: Int,
    val fileIndex: Int,
)
