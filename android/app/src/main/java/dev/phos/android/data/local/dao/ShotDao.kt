package dev.phos.android.data.local.dao

import androidx.room.Dao
import androidx.room.Query
import androidx.room.Upsert
import dev.phos.android.data.local.entity.ShotEntity
import kotlinx.coroutines.flow.Flow

@Dao
interface ShotDao {
    @Query("SELECT * FROM shots WHERE primaryPersonId = :personId ORDER BY timestamp DESC")
    fun getShotsByPersonId(personId: String): Flow<List<ShotEntity>>

    @Query("SELECT * FROM shots WHERE id = :id")
    suspend fun getById(id: String): ShotEntity?

    @Upsert
    suspend fun upsertAll(shots: List<ShotEntity>)

    @Query("DELETE FROM shots WHERE id = :id")
    suspend fun deleteById(id: String)

    @Query("DELETE FROM shots WHERE primaryPersonId = :personId")
    suspend fun deleteByPersonId(personId: String)
}
