package dev.phos.android.ui.browser

import android.view.ViewGroup
import android.widget.FrameLayout
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.pager.HorizontalPager
import androidx.compose.foundation.pager.VerticalPager
import androidx.compose.foundation.pager.rememberPagerState
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.media3.common.MediaItem
import androidx.media3.common.Player
import androidx.media3.datasource.okhttp.OkHttpDataSource
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.exoplayer.source.DefaultMediaSourceFactory
import androidx.media3.ui.PlayerView
import coil3.compose.AsyncImage
import dev.phos.android.data.local.entity.FileEntity
import dev.phos.android.ui.common.FullScreenLoading
import kotlinx.coroutines.flow.distinctUntilChanged
import me.saket.telephoto.zoomable.coil3.ZoomableAsyncImage
import me.saket.telephoto.zoomable.rememberZoomableImageState
import me.saket.telephoto.zoomable.rememberZoomableState
import me.saket.telephoto.zoomable.zoomable

@Composable
fun BrowserScreen(
    onBack: () -> Unit,
    viewModel: BrowserViewModel = hiltViewModel(),
) {
    val uiState by viewModel.uiState.collectAsState()

    if (uiState.isLoading) {
        FullScreenLoading("Loading shots...")
        return
    }

    if (uiState.shots.isEmpty()) {
        Box(
            modifier = Modifier.fillMaxSize(),
            contentAlignment = Alignment.Center,
        ) {
            Column(horizontalAlignment = Alignment.CenterHorizontally) {
                Text("No shots found", style = MaterialTheme.typography.titleMedium)
                Spacer(modifier = Modifier.height(8.dp))
                Text(
                    uiState.error ?: "This person has no photos or videos.",
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
        return
    }

    var showOverlay by remember { mutableStateOf(true) }
    var currentFileIndex by remember { mutableStateOf(uiState.initialFileIndex) }

    val verticalPagerState = rememberPagerState(
        initialPage = uiState.initialShotIndex,
        pageCount = { uiState.shots.size },
    )

    // Track shot changes for position persistence and prefetch
    LaunchedEffect(verticalPagerState) {
        snapshotFlow { verticalPagerState.currentPage }
            .distinctUntilChanged()
            .collect { shotIndex ->
                currentFileIndex = 0
                viewModel.onShotChanged(shotIndex, 0)
            }
    }

    Box(
        modifier = Modifier
            .fillMaxSize()
            .background(Color.Black),
    ) {
        // Vertical pager (shots)
        VerticalPager(
            state = verticalPagerState,
            beyondViewportPageCount = 1,
            modifier = Modifier.fillMaxSize(),
        ) { shotIndex ->
            val shot = uiState.shots[shotIndex]

            if (shot.files.isEmpty()) {
                Box(
                    modifier = Modifier.fillMaxSize(),
                    contentAlignment = Alignment.Center,
                ) {
                    Text("No files", color = Color.White)
                }
            } else {
                val horizontalPagerState = rememberPagerState(
                    initialPage = if (shotIndex == uiState.initialShotIndex) {
                        uiState.initialFileIndex.coerceIn(0, maxOf(0, shot.files.size - 1))
                    } else 0,
                    pageCount = { shot.files.size },
                )

                // Track file changes
                LaunchedEffect(horizontalPagerState) {
                    snapshotFlow { horizontalPagerState.currentPage }
                        .distinctUntilChanged()
                        .collect { fileIndex ->
                            currentFileIndex = fileIndex
                            viewModel.onShotChanged(shotIndex, fileIndex)
                        }
                }

                // Horizontal pager (files/variants)
                HorizontalPager(
                    state = horizontalPagerState,
                    beyondViewportPageCount = 1,
                    modifier = Modifier.fillMaxSize(),
                ) { fileIndex ->
                    val file = shot.files[fileIndex]
                    val isVideo = viewModel.isVideo(file)
                    MediaPage(
                        file = file,
                        thumbnailUrl = viewModel.buildThumbnailUrl(file.id, 320),
                        previewUrl = viewModel.buildThumbnailUrl(file.id, 1080),
                        originalUrl = viewModel.buildOriginalUrl(file.id),
                        isVideo = isVideo,
                        okHttpClient = viewModel.getOkHttpClient(),
                        onTap = { showOverlay = !showOverlay },
                    )
                }

                // File indicator dots (if multiple files)
                if (shot.files.size > 1) {
                    AnimatedVisibility(
                        visible = showOverlay,
                        enter = fadeIn(),
                        exit = fadeOut(),
                        modifier = Modifier
                            .align(Alignment.BottomCenter)
                            .padding(bottom = 80.dp),
                    ) {
                        Row(
                            horizontalArrangement = Arrangement.Center,
                        ) {
                            repeat(shot.files.size) { index ->
                                Box(
                                    modifier = Modifier
                                        .size(if (index == horizontalPagerState.currentPage) 8.dp else 6.dp)
                                        .padding(2.dp)
                                        .background(
                                            if (index == horizontalPagerState.currentPage)
                                                Color.White
                                            else Color.White.copy(alpha = 0.5f),
                                            shape = MaterialTheme.shapes.small,
                                        )
                                )
                            }
                        }
                    }
                }
            }
        }

        // Overlay
        AnimatedVisibility(
            visible = showOverlay,
            enter = fadeIn(),
            exit = fadeOut(),
        ) {
            MediaOverlay(
                personName = uiState.personName,
                shotIndex = verticalPagerState.currentPage,
                shotCount = uiState.shots.size,
                fileIndex = currentFileIndex,
                fileCount = if (uiState.shots.isNotEmpty()) {
                    uiState.shots[verticalPagerState.currentPage].files.size
                } else 0,
                timestamp = if (uiState.shots.isNotEmpty()) {
                    uiState.shots[verticalPagerState.currentPage].shot.timestamp
                } else null,
                onBack = onBack,
            )
        }
    }
}

@Composable
private fun MediaPage(
    file: FileEntity,
    thumbnailUrl: String,
    previewUrl: String,
    originalUrl: String,
    isVideo: Boolean,
    okHttpClient: okhttp3.OkHttpClient,
    onTap: () -> Unit,
) {
    if (isVideo) {
        VideoPage(
            thumbnailUrl = previewUrl,
            videoUrl = originalUrl,
            okHttpClient = okHttpClient,
            onTap = onTap,
        )
    } else {
        ImagePage(
            thumbnailUrl = thumbnailUrl,
            previewUrl = previewUrl,
            onTap = onTap,
        )
    }
}

@Composable
private fun ImagePage(
    thumbnailUrl: String,
    previewUrl: String,
    onTap: () -> Unit,
) {
    val zoomableState = rememberZoomableImageState(rememberZoomableState())

    Box(
        modifier = Modifier
            .fillMaxSize()
            .clickable(
                interactionSource = remember { MutableInteractionSource() },
                indication = null,
            ) { onTap() },
        contentAlignment = Alignment.Center,
    ) {
        // Show low-res thumbnail immediately as placeholder
        if (!zoomableState.isImageDisplayed) {
            AsyncImage(
                model = thumbnailUrl,
                contentDescription = null,
                contentScale = ContentScale.Fit,
                modifier = Modifier.fillMaxSize(),
            )
        }

        // Progressive loading: show zoomable high-res preview on top
        ZoomableAsyncImage(
            model = previewUrl,
            state = zoomableState,
            contentDescription = null,
            contentScale = ContentScale.Fit,
            modifier = Modifier.fillMaxSize(),
        )
    }
}

@Composable
@androidx.annotation.OptIn(androidx.media3.common.util.UnstableApi::class)
private fun VideoPage(
    thumbnailUrl: String,
    videoUrl: String,
    okHttpClient: okhttp3.OkHttpClient,
    onTap: () -> Unit,
) {
    var isPlaying by remember { mutableStateOf(false) }
    val context = LocalContext.current
    val zoomableState = rememberZoomableState()

    Box(
        modifier = Modifier
            .fillMaxSize(),
        contentAlignment = Alignment.Center,
    ) {
        if (isPlaying) {
            // ExoPlayer with OkHttp data source for auth
            val exoPlayer = remember {
                val dataSourceFactory = OkHttpDataSource.Factory(okHttpClient)
                val mediaSourceFactory = DefaultMediaSourceFactory(dataSourceFactory)
                ExoPlayer.Builder(context)
                    .setMediaSourceFactory(mediaSourceFactory)
                    .build()
                    .apply {
                        setMediaItem(MediaItem.fromUri(videoUrl))
                        prepare()
                        playWhenReady = true
                        repeatMode = Player.REPEAT_MODE_ONE
                    }
            }

            DisposableEffect(Unit) {
                onDispose {
                    exoPlayer.release()
                }
            }

            AndroidView(
                factory = {
                    PlayerView(it).apply {
                        player = exoPlayer
                        useController = false
                        layoutParams = FrameLayout.LayoutParams(
                            ViewGroup.LayoutParams.MATCH_PARENT,
                            ViewGroup.LayoutParams.MATCH_PARENT,
                        )
                    }
                },
                modifier = Modifier
                    .fillMaxSize()
                    .zoomable(zoomableState),
            )
        } else {
            // Poster frame with play button
            Box(
                modifier = Modifier
                    .fillMaxSize()
                    .clickable(
                        interactionSource = remember { MutableInteractionSource() },
                        indication = null,
                    ) { isPlaying = true },
                contentAlignment = Alignment.Center,
            ) {
                AsyncImage(
                    model = thumbnailUrl,
                    contentDescription = null,
                    contentScale = ContentScale.Fit,
                    modifier = Modifier.fillMaxSize(),
                )

                Box(
                    modifier = Modifier
                        .size(64.dp)
                        .background(Color.Black.copy(alpha = 0.5f), MaterialTheme.shapes.extraLarge),
                    contentAlignment = Alignment.Center,
                ) {
                    Icon(
                        imageVector = Icons.Default.PlayArrow,
                        contentDescription = "Play video",
                        tint = Color.White,
                        modifier = Modifier.size(40.dp),
                    )
                }
            }
        }
    }
}

@Composable
private fun MediaOverlay(
    personName: String?,
    shotIndex: Int,
    shotCount: Int,
    fileIndex: Int,
    fileCount: Int,
    timestamp: String?,
    onBack: () -> Unit,
) {
    Box(modifier = Modifier.fillMaxSize()) {
        // Top gradient + info
        Box(
            modifier = Modifier
                .fillMaxWidth()
                .background(
                    Brush.verticalGradient(
                        colors = listOf(Color.Black.copy(alpha = 0.6f), Color.Transparent),
                    )
                )
                .statusBarsPadding()
                .padding(8.dp),
        ) {
            Row(
                verticalAlignment = Alignment.CenterVertically,
                modifier = Modifier.fillMaxWidth(),
            ) {
                IconButton(onClick = onBack) {
                    Icon(
                        Icons.AutoMirrored.Filled.ArrowBack,
                        contentDescription = "Back",
                        tint = Color.White,
                    )
                }

                Spacer(modifier = Modifier.width(8.dp))

                Column(modifier = Modifier.weight(1f)) {
                    Text(
                        text = personName ?: "Unknown",
                        style = MaterialTheme.typography.titleMedium,
                        color = Color.White,
                    )
                    Text(
                        text = "${shotIndex + 1} / $shotCount",
                        style = MaterialTheme.typography.bodySmall,
                        color = Color.White.copy(alpha = 0.8f),
                    )
                }

                if (fileCount > 1) {
                    Text(
                        text = "Variant ${fileIndex + 1}/$fileCount",
                        style = MaterialTheme.typography.bodySmall,
                        color = Color.White.copy(alpha = 0.8f),
                        modifier = Modifier.padding(end = 8.dp),
                    )
                }
            }
        }

        // Bottom gradient + timestamp
        if (timestamp != null) {
            Box(
                modifier = Modifier
                    .fillMaxWidth()
                    .align(Alignment.BottomCenter)
                    .background(
                        Brush.verticalGradient(
                            colors = listOf(Color.Transparent, Color.Black.copy(alpha = 0.6f)),
                        )
                    )
                    .navigationBarsPadding()
                    .padding(16.dp),
            ) {
                Text(
                    text = timestamp,
                    style = MaterialTheme.typography.bodySmall,
                    color = Color.White.copy(alpha = 0.8f),
                )
            }
        }
    }
}
