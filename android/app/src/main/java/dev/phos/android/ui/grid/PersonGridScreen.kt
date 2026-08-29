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
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.itemsIndexed
import androidx.compose.foundation.lazy.grid.rememberLazyGridState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.material3.AlertDialog
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
import androidx.compose.ui.draw.clip
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
import dev.phos.android.ui.common.MonoSmall
import dev.phos.android.ui.common.PhosColors
import dev.phos.android.ui.common.PhosDivider
import dev.phos.android.ui.common.PhosMonoText
import dev.phos.android.ui.common.PhosOutlinedButton
import dev.phos.android.ui.common.PhosTopBar
import dev.phos.android.ui.common.ShimmerBox
import dev.phos.android.ui.common.SignalDot
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

    val c = PhosColors.current

    Scaffold(
        containerColor = c.base,
        topBar = {
            if (uiState.selectionMode) {
                // A contextual bar, not a menu: while a selection exists the whole
                // screen is about that selection, and the way out is the same ✕ that
                // clears it. Actions are mono words — a row of icons at this size is
                // a guessing game.
                Column(
                    modifier = Modifier
                        .fillMaxWidth()
                        .background(c.surface)
                        .statusBarsPadding(),
                ) {
                    Row(
                        modifier = Modifier
                            .fillMaxWidth()
                            .padding(horizontal = 8.dp)
                            .height(56.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        BarAction("✕", c.textSecondary, enabled = true, onClick = viewModel::clearSelection)
                        Text(
                            text = "${uiState.selected.size} selected",
                            style = MaterialTheme.typography.titleSmall,
                            color = c.textPrimary,
                            modifier = Modifier.weight(1f).padding(start = 8.dp),
                        )
                        BarAction("all", c.textSecondary, !uiState.busy, viewModel::selectAll)
                        BarAction("✓", c.ready, !uiState.busy, viewModel::confirmSelected)
                        BarAction("move", c.textSecondary, !uiState.busy) {
                            showPersonPicker = true
                            viewModel.loadPeople()
                        }
                        BarAction("del", c.error, !uiState.busy) { showDeleteConfirm = true }
                    }
                    Box(
                        modifier = Modifier
                            .fillMaxWidth()
                            .height(1.dp)
                            .background(c.signalMuted),
                    )
                }
            } else {
                PhosTopBar {
                    Text(
                        text = "←",
                        style = MonoSmall,
                        color = c.textSecondary,
                        modifier = Modifier
                            .clickable(onClick = onBack)
                            .padding(8.dp),
                    )
                    Text(
                        text = uiState.personName ?: "Unsorted",
                        style = MaterialTheme.typography.titleMedium,
                        color = c.textPrimary,
                        modifier = Modifier.weight(1f),
                    )
                    PhosMonoText("${uiState.tiles.size} shots")
                    PhosOutlinedButton(onClick = viewModel::enterSelectionMode) {
                        PhosMonoText("select", color = c.textSecondary)
                    }
                }
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
                uiState.isLoading -> FullScreenLoading("loading shots…")
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
                            fileCount = tile.fileCount,
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
    fileCount: Int,
    isSelected: Boolean,
    onClick: () -> Unit,
    onLongClick: () -> Unit,
) {
    val c = PhosColors.current
    Box(
        modifier = Modifier
            .aspectRatio(1f)
            .clip(RoundedCornerShape(2.dp))
            .background(c.raised)
            .border(1.dp, if (isSelected) c.signal else c.line, RoundedCornerShape(2.dp))
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

        // A shot with several files is one tile; saying so stops the grid from
        // reading as if the other copies had gone missing.
        if (fileCount > 1) {
            Box(
                modifier = Modifier
                    .align(Alignment.BottomEnd)
                    .padding(4.dp)
                    .background(c.base, RoundedCornerShape(2.dp))
                    .border(1.dp, c.line, RoundedCornerShape(2.dp))
                    .padding(horizontal = 3.dp),
            ) {
                Text(text = "×$fileCount", style = MonoSmall, color = c.textSecondary)
            }
        }

        if (isSelected) {
            // Scrim plus a mark: on a wall of thumbnails a badge alone is easy to
            // miss, and the dimming is what makes the selection readable at a glance.
            Box(
                modifier = Modifier
                    .fillMaxSize()
                    .background(c.base.copy(alpha = 0.55f)),
            )
            Text(
                text = "✓",
                style = MonoSmall,
                color = c.signal,
                modifier = Modifier
                    .align(Alignment.TopEnd)
                    .padding(4.dp),
            )
        }
    }
}

/** A mono word in the contextual bar. Words, not icons — this bar is rarely open. */
@Composable
private fun BarAction(
    label: String,
    color: androidx.compose.ui.graphics.Color,
    enabled: Boolean,
    onClick: () -> Unit,
) {
    Text(
        text = label,
        style = MonoSmall,
        color = if (enabled) color else color.copy(alpha = 0.4f),
        modifier = Modifier
            .clickable(enabled = enabled, onClick = onClick)
            .padding(horizontal = 8.dp, vertical = 12.dp),
    )
}

@Composable
private fun EmptyState() {
    val c = PhosColors.current
    Box(
        modifier = Modifier.fillMaxSize(),
        contentAlignment = Alignment.Center,
    ) {
        Column(
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            SignalDot(color = c.stopped, size = 10.dp)
            Text(
                text = "Nothing filed here",
                style = MaterialTheme.typography.titleMedium,
                color = c.textPrimary,
            )
            Text(
                text = "Shots land here once they are routed to this person.",
                textAlign = TextAlign.Center,
                style = MaterialTheme.typography.bodySmall,
                color = c.textSecondary,
            )
        }
    }
}
