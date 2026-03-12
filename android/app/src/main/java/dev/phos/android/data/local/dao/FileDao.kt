package dev.phos.android.data.local.dao

import androidx.room.Dao
import androidx.room.Query
import androidx.room.Upsert
import dev.phos.android.data.local.entity.FileEntity
import kotlinx.coroutines.flow.Flow

@Dao
interface FileDao {
    @Query("SELECT * FROM files WHERE shotId = :shotId ORDER BY isOriginal DESC")
    fun getFilesByShotId(shotId: String): Flow<List<FileEntity>>

    @Query("SELECT * FROM files WHERE shotId = :shotId ORDER BY isOriginal DESC")
    suspend fun getFilesByShotIdOnce(shotId: String): List<FileEntity>

    @Query("SELECT * FROM files WHERE id = :id")
    suspend fun getById(id: String): FileEntity?

    @Upsert
    suspend fun upsertAll(files: List<FileEntity>)

    @Query("DELETE FROM files WHERE id = :id")
    suspend fun deleteById(id: String)

    @Query("DELETE FROM files WHERE shotId = :shotId")
    suspend fun deleteByShotId(shotId: String)
}
