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
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.filled.Pause
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
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
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.ui.draw.clip
import dev.phos.android.ui.common.MonoBody
import dev.phos.android.ui.common.MonoSmall
import dev.phos.android.ui.common.PhosColors
import dev.phos.android.ui.common.SignalDot
import dev.phos.android.domain.model.MediaFile
import dev.phos.android.ui.common.FullScreenLoading
import dev.phos.android.ui.organize.FacesSheet
import dev.phos.android.ui.organize.MergeSheet
import dev.phos.android.ui.organize.PersonPickerSheet
import dev.phos.android.ui.organize.ShotActionsSheet
import dev.phos.android.ui.organize.SplitSheet
import dev.phos.android.ui.review.FaceSheet
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.distinctUntilChanged
import me.saket.telephoto.zoomable.coil3.ZoomableAsyncImage
import me.saket.telephoto.zoomable.rememberZoomableImageState
import me.saket.telephoto.zoomable.ZoomSpec
import me.saket.telephoto.zoomable.rememberZoomableState
import me.saket.telephoto.zoomable.zoomable

/** Which organizing sheet the browser currently has open. */
private enum class OrganizeSheet { None, Actions, Person, Split, Merge, Faces, Face }

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun BrowserScreen(
    onBack: () -> Unit,
    viewModel: BrowserViewModel = hiltViewModel(),
) {
    val uiState by viewModel.uiState.collectAsState()

    if (uiState.isLoading) {
        FullScreenLoading("loading shots…")
        return
    }

    if (uiState.shots.isEmpty()) {
        Box(
            modifier = Modifier.fillMaxSize(),
            contentAlignment = Alignment.Center,
        ) {
            Column(
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                SignalDot(
                    color = if (uiState.error != null) PhosColors.current.error else PhosColors.current.stopped,
                    size = 10.dp,
                )
                Text(
                    text = "No shots here",
                    style = MaterialTheme.typography.titleMedium,
                    color = PhosColors.current.textPrimary,
                )
                Text(
                    text = uiState.error ?: "This person has no photos or videos.",
                    style = MaterialTheme.typography.bodySmall,
                    color = PhosColors.current.textSecondary,
                )
            }
        }
        return
    }

    var showOverlay by remember { mutableStateOf(true) }
    var currentFileIndex by remember { mutableStateOf(uiState.initialFileIndex) }
    var showDeleteConfirm by remember { mutableStateOf(false) }
    var showDeleteShotConfirm by remember { mutableStateOf(false) }
    // Which organizing sheet is open, if any. UI state, so it lives here rather
    // than in the ViewModel — a rotation losing an open sheet is fine, a rotation
    // losing an in-flight delete is not, and that one is in the ViewModel.
    var openSheet by remember { mutableStateOf(OrganizeSheet.None) }
    val snackbarHostState = remember { SnackbarHostState() }

    // One place where every failure and confirmation surfaces.
    LaunchedEffect(uiState.message) {
        val message = uiState.message ?: return@LaunchedEffect
        snackbarHostState.showSnackbar(message)
        viewModel.consumeMessage()
    }

    val verticalPagerState = rememberPagerState(
        initialPage = uiState.initialShotIndex,
        pageCount = { uiState.shots.size },
    )

    // getOrNull, not indexing: a delete or a merge shortens the list under the
    // pager, and for one frame `currentPage` can still point past the end.
    val currentShot = uiState.shots.getOrNull(verticalPagerState.currentPage)

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
            .background(PhosColors.current.base),
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
                    Text("no files", style = MonoBody, color = PhosColors.current.textTertiary)
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
                                                PhosColors.current.signal
                                            else Color.White.copy(alpha = 0.4f),
                                            shape = RoundedCornerShape(2.dp),
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
                fileCount = currentShot?.files?.size ?: 0,
                isOriginal = currentShot?.files?.getOrNull(currentFileIndex)?.isOriginal ?: true,
                isSynthetic = currentShot?.files?.getOrNull(currentFileIndex)?.synthetic ?: false,
                timestamp = currentShot?.shot?.timestamp,
                onBack = onBack,
                onDeleteVariant = { showDeleteConfirm = true },
                onActions = { openSheet = OrganizeSheet.Actions },
            )
        }

        if (showDeleteConfirm) {
            AlertDialog(
                onDismissRequest = { showDeleteConfirm = false },
                containerColor = PhosColors.current.overlay,
                title = { Text("Delete variant?") },
                text = { Text("This will permanently delete this file variant from the server.") },
                confirmButton = {
                    TextButton(onClick = {
                        showDeleteConfirm = false
                        viewModel.deleteFile(verticalPagerState.currentPage, currentFileIndex)
                    }) {
                        Text("Delete", color = PhosColors.current.error)
                    }
                },
                dismissButton = {
                    TextButton(onClick = { showDeleteConfirm = false }) {
                        Text("Cancel")
                    }
                },
            )
        }

        if (showDeleteShotConfirm && currentShot != null) {
            val fileCount = currentShot.files.size
            AlertDialog(
                onDismissRequest = { showDeleteShotConfirm = false },
                containerColor = PhosColors.current.overlay,
                title = { Text("Delete shot?") },
                text = {
                    Text(
                        if (fileCount > 1) {
                            "This deletes all $fileCount files in this shot from the " +
                                "server. It can't be undone."
                        } else {
                            "This deletes the file from the server. It can't be undone."
                        }
                    )
                },
                confirmButton = {
                    TextButton(onClick = {
                        showDeleteShotConfirm = false
                        viewModel.deleteShot(currentShot.shot.id)
                    }) {
                        Text("Delete", color = PhosColors.current.error)
                    }
                },
                dismissButton = {
                    TextButton(onClick = { showDeleteShotConfirm = false }) { Text("Cancel") }
                },
            )
        }

        // ---- organizing sheets ------------------------------------------
        // All of them close before the call starts: the result arrives as a
        // snackbar, and a sheet sitting over a list that is about to change is
        // just something else for the user to dismiss.
        if (currentShot != null) {
            when (openSheet) {
                OrganizeSheet.None -> Unit

                OrganizeSheet.Actions -> ShotActionsSheet(
                    fileCount = currentShot.files.size,
                    currentFileIsOriginal =
                        currentShot.files.getOrNull(currentFileIndex)?.isOriginal ?: true,
                    onDismiss = { openSheet = OrganizeSheet.None },
                    onMoveToPerson = {
                        openSheet = OrganizeSheet.Person
                        viewModel.loadPeople()
                    },
                    onFaces = {
                        openSheet = OrganizeSheet.Faces
                        viewModel.loadFaces(currentShot.shot.id)
                    },
                    onSplit = { openSheet = OrganizeSheet.Split },
                    onMerge = {
                        openSheet = OrganizeSheet.Merge
                        viewModel.loadSimilar(currentShot.shot.id)
                    },
                    onDeleteVariant = {
                        openSheet = OrganizeSheet.None
                        showDeleteConfirm = true
                    },
                    onDeleteShot = {
                        openSheet = OrganizeSheet.None
                        showDeleteShotConfirm = true
                    },
                )

                OrganizeSheet.Person -> PersonPickerSheet(
                    people = uiState.people,
                    isLoading = uiState.peopleLoading,
                    title = "Move this shot to",
                    onDismiss = { openSheet = OrganizeSheet.None },
                    onPick = { personId ->
                        openSheet = OrganizeSheet.None
                        viewModel.moveToPerson(
                            shotId = currentShot.shot.id,
                            personId = personId,
                            personName = uiState.people.firstOrNull { it.id == personId }?.name,
                        )
                    },
                    onCreate = { name ->
                        openSheet = OrganizeSheet.None
                        viewModel.createPersonAndMove(currentShot.shot.id, name)
                    },
                )

                OrganizeSheet.Split -> SplitSheet(
                    files = currentShot.files,
                    thumbnailUrl = { fileId -> viewModel.buildThumbnailUrl(fileId, 320) },
                    isBusy = uiState.busy,
                    onDismiss = { openSheet = OrganizeSheet.None },
                    onSplit = { fileIds ->
                        openSheet = OrganizeSheet.None
                        viewModel.splitShot(currentShot.shot.id, fileIds)
                    },
                )

                OrganizeSheet.Faces -> FacesSheet(
                    faces = uiState.faces,
                    isLoading = uiState.facesLoading,
                    faceThumbnailUrl = { faceId -> viewModel.faceThumbnailUrl(faceId) },
                    onDismiss = { openSheet = OrganizeSheet.None },
                    onPick = { face ->
                        // One sheet at a time: the faces list is swapped for the
                        // "who is this?" sheet, and dismissing that returns here.
                        openSheet = OrganizeSheet.Face
                        viewModel.openFace(face)
                    },
                )

                OrganizeSheet.Face -> {
                    val face = uiState.activeFace
                    if (face == null) {
                        openSheet = OrganizeSheet.Faces
                    } else {
                        FaceSheet(
                            face = face,
                            suggestions = uiState.suggestions,
                            suggestionsLoading = uiState.suggestionsLoading,
                            people = uiState.people,
                            peopleLoading = uiState.peopleLoading,
                            thumbnailUrl = { path -> viewModel.absoluteUrl(path) },
                            onDismiss = {
                                viewModel.closeFace()
                                openSheet = OrganizeSheet.Faces
                            },
                            onAssign = { personId, personName ->
                                openSheet = OrganizeSheet.None
                                viewModel.assignFace(personId, personName)
                            },
                            onCreate = { name ->
                                openSheet = OrganizeSheet.None
                                viewModel.createPersonAndAssignFace(name)
                            },
                            onDeleteFace = {
                                openSheet = OrganizeSheet.None
                                viewModel.deleteActiveFace()
                            },
                        )
                    }
                }

                OrganizeSheet.Merge -> MergeSheet(
                    candidates = uiState.similar,
                    isLoading = uiState.similarLoading,
                    isBusy = uiState.busy,
                    thumbnailUrl = { path -> viewModel.absoluteUrl(path) },
                    onDismiss = { openSheet = OrganizeSheet.None },
                    onMerge = { sourceId ->
                        openSheet = OrganizeSheet.None
                        viewModel.mergeInto(
                            targetShotId = currentShot.shot.id,
                            sourceShotId = sourceId,
                        )
                    },
                )
            }
        }

        SnackbarHost(
            hostState = snackbarHostState,
            modifier = Modifier
                .align(Alignment.BottomCenter)
                .navigationBarsPadding(),
        )
    }
}

