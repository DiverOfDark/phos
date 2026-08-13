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
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Check
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.filled.SkipNext
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
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
import dev.phos.android.domain.model.Face
import dev.phos.android.domain.model.ShotDetail
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

    Scaffold(
        topBar = {
            TopAppBar(
                title = {
                    Column {
                        Text("Review")
                        if (!uiState.isLoading) {
                            Text(
                                text = if (uiState.isEmpty) {
                                    "Nothing pending"
                                } else {
                                    "${uiState.remaining} left" +
                                        if (uiState.reviewed > 0) " · ${uiState.reviewed} done" else ""
                                },
                                style = MaterialTheme.typography.bodySmall,
                            )
                        }
                    }
                },
                navigationIcon = {
                    IconButton(onClick = onBack) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
                    }
                },
                actions = {
                    if (!uiState.isEmpty) {
                        IconButton(onClick = { showOverflow = true }, enabled = !uiState.busy) {
                            Icon(Icons.Default.MoreVert, contentDescription = "More actions")
                        }
                        DropdownMenu(
                            expanded = showOverflow,
                            onDismissRequest = { showOverflow = false },
                        ) {
                            DropdownMenuItem(
                                text = { Text("Previous shot") },
                                onClick = {
                                    showOverflow = false
                                    viewModel.previous()
                                },
                            )
                            DropdownMenuItem(
                                text = { Text("Leave unsorted") },
                                onClick = {
                                    showOverflow = false
                                    viewModel.markUnsorted()
                                },
                            )
                            DropdownMenuItem(
                                text = { Text("Split shot") },
                                enabled = (uiState.detail?.files?.size ?: 0) > 1,
                                onClick = {
                                    showOverflow = false
                                    showSplit = true
                                },
                            )
                            DropdownMenuItem(
                                text = {
                                    Text("Delete shot", color = MaterialTheme.colorScheme.error)
                                },
                                onClick = {
                                    showOverflow = false
                                    showDeleteConfirm = true
                                },
                            )
                        }
                    }
                },
            )
        },
        snackbarHost = { SnackbarHost(snackbarHostState) },
    ) { innerPadding ->
        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(innerPadding),
        ) {
            if (uiState.busy) {
                LinearProgressIndicator(modifier = Modifier.fillMaxWidth())
            }

            when {
                uiState.isLoading -> Centered { CircularProgressIndicator() }

                uiState.error != null -> Centered {
                    Column(horizontalAlignment = Alignment.CenterHorizontally) {
                        Text(uiState.error!!, style = MaterialTheme.typography.bodyMedium)
                        Spacer(Modifier.height(12.dp))
                        OutlinedButton(onClick = viewModel::load) { Text("Try again") }
                    }
                }

                uiState.isEmpty -> Centered {
                    Column(horizontalAlignment = Alignment.CenterHorizontally) {
                        Text("Everything is reviewed.", style = MaterialTheme.typography.titleMedium)
                        Spacer(Modifier.height(4.dp))
                        Text(
                            if (uiState.reviewed > 0) {
                                "You cleared ${uiState.reviewed} shot(s) this session."
                            } else {
                                "Nothing is waiting on you."
                            },
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                        Spacer(Modifier.height(12.dp))
                        OutlinedButton(onClick = viewModel::load) { Text("Check again") }
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
                confirmButton = {
                    TextButton(onClick = {
                        showDeleteConfirm = false
                        viewModel.deleteShot()
                    }) {
                        Text("Delete", color = MaterialTheme.colorScheme.error)
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
    Column(
        modifier = modifier
            .fillMaxWidth()
            .padding(horizontal = 16.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Spacer(Modifier.height(8.dp))

        // The shot's own file, not the queue thumbnail: the face boxes are in this
        // image's coordinates and have to land on the picture the user is judging.
        val file = detail?.files?.firstOrNull { it.isOriginal } ?: detail?.files?.firstOrNull()

        Box(modifier = Modifier.weight(1f), contentAlignment = Alignment.Center) {
            when {
                isLoadingDetail && detail == null -> CircularProgressIndicator()

                file == null -> Text(
                    "This shot has no files.",
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )

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
        }

        Spacer(Modifier.height(8.dp))

        Text(
            text = detail?.primaryPersonName ?: guessedName ?: "Unassigned",
            style = MaterialTheme.typography.titleMedium,
            fontWeight = FontWeight.Medium,
        )
        Text(
            text = buildString {
                append(if (detail?.faces.isNullOrEmpty()) "No faces detected" else "${detail?.faces?.size} face(s)")
                val extra = detail?.alsoContains.orEmpty()
                if (extra.isNotEmpty()) append(" · also ${extra.joinToString()}")
                if ((detail?.files?.size ?: 0) > 1) append(" · ${detail?.files?.size} files")
            },
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        if (detail != null && detail.faces.isNotEmpty()) {
            Text(
                "Tap a face to say who it is.",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        Spacer(Modifier.height(8.dp))
    }
}

@Composable
private fun FaceBox(
    face: Face,
    onTap: () -> Unit,
    modifier: Modifier = Modifier,
) {
    // Assigned faces are outlined quietly; an unassigned one is the thing the
    // reviewer is here to fix, so it gets the accent colour.
    val color = if (face.personId == null) {
        MaterialTheme.colorScheme.tertiary
    } else {
        Color.White
    }
    Box(
        modifier = modifier
            .border(BorderStroke(2.dp, color), RoundedCornerShape(4.dp))
            .clickable(onClick = onTap),
        contentAlignment = Alignment.BottomStart,
    ) {
        val label = face.personName ?: "?"
        Text(
            text = label,
            style = MaterialTheme.typography.labelSmall,
            color = Color.White,
            modifier = Modifier
                .background(Color.Black.copy(alpha = 0.55f))
                .padding(horizontal = 4.dp),
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
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(16.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        OutlinedButton(onClick = onSkip, enabled = !busy) {
            Icon(Icons.Default.SkipNext, contentDescription = null, modifier = Modifier.size(18.dp))
            Spacer(Modifier.width(4.dp))
            Text("Skip")
        }
        OutlinedButton(onClick = onMove, enabled = !busy, modifier = Modifier.weight(1f)) {
            Text("Someone else")
        }
        Button(onClick = onConfirm, enabled = !busy, modifier = Modifier.weight(1f)) {
            Icon(Icons.Default.Check, contentDescription = null, modifier = Modifier.size(18.dp))
            Spacer(Modifier.width(4.dp))
            Text("Correct")
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
