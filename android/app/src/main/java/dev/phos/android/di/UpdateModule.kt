package dev.phos.android.di

import dagger.Binds
import dagger.Module
import dagger.Provides
import dagger.hilt.InstallIn
import dagger.hilt.components.SingletonComponent
import dev.phos.android.BuildConfig
import dev.phos.android.update.ApkDownloader
import dev.phos.android.update.ApkInstaller
import dev.phos.android.update.ApkSignatures
import dev.phos.android.update.OkHttpApkDownloader
import dev.phos.android.update.PackageInstallerApkInstaller
import dev.phos.android.update.PackageManagerApkSignatures
import dev.phos.android.update.RunningVersion
import javax.inject.Singleton

/**
 * Wiring for the in-app updater.
 *
 * The three interfaces bound here are the ones that cannot exist on a JVM test
 * runner — reading certificates, driving `PackageInstaller`, and pulling tens of
 * megabytes over the network. Everything that *decides* anything sits on the other
 * side of them and is unit-tested with fakes.
 */
@Module
@InstallIn(SingletonComponent::class)
abstract class UpdateModule {

    @Binds
    @Singleton
    abstract fun bindApkDownloader(impl: OkHttpApkDownloader): ApkDownloader

    @Binds
    @Singleton
    abstract fun bindApkSignatures(impl: PackageManagerApkSignatures): ApkSignatures

    @Binds
    @Singleton
    abstract fun bindApkInstaller(impl: PackageInstallerApkInstaller): ApkInstaller

    companion object {

        /**
         * The identity of this build, read from `BuildConfig` in exactly one place.
         *
         * `versionCode` is the git commit count supplied by CI (`-PversionCode`), which
         * is what makes "the server is newer" answerable at all — before that, every
         * master build reported 1 and the comparison could only ever say "up to date".
         */
        @Provides
        @Singleton
        fun provideRunningVersion(): RunningVersion = RunningVersion(
            versionCode = BuildConfig.VERSION_CODE,
            versionName = BuildConfig.VERSION_NAME,
        )
    }
}
