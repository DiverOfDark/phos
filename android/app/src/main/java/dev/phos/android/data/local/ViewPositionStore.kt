package dev.phos.android.data.local

import android.content.SharedPreferences
import javax.inject.Inject
import javax.inject.Named
import javax.inject.Singleton

data class ViewPosition(
    val shotIndex: Int,
    val fileIndex: Int,
)

@Singleton
class ViewPositionStore @Inject constructor(
    @Named("auth") private val prefs: SharedPreferences,
) {
    fun getViewPosition(personId: String): ViewPosition? {
        val shotKey = shotKey(personId)
        val fileKey = fileKey(personId)
        if (!prefs.contains(shotKey)) return null
        return ViewPosition(
            shotIndex = prefs.getInt(shotKey, 0),
            fileIndex = prefs.getInt(fileKey, 0),
        )
    }

    fun saveViewPosition(personId: String, shotIndex: Int, fileIndex: Int) {
        prefs.edit()
            .putInt(shotKey(personId), shotIndex)
            .putInt(fileKey(personId), fileIndex)
            .apply()
    }

    private fun shotKey(personId: String) = "vp_${personId}_shot"
    private fun fileKey(personId: String) = "vp_${personId}_file"
}