@Composable
private fun MediaPage(
    file: MediaFile,
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
                        .background(Color.Black.copy(alpha = 0.65f))
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
                        style = MonoSmall,
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
                            thumbColor = PhosColors.current.signal,
                            activeTrackColor = PhosColors.current.signal,
                            inactiveTrackColor = Color.White.copy(alpha = 0.3f),
                        ),
                    )

                    Text(
                        text = formatDuration(duration),
                        color = Color.White,
                        style = MonoSmall,
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
                        .clip(RoundedCornerShape(4.dp))
                        .background(Color.Black.copy(alpha = 0.5f))
                        .padding(horizontal = 20.dp, vertical = 10.dp),
                    contentAlignment = Alignment.Center,
                ) {
                    Text(text = "▶ play", style = MonoBody, color = Color.White)
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
    isSynthetic: Boolean,
    timestamp: String?,
    onBack: () -> Unit,
    onDeleteVariant: () -> Unit,
    onActions: () -> Unit,
) {
    val c = PhosColors.current
    Box(modifier = Modifier.fillMaxSize()) {
        // No protection gradient: the design system does not have one. Each control
        // carries its own small solid ground instead, so the photo stays uncovered.
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .statusBarsPadding()
                .padding(horizontal = 16.dp, vertical = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            OverlayChip(text = "←", onClick = onBack)

            Column(modifier = Modifier.weight(1f)) {
                Text(
                    text = personName ?: "Unsorted",
                    style = MaterialTheme.typography.titleSmall,
                    color = Color.White,
                )
                Text(
                    text = buildString {
                        append("shot ")
                        append(shotIndex + 1)
                        append(" / ")
                        append(shotCount)
                        if (fileCount > 1) {
                            append(" · variant ")
                            append(fileIndex + 1)
                            append(" / ")
                            append(fileCount)
                        }
                        // An attribute of the file, not a status — same
                        // register as the rest of this line, no colour.
                        if (isSynthetic) {
                            append(" · GENERATED")
                        }
                    },
                    style = MonoSmall,
                    color = Color.White.copy(alpha = 0.8f),
                )
            }

            if (!isOriginal) {
                OverlayChip(text = "del", onClick = onDeleteVariant, color = c.error)
            }
            OverlayChip(text = "⋮", onClick = onActions)
        }

        if (timestamp != null) {
            Box(
                modifier = Modifier
                    .align(Alignment.BottomStart)
                    .navigationBarsPadding()
                    .padding(16.dp)
                    .background(Color.Black.copy(alpha = 0.4f), RoundedCornerShape(4.dp))
                    .padding(horizontal = 8.dp, vertical = 4.dp),
            ) {
                Text(text = timestamp, style = MonoSmall, color = Color.White.copy(alpha = 0.8f))
            }
        }
    }
}

/**
 * A control on top of a photo: a small solid ground rather than a gradient over
 * the whole edge, so the picture keeps its corners.
 */
@Composable
private fun OverlayChip(
    text: String,
    onClick: () -> Unit,
    color: Color = Color.White,
) {
    Box(
        modifier = Modifier
            .clip(RoundedCornerShape(4.dp))
            .background(Color.Black.copy(alpha = 0.4f))
            .clickable(onClick = onClick)
            .padding(horizontal = 12.dp, vertical = 8.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(text = text, style = MonoSmall, color = color)
    }
}
