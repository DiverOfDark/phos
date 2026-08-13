package dev.phos.android.ui.grid

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.itemsIndexed
import androidx.compose.foundation.lazy.grid.rememberLazyGridState
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Check
import androidx.compose.material.icons.filled.CheckCircle
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material.icons.filled.Person
import androidx.compose.material.icons.filled.SelectAll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
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
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import coil3.compose.AsyncImage
import dev.phos.android.ui.common.ErrorBanner
import dev.phos.android.ui.common.FullScreenLoading
import dev.phos.android.ui.common.ShimmerBox
import dev.phos.android.ui.organize.PersonPickerSheet

@OptIn(ExperimentalMaterial3Api::class, ExperimentalFoundationApi::class)
@Composable
fun PersonGridScreen(
    onBack: () -> Unit,
    onTileClick: (shotIndex: Int) -> Unit,
    viewModel: PersonGridViewModel = hiltViewModel(),
) {
    val uiState by viewModel.uiState.collectAsState()

    // Re-read last-viewed position when returning from the fullscreen browser.
    val lifecycleOwner = LocalLifecycleOwner.current
    LaunchedEffect(lifecycleOwner) {
        val observer = LifecycleEventObserver { _, event ->
            if (event == Lifecycle.Event.ON_RESUME) {
                viewModel.refreshLastViewedPosition()
            }
        }
        lifecycleOwner.lifecycle.addObserver(observer)
    }

    val gridState = rememberLazyGridState()
    val snackbarHostState = remember { SnackbarHostState() }
    var showPersonPicker by remember { mutableStateOf(false) }
    var showDeleteConfirm by remember { mutableStateOf(false) }

    LaunchedEffect(uiState.message) {
        val message = uiState.message ?: return@LaunchedEffect
        snackbarHostState.showSnackbar(message)
        viewModel.consumeMessage()
    }

    // System back clears the selection before it leaves the screen — the same thing
    // the X in the contextual bar does, and what every gallery on the platform does.
    BackHandler(enabled = uiState.selectionMode) { viewModel.clearSelection() }

    // Scroll to last-viewed tile whenever it changes.
    LaunchedEffect(uiState.lastViewedShotIndex, uiState.tiles.size) {
        if (uiState.tiles.isNotEmpty()) {
            gridState.scrollToItem(uiState.lastViewedShotIndex)
        }
    }

    Scaffold(
        topBar = {
            if (uiState.selectionMode) {
                // A contextual bar, not a menu: while a selection exists the whole
                // screen is about that selection, and the way out is the same X that
                // clears it.
                TopAppBar(
                    title = { Text("${uiState.selected.size} selected") },
                    navigationIcon = {
                        IconButton(onClick = viewModel::clearSelection) {
                            Icon(Icons.Default.Close, contentDescription = "Clear selection")
                        }
                    },
                    actions = {
                        IconButton(onClick = viewModel::selectAll, enabled = !uiState.busy) {
                            Icon(Icons.Default.SelectAll, contentDescription = "Select all")
                        }
                        IconButton(onClick = viewModel::confirmSelected, enabled = !uiState.busy) {
                            Icon(Icons.Default.Check, contentDescription = "Mark reviewed")
                        }
                        IconButton(
                            onClick = {
                                showPersonPicker = true
                                viewModel.loadPeople()
                            },
                            enabled = !uiState.busy,
                        ) {
                            Icon(Icons.Default.Person, contentDescription = "Move to person")
                        }
                        IconButton(
                            onClick = { showDeleteConfirm = true },
                            enabled = !uiState.busy,
                        ) {
                            Icon(
                                Icons.Default.Delete,
                                contentDescription = "Delete selected",
                                tint = MaterialTheme.colorScheme.error,
                            )
                        }
                    },
                    colors = TopAppBarDefaults.topAppBarColors(
                        containerColor = MaterialTheme.colorScheme.secondaryContainer,
                    ),
                )
            } else {
                TopAppBar(
                    title = { Text(uiState.personName ?: "Photos") },
                    navigationIcon = {
                        IconButton(onClick = onBack) {
                            Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
                        }
                    },
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
            uiState.error?.let { ErrorBanner(message = it) }

            when {
                uiState.isLoading -> FullScreenLoading("Loading photos...")
                uiState.tiles.isEmpty() -> EmptyState()
                else -> LazyVerticalGrid(
                    state = gridState,
                    columns = GridCells.Fixed(3),
                    modifier = Modifier.fillMaxSize(),
                    contentPadding = PaddingValues(2.dp),
                    horizontalArrangement = Arrangement.spacedBy(2.dp),
                    verticalArrangement = Arrangement.spacedBy(2.dp),
                ) {
                    itemsIndexed(uiState.tiles, key = { _, tile -> tile.shot.id }) { index, tile ->
                        GridTileView(
                            thumbnailUrl = tile.cover?.let { viewModel.buildThumbnailUrl(it.id) },
                            isSelected = tile.shot.id in uiState.selected,
                            // In selection mode a tap toggles instead of opening —
                            // otherwise picking the second shot would launch the
                            // viewer over the selection the user is building.
                            onClick = {
                                if (uiState.selectionMode) {
                                    viewModel.toggleSelection(tile.shot.id)
                                } else {
                                    onTileClick(index)
                                }
                            },
                            onLongClick = { viewModel.toggleSelection(tile.shot.id) },
                        )
                    }
                }
            }
        }

        if (showPersonPicker) {
            PersonPickerSheet(
                people = uiState.people,
                isLoading = uiState.peopleLoading,
                title = "Move ${uiState.selected.size} shot(s) to",
                onDismiss = { showPersonPicker = false },
                onPick = { personId ->
                    showPersonPicker = false
                    viewModel.moveSelectedTo(
                        personId = personId,
                        personName = uiState.people.firstOrNull { it.id == personId }?.name,
                    )
                },
                onCreate = { name ->
                    showPersonPicker = false
                    viewModel.moveSelectedToNewPerson(name)
                },
            )
        }

        if (showDeleteConfirm) {
            val count = uiState.selected.size
            AlertDialog(
                onDismissRequest = { showDeleteConfirm = false },
                title = { Text("Delete $count shot(s)?") },
                text = {
                    Text(
                        "Every file in them is deleted from the server. This can't be " +
                            "undone."
                    )
                },
                confirmButton = {
                    TextButton(onClick = {
                        showDeleteConfirm = false
                        viewModel.deleteSelected()
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

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun GridTileView(
    thumbnailUrl: String?,
    isSelected: Boolean,
    onClick: () -> Unit,
    onLongClick: () -> Unit,
) {
    Box(
        modifier = Modifier
            .aspectRatio(1f)
            .combinedClickable(onClick = onClick, onLongClick = onLongClick),
    ) {
        if (thumbnailUrl != null) {
            AsyncImage(
                model = thumbnailUrl,
                contentDescription = null,
                contentScale = ContentScale.Crop,
                modifier = Modifier.fillMaxSize(),
            )
        } else {
            ShimmerBox(modifier = Modifier.fillMaxSize())
        }

        if (isSelected) {
            // Scrim plus a badge: on a wall of thumbnails a badge alone is easy to
            // miss, and the dimming is what makes the selection readable at a glance.
            Box(
                modifier = Modifier
                    .fillMaxSize()
                    .background(Color.Black.copy(alpha = 0.45f)),
            )
            Icon(
                Icons.Default.CheckCircle,
                contentDescription = "Selected",
                tint = Color.White,
                modifier = Modifier
                    .align(Alignment.TopEnd)
                    .padding(4.dp),
            )
        }
    }
}

@Composable
private fun EmptyState() {
    Box(
        modifier = Modifier.fillMaxSize(),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            text = "No photos for this person.",
            textAlign = TextAlign.Center,
            style = MaterialTheme.typography.bodyLarge,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}
