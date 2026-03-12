package dev.phos.android.ui.browser

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
import androidx.compose.material.icons.filled.Videocam
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
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
import androidx.compose.ui.unit.dp
import androidx.hilt.navigation.compose.hiltViewModel
import coil3.compose.AsyncImage
import dev.phos.android.data.local.entity.FileEntity
import dev.phos.android.ui.common.FullScreenLoading
import kotlinx.coroutines.flow.distinctUntilChanged

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

    val verticalPagerState = rememberPagerState(
        initialPage = uiState.initialShotIndex,
        pageCount = { uiState.shots.size },
    )

    // Track shot changes for position persistence and prefetch
    LaunchedEffect(verticalPagerState) {
        snapshotFlow { verticalPagerState.currentPage }
            .distinctUntilChanged()
            .collect { shotIndex ->
                viewModel.onShotChanged(shotIndex, 0)
            }
    }

    Box(
        modifier = Modifier
            .fillMaxSize()
            .background(Color.Black)
            .clickable(
                interactionSource = remember { MutableInteractionSource() },
                indication = null,
            ) { showOverlay = !showOverlay },
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
                    MediaPage(
                        file = file,
                        thumbnailUrl = viewModel.buildThumbnailUrl(file.id, 1080),
                        isVideo = viewModel.isVideo(file),
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
                fileIndex = if (uiState.shots.isNotEmpty()) {
                    val currentShot = uiState.shots[verticalPagerState.currentPage]
                    0 // Will be updated by horizontal pager
                } else 0,
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
    isVideo: Boolean,
) {
    Box(
        modifier = Modifier.fillMaxSize(),
        contentAlignment = Alignment.Center,
    ) {
        // Image (thumbnail -> preview quality)
        AsyncImage(
            model = thumbnailUrl,
            contentDescription = null,
            contentScale = ContentScale.Fit,
            modifier = Modifier.fillMaxSize(),
        )

        // Video indicator
        if (isVideo) {
            Box(
                modifier = Modifier
                    .align(Alignment.Center)
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
