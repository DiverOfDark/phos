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
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material.icons.filled.Pause
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Slider
import androidx.compose.material3.SliderDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
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
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.distinctUntilChanged
import me.saket.telephoto.zoomable.coil3.ZoomableAsyncImage
import me.saket.telephoto.zoomable.rememberZoomableImageState
import me.saket.telephoto.zoomable.ZoomSpec
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
    var showDeleteConfirm by remember { mutableStateOf(false) }

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
                isOriginal = if (uiState.shots.isNotEmpty()) {
                    val shot = uiState.shots[verticalPagerState.currentPage]
                    currentFileIndex in shot.files.indices && shot.files[currentFileIndex].isOriginal
                } else true,
                timestamp = if (uiState.shots.isNotEmpty()) {
                    uiState.shots[verticalPagerState.currentPage].shot.timestamp
                } else null,
                onBack = onBack,
                onDeleteVariant = { showDeleteConfirm = true },
            )
        }

        if (showDeleteConfirm) {
            AlertDialog(
                onDismissRequest = { showDeleteConfirm = false },
                title = { Text("Delete variant?") },
                text = { Text("This will permanently delete this file variant from the server.") },
                confirmButton = {
                    TextButton(onClick = {
                        showDeleteConfirm = false
                        viewModel.deleteFile(verticalPagerState.currentPage, currentFileIndex)
                    }) {
                        Text("Delete", color = MaterialTheme.colorScheme.error)
                    }
                },
                dismissButton = {
                    TextButton(onClick = { showDeleteConfirm = false }) {
                        Text("Cancel")
                    }
                },
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
    val zoomableState = rememberZoomableImageState(
        rememberZoomableState(zoomSpec = ZoomSpec(maxZoomFactor = 4f))
    )

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
    var isStarted by remember { mutableStateOf(false) }
    val context = LocalContext.current
    val zoomableState = rememberZoomableState(zoomSpec = ZoomSpec(maxZoomFactor = 4f))

    Box(
        modifier = Modifier
            .fillMaxSize(),
        contentAlignment = Alignment.Center,
    ) {
        if (isStarted) {
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

            // Playback state
            var isPaused by remember { mutableStateOf(false) }
            var currentPosition by remember { mutableStateOf(0L) }
            var duration by remember { mutableStateOf(0L) }
            var showControls by remember { mutableStateOf(true) }
            var isSeeking by remember { mutableStateOf(false) }

            // Poll playback position
            LaunchedEffect(exoPlayer) {
                while (true) {
                    if (!isSeeking) {
                        currentPosition = exoPlayer.currentPosition
                    }
                    duration = exoPlayer.duration.coerceAtLeast(0L)
                    delay(200)
                }
            }

            // Player view
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
                    .zoomable(zoomableState)
                    .clickable(
                        interactionSource = remember { MutableInteractionSource() },
                        indication = null,
                    ) {
                        showControls = !showControls
                        onTap()
                    },
            )

            // Controls bar
            AnimatedVisibility(
                visible = showControls,
                enter = fadeIn(),
                exit = fadeOut(),
                modifier = Modifier.align(Alignment.BottomCenter),
            ) {
                Row(
                    modifier = Modifier
                        .fillMaxWidth()
                        .background(
                            Brush.verticalGradient(
                                listOf(Color.Transparent, Color.Black.copy(alpha = 0.6f)),
                            )
                        )
                        .navigationBarsPadding()
                        .padding(horizontal = 4.dp, vertical = 4.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    IconButton(onClick = {
                        isPaused = !isPaused
                        exoPlayer.playWhenReady = !isPaused
                    }) {
                        Icon(
                            imageVector = if (isPaused) Icons.Default.PlayArrow else Icons.Filled.Pause,
                            contentDescription = if (isPaused) "Play" else "Pause",
                            tint = Color.White,
                        )
                    }

                    Text(
                        text = formatDuration(currentPosition),
                        color = Color.White,
                        style = MaterialTheme.typography.bodySmall,
                    )

                    Slider(
                        value = currentPosition.toFloat(),
                        onValueChange = {
                            isSeeking = true
                            currentPosition = it.toLong()
                        },
                        onValueChangeFinished = {
                            exoPlayer.seekTo(currentPosition)
                            isSeeking = false
                        },
                        valueRange = 0f..duration.toFloat().coerceAtLeast(1f),
                        modifier = Modifier
                            .weight(1f)
                            .padding(horizontal = 4.dp),
                        colors = SliderDefaults.colors(
                            thumbColor = Color.White,
                            activeTrackColor = Color.White,
                            inactiveTrackColor = Color.White.copy(alpha = 0.3f),
                        ),
                    )

                    Text(
                        text = formatDuration(duration),
                        color = Color.White,
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
            }
        } else {
            // Poster frame with play button
            Box(
                modifier = Modifier
                    .fillMaxSize()
                    .clickable(
                        interactionSource = remember { MutableInteractionSource() },
                        indication = null,
                    ) { isStarted = true },
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

private fun formatDuration(ms: Long): String {
    val totalSeconds = ms / 1000
    val minutes = totalSeconds / 60
    val seconds = totalSeconds % 60
    return "%d:%02d".format(minutes, seconds)
}

@Composable
private fun MediaOverlay(
    personName: String?,
    shotIndex: Int,
    shotCount: Int,
    fileIndex: Int,
    fileCount: Int,
    isOriginal: Boolean,
    timestamp: String?,
    onBack: () -> Unit,
    onDeleteVariant: () -> Unit,
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

                if (!isOriginal) {
                    IconButton(onClick = onDeleteVariant) {
                        Icon(
                            Icons.Default.Delete,
                            contentDescription = "Delete variant",
                            tint = Color.White.copy(alpha = 0.8f),
                        )
                    }
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
