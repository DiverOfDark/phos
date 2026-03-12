package dev.phos.android.data.local

import androidx.room.Database
import androidx.room.RoomDatabase
import dev.phos.android.data.local.dao.FileDao
import dev.phos.android.data.local.dao.PersonDao
import dev.phos.android.data.local.dao.ShotDao
import dev.phos.android.data.local.dao.SyncStateDao
import dev.phos.android.data.local.entity.FileEntity
import dev.phos.android.data.local.entity.PersonEntity
import dev.phos.android.data.local.entity.ShotEntity
import dev.phos.android.data.local.entity.SyncStateEntity
import dev.phos.android.data.local.entity.ViewPositionEntity

@Database(
    entities = [
        PersonEntity::class,
        ShotEntity::class,
        FileEntity::class,
        SyncStateEntity::class,
        ViewPositionEntity::class,
    ],
    version = 1,
    exportSchema = false,
)
abstract class PhosDatabase : RoomDatabase() {
    abstract fun personDao(): PersonDao
    abstract fun shotDao(): ShotDao
    abstract fun fileDao(): FileDao
    abstract fun syncStateDao(): SyncStateDao
}
