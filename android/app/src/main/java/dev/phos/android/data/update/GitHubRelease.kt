package dev.phos.android.data.update

import com.fasterxml.jackson.annotation.JsonProperty

data class GitHubRelease(
    @JsonProperty("tag_name") val tagName: String = "",
    val name: String = "",
    val body: String = "",
    @JsonProperty("html_url") val htmlUrl: String = "",
    val assets: List<GitHubAsset> = emptyList(),
)

data class GitHubAsset(
    val name: String = "",
    @JsonProperty("browser_download_url") val browserDownloadUrl: String = "",
    val size: Long = 0,
    @JsonProperty("content_type") val contentType: String = "",
)
