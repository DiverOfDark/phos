package dev.phos.android.data.local.entity

import androidx.room.Entity
import androidx.room.PrimaryKey

@Entity(tableName = "sync_state")
data class SyncStateEntity(
    @PrimaryKey val id: String = "default",
    val syncToken: String?,
    val lastSyncTime: Long?,
)
