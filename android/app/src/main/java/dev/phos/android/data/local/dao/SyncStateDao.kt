package dev.phos.android.data.local.dao

import androidx.room.Dao
import androidx.room.Query
import androidx.room.Upsert
import dev.phos.android.data.local.entity.SyncStateEntity
import dev.phos.android.data.local.entity.ViewPositionEntity

@Dao
interface SyncStateDao {
    @Query("SELECT * FROM sync_state WHERE id = 'default'")
    suspend fun getSyncState(): SyncStateEntity?

    @Upsert
    suspend fun upsertSyncState(state: SyncStateEntity)

    @Query("SELECT * FROM view_positions WHERE personId = :personId")
    suspend fun getViewPosition(personId: String): ViewPositionEntity?

    @Upsert
    suspend fun upsertViewPosition(position: ViewPositionEntity)
}
