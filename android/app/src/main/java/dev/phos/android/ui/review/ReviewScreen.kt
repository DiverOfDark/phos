package dev.phos.android.ui.review

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.hilt.navigation.compose.hiltViewModel
import coil3.compose.AsyncImage
import androidx.compose.ui.draw.clip
import dev.phos.android.domain.model.Face
import dev.phos.android.domain.model.ShotDetail
import dev.phos.android.ui.common.MonoSmall
import dev.phos.android.ui.common.PhosColors
import dev.phos.android.ui.common.PhosDivider
import dev.phos.android.ui.common.PhosMonoText
import dev.phos.android.ui.common.PhosOutlinedButton
import dev.phos.android.ui.common.PhosPrimaryButton
import dev.phos.android.ui.common.PhosTag
import dev.phos.android.ui.common.PhosTopBar
import dev.phos.android.ui.common.SignalDot
import dev.phos.android.ui.organize.PersonPickerSheet
import dev.phos.android.ui.organize.SplitSheet

/**
 * The review queue.
 *
 * One pending shot at a time with a verdict always one tap away. The layout is
 * deliberately not the web's control panel: on a phone the scarce thing is taps, so
 * the two answers that clear most of a backlog — "yes, that's right" and "no, it's
 * someone else" — are permanent buttons, and everything rarer lives behind the ⋮.
 *
 * Tapping a face goes straight to the face sheet, because a wrong *shot* assignment
 * is usually a wrong *face* assignment underneath, and fixing the face is what stops
 * the same mistake coming back after the next scan.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ReviewScreen(
    onBack: () -> Unit,
    viewModel: ReviewViewModel = hiltViewModel(),
) {
    val uiState by viewModel.uiState.collectAsState()
    val snackbarHostState = remember { SnackbarHostState() }
    var showPersonPicker by remember { mutableStateOf(false) }
    var showSplit by remember { mutableStateOf(false) }
    var showDeleteConfirm by remember { mutableStateOf(false) }
    var showOverflow by remember { mutableStateOf(false) }

    LaunchedEffect(uiState.message) {
        val message = uiState.message ?: return@LaunchedEffect
        snackbarHostState.showSnackbar(message)
        viewModel.consumeMessage()
    }

    val c = PhosColors.current

    Scaffold(
        containerColor = c.base,
        topBar = {
            Box {
                PhosTopBar {
                    Text(
                        text = "←",
                        style = MonoSmall,
                        color = c.textSecondary,
                        modifier = Modifier
                            .clickable(onClick = onBack)
                            .padding(8.dp),
                    )
                    Column(modifier = Modifier.weight(1f)) {
                        Text(
                            text = "Review",
                            style = MaterialTheme.typography.titleMedium,
                            color = c.textPrimary,
                        )
                        if (!uiState.isLoading) {
                            PhosMonoText(
                                text = if (uiState.isEmpty) {
                                    "nothing pending"
                                } else {
                                    "${uiState.remaining} left" +
                                        if (uiState.reviewed > 0) " · ${uiState.reviewed} done" else ""
                                },
                            )
                        }
                    }
                    if (!uiState.isEmpty) {
                        Text(
                            text = "⋮",
                            style = MonoSmall,
                            color = c.textSecondary,
                            modifier = Modifier
                                .clickable(enabled = !uiState.busy) { showOverflow = true }
                                .padding(horizontal = 12.dp, vertical = 8.dp),
                        )
                    }
                }
                DropdownMenu(
                    expanded = showOverflow,
                    onDismissRequest = { showOverflow = false },
                    modifier = Modifier.align(Alignment.TopEnd),
                    containerColor = c.overlay,
                ) {
                    DropdownMenuItem(
                        text = { Text("Previous shot", color = c.textPrimary) },
                        onClick = {
                            showOverflow = false
                            viewModel.previous()
                        },
                    )
                    DropdownMenuItem(
                        text = { Text("Leave unsorted", color = c.textPrimary) },
                        onClick = {
                            showOverflow = false
                            viewModel.markUnsorted()
                        },
                    )
                    DropdownMenuItem(
                        text = { Text("Split shot", color = c.textPrimary) },
                        enabled = (uiState.detail?.files?.size ?: 0) > 1,
                        onClick = {
                            showOverflow = false
                            showSplit = true
                        },
                    )
                    DropdownMenuItem(
                        text = { Text("Delete shot", color = c.error) },
                        onClick = {
                            showOverflow = false
                            showDeleteConfirm = true
                        },
                    )
                }
            }
        },
        // The verdict buttons are the Scaffold's bottom bar, not the last row of
        // the content: a snackbar hosted by the Scaffold is laid out above the
        // bottom bar, and as content they sat under it — every "Moved to Anna"
        // covered the three buttons the user was about to press again.
        bottomBar = {
            if (!uiState.isEmpty && uiState.error == null && !uiState.isLoading) {
                ActionBar(
                    busy = uiState.busy,
                    onSkip = viewModel::skip,
                    onMove = {
                        showPersonPicker = true
                        viewModel.loadPeople()
                    },
                    onConfirm = viewModel::confirm,
                )
            }
        },
        snackbarHost = { SnackbarHost(snackbarHostState) },
    ) { innerPadding ->
        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(innerPadding),
        ) {
            // A hairline that fills while work is in flight — the design's one
            // progress affordance, and it never moves the layout.
            Box(
                modifier = Modifier
                    .fillMaxWidth()
                    .height(1.dp)
                    .background(if (uiState.busy) c.signal else c.line),
            )

            when {
                uiState.isLoading -> Centered {
                    Row(
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        SignalDot(color = c.building, size = 6.dp, pulsing = true)
                        PhosMonoText("loading queue…", color = c.textSecondary)
                    }
                }

                uiState.error != null -> Centered {
                    Column(
                        horizontalAlignment = Alignment.CenterHorizontally,
                        verticalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        SignalDot(color = c.error, size = 10.dp)
                        Text(
                            text = "Could not load the queue",
                            style = MaterialTheme.typography.titleMedium,
                            color = c.textPrimary,
                        )
                        PhosMonoText(uiState.error!!, color = c.error, maxLines = 3)
                        PhosOutlinedButton(onClick = viewModel::load) {
                            Text("Try again", style = MaterialTheme.typography.bodySmall, color = c.textSecondary)
                        }
                    }
                }

                uiState.isEmpty -> Centered {
                    Column(
                        horizontalAlignment = Alignment.CenterHorizontally,
                        verticalArrangement = Arrangement.spacedBy(8.dp),
                        modifier = Modifier.padding(32.dp),
                    ) {
                        SignalDot(color = c.ready, size = 10.dp)
                        Text(
                            text = "Everything is reviewed",
                            style = MaterialTheme.typography.titleMedium,
                            color = c.textPrimary,
                        )
                        Text(
                            text = if (uiState.reviewed > 0) {
                                "${uiState.reviewed} shots cleared this session."
                            } else {
                                "Nothing is waiting on you."
                            },
                            style = MaterialTheme.typography.bodySmall,
                            color = c.textSecondary,
                        )
                        PhosOutlinedButton(onClick = viewModel::load) {
                            Text("Check again", style = MaterialTheme.typography.bodySmall, color = c.textSecondary)
                        }
                    }
                }

                else -> ReviewBody(
                    detail = uiState.detail,
                    guessedName = uiState.current?.personName,
                    isLoadingDetail = uiState.isLoadingDetail,
                    thumbnailUrl = { fileId -> viewModel.buildThumbnailUrl(fileId) },
                    onFaceTap = viewModel::openFace,
                    modifier = Modifier.weight(1f),
                )
            }
        }

        val face = uiState.activeFace
        if (face != null) {
            FaceSheet(
                face = face,
                suggestions = uiState.suggestions,
                suggestionsLoading = uiState.suggestionsLoading,
                people = uiState.people,
                peopleLoading = uiState.peopleLoading,
                thumbnailUrl = { path -> viewModel.absoluteUrl(path) },
                onDismiss = viewModel::closeFace,
                onAssign = viewModel::assignFace,
                onCreate = viewModel::createPersonAndAssignFace,
                onDeleteFace = viewModel::deleteActiveFace,
            )
        }

        if (showPersonPicker) {
            PersonPickerSheet(
                people = uiState.people,
                isLoading = uiState.peopleLoading,
                title = "This shot is",
                onDismiss = { showPersonPicker = false },
                onPick = { personId ->
                    showPersonPicker = false
                    viewModel.moveToPerson(
                        personId,
                        uiState.people.firstOrNull { it.id == personId }?.name,
                    )
                },
                onCreate = { name ->
                    showPersonPicker = false
                    viewModel.createPersonAndMove(name)
                },
            )
        }

        val detail = uiState.detail
        if (showSplit && detail != null) {
            SplitSheet(
                files = detail.files,
                thumbnailUrl = { fileId -> viewModel.buildThumbnailUrl(fileId, 320) },
                isBusy = uiState.busy,
                onDismiss = { showSplit = false },
                onSplit = { fileIds ->
                    showSplit = false
                    viewModel.split(fileIds)
                },
            )
        }

        if (showDeleteConfirm) {
            AlertDialog(
                onDismissRequest = { showDeleteConfirm = false },
                title = { Text("Delete shot?") },
                text = { Text("Every file in it is deleted from the server. Can't be undone.") },
                containerColor = c.overlay,
                confirmButton = {
                    TextButton(onClick = {
                        showDeleteConfirm = false
                        viewModel.deleteShot()
                    }) {
                        Text("Delete", color = c.error)
                    }
                },
                dismissButton = {
                    TextButton(onClick = { showDeleteConfirm = false }) { Text("Cancel") }
                },
            )
        }
    }
}

@Composable
private fun ReviewBody(
    detail: ShotDetail?,
    guessedName: String?,
    isLoadingDetail: Boolean,
    thumbnailUrl: (fileId: String) -> String,
    onFaceTap: (Face) -> Unit,
    modifier: Modifier = Modifier,
) {
    val c = PhosColors.current
    Column(
        modifier = modifier
            .fillMaxWidth()
            .padding(16.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        // The shot's own file, not the queue thumbnail: the face boxes are in this
        // image's coordinates and have to land on the picture the user is judging.
        val file = detail?.files?.firstOrNull { it.isOriginal } ?: detail?.files?.firstOrNull()
        val isVideo = file?.mimeType?.startsWith("video/") == true

        Box(
            modifier = Modifier
                .weight(1f)
                .fillMaxWidth()
                .clip(RoundedCornerShape(4.dp))
                .background(c.surface)
                .border(1.dp, c.line, RoundedCornerShape(4.dp)),
            contentAlignment = Alignment.Center,
        ) {
            when {
                isLoadingDetail && detail == null -> PhosMonoText("loading shot…", color = c.textTertiary)

                file == null -> PhosMonoText("this shot has no files", color = c.textTertiary)

                else -> {
                    val faces = detail!!.faces.filter { it.fileId == file.id }
                    val width = detail.width
                    val height = detail.height
                    // Pinning the frame to the image's aspect ratio is what makes the
                    // overlay simple: the composable's bounds and the picture's bounds
                    // become the same rectangle, so a face box is just a fraction of it.
                    val ratioModifier = if (width != null && height != null && width > 0 && height > 0) {
                        Modifier.aspectRatio(width.toFloat() / height.toFloat())
                    } else {
                        Modifier
                    }

                    BoxWithConstraints(modifier = ratioModifier) {
                        AsyncImage(
                            model = thumbnailUrl(file.id),
                            contentDescription = null,
                            contentScale = ContentScale.Fit,
                            modifier = Modifier.fillMaxSize(),
                        )

                        if (width != null && height != null && width > 0 && height > 0) {
                            val frameWidth = maxWidth
                            val frameHeight = maxHeight
                            for (face in faces) {
                                val left = frameWidth * (face.x1 / width)
                                val top = frameHeight * (face.y1 / height)
                                val boxWidth = frameWidth * ((face.x2 - face.x1) / width)
                                val boxHeight = frameHeight * ((face.y2 - face.y1) / height)
                                FaceBox(
                                    face = face,
                                    onTap = { onFaceTap(face) },
                                    modifier = Modifier
                                        .offset(x = left, y = top)
                                        .size(width = boxWidth, height = boxHeight),
                                )
                            }
                        }
                    }
                }
            }

            if (isVideo) {
                PhosTag(
                    text = "Video",
                    color = c.building,
                    background = c.base,
                    modifier = Modifier
                        .align(Alignment.TopEnd)
                        .padding(8.dp),
                )
            }

            // An attribute of the file, not a status: the label register, no
            // colour — same disclosure the web client gives.
            if (file?.synthetic == true) {
                PhosTag(
                    text = "GENERATED",
                    color = c.textTertiary,
                    background = c.base,
                    modifier = Modifier
                        .align(Alignment.TopStart)
                        .padding(8.dp),
                )
            }
        }

        Column(horizontalAlignment = Alignment.CenterHorizontally) {
            Text(
                text = detail?.primaryPersonName ?: guessedName ?: "Unsorted",
                style = MaterialTheme.typography.titleSmall,
                color = c.textPrimary,
            )
            PhosMonoText(
                text = buildString {
                    append(if (detail?.faces.isNullOrEmpty()) "no faces detected" else "${detail?.faces?.size} face(s)")
                    val extra = detail?.alsoContains.orEmpty()
                    if (extra.isNotEmpty()) append(" · also ${extra.joinToString()}")
                    if ((detail?.files?.size ?: 0) > 1) append(" · ${detail?.files?.size} files")
                },
                modifier = Modifier.padding(top = 2.dp),
            )
            if (detail != null && detail.faces.isNotEmpty()) {
                Text(
                    text = "Tap a face to say who it is.",
                    style = MaterialTheme.typography.bodySmall,
                    color = c.textSecondary,
                    modifier = Modifier.padding(top = 4.dp),
                )
            }
        }
    }
}

@Composable
private fun FaceBox(
    face: Face,
    onTap: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val c = PhosColors.current
    // Assigned faces are outlined quietly; an unassigned one is the thing the
    // reviewer is here to fix, so it gets the signal colour.
    val color = if (face.personId == null) c.signal else Color.White.copy(alpha = 0.7f)
    Box(
        modifier = modifier
            .border(BorderStroke(2.dp, color), RoundedCornerShape(4.dp))
            .clickable(onClick = onTap),
        contentAlignment = Alignment.BottomStart,
    ) {
        Text(
            text = face.personName ?: "?",
            style = MonoSmall,
            color = c.textPrimary,
            modifier = Modifier
                .background(Color.Black.copy(alpha = 0.6f))
                .padding(horizontal = 4.dp, vertical = 1.dp),
        )
    }
}

@Composable
private fun ActionBar(
    busy: Boolean,
    onSkip: () -> Unit,
    onMove: () -> Unit,
    onConfirm: () -> Unit,
) {
    val c = PhosColors.current
    Column {
        PhosDivider()
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .background(c.base)
                // As a bottom bar it is placed against the screen edge, so the gesture
                // bar inset is its own to add — the content padding no longer covers it.
                .navigationBarsPadding()
                .padding(16.dp),
            horizontalArrangement = Arrangement.spacedBy(8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            PhosOutlinedButton(onClick = onSkip, enabled = !busy) {
                Text("Skip", style = MaterialTheme.typography.bodySmall, color = c.textSecondary)
            }
            PhosOutlinedButton(onClick = onMove, enabled = !busy, modifier = Modifier.weight(1f)) {
                Text(
                    text = "Someone else",
                    style = MaterialTheme.typography.bodySmall,
                    color = c.textSecondary,
                )
            }
            PhosPrimaryButton(onClick = onConfirm, enabled = !busy, modifier = Modifier.weight(1f)) {
                Text("Correct", style = MaterialTheme.typography.labelLarge, color = c.signalFg)
            }
        }
    }
}

@Composable
private fun Centered(content: @Composable () -> Unit) {
    Box(
        modifier = Modifier.fillMaxSize(),
        contentAlignment = Alignment.Center,
    ) { content() }
}
