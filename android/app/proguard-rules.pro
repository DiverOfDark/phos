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

# (The updater's wire model is generated into data.remote.model, which the rule
# above already covers; nothing in dev.phos.android.update is reflected on.)

# Jackson TypeReference (used by reified readValue<T>) resolves its generic
# supertype reflectively; keep subclasses so R8 retains their Signature
# attribute (-keepattributes only applies to classes matched by a keep rule).
-keep,allowobfuscation,allowshrinking class * extends com.fasterxml.jackson.core.type.TypeReference

# Room
-keep class * extends androidx.room.RoomDatabase
-dontwarn androidx.room.paging.**

# AppAuth
-keep class net.openid.appauth.** { *; }
