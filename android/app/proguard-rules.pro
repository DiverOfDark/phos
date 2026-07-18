# Retrofit
-keepattributes Signature, InnerClasses, EnclosingMethod
-keepattributes RuntimeVisibleAnnotations, RuntimeVisibleParameterAnnotations
-keepclassmembers,allowshrinking,allowobfuscation interface * {
    @retrofit2.http.* <methods>;
}

# kotlinx.serialization
-keepattributes *Annotation*, InnerClasses
-dontnote kotlinx.serialization.AnnotationsKt

-keepclassmembers class dev.phos.android.data.remote.** {
    *;
}

# GitHub API models (Jackson deserialization)
-keepclassmembers class dev.phos.android.data.update.** {
    *;
}

# Jackson TypeReference (used by reified readValue<T>) resolves its generic
# supertype reflectively; keep subclasses so R8 retains their Signature
# attribute (-keepattributes only applies to classes matched by a keep rule).
-keep,allowobfuscation,allowshrinking class * extends com.fasterxml.jackson.core.type.TypeReference

# Room
-keep class * extends androidx.room.RoomDatabase
-dontwarn androidx.room.paging.**

# AppAuth
-keep class net.openid.appauth.** { *; }
