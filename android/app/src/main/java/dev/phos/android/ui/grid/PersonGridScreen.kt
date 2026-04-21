package dev.phos.android.ui.grid

import androidx.compose.foundation.clickable
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
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
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

@OptIn(ExperimentalMaterial3Api::class)
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

    // Scroll to last-viewed tile whenever it changes.
    LaunchedEffect(uiState.lastViewedShotIndex, uiState.tiles.size) {
        if (uiState.tiles.isNotEmpty()) {
            gridState.scrollToItem(uiState.lastViewedShotIndex)
        }
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(uiState.personName ?: "Photos") },
                navigationIcon = {
                    IconButton(onClick = onBack) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
                    }
                },
            )
        },
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
                            onClick = { onTileClick(index) },
                        )
                    }
                }
            }
        }
    }
}

@Composable
private fun GridTileView(
    thumbnailUrl: String?,
    onClick: () -> Unit,
) {
    Box(
        modifier = Modifier
            .aspectRatio(1f)
            .clickable(onClick = onClick),
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
