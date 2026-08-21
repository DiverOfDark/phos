package dev.phos.android.ui.people

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.HelpOutline
import androidx.compose.material.icons.filled.RateReview
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material3.Card
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.Badge
import androidx.compose.material3.BadgedBox
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
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
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import coil3.compose.AsyncImage
import dev.phos.android.domain.model.Person
import dev.phos.android.ui.common.AuthExpiredBanner
import dev.phos.android.ui.common.ErrorBanner
import dev.phos.android.ui.common.ShimmerBox

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun PeopleScreen(
    onPersonClick: (String) -> Unit,
    onUnsortedClick: () -> Unit,
    onSettingsClick: () -> Unit,
    onReviewClick: () -> Unit,
    onReLogin: () -> Unit,
    viewModel: PeopleViewModel = hiltViewModel(),
) {
    val people by viewModel.people.collectAsState()
    val unsortedCount by viewModel.unsortedCount.collectAsState()
    val isRefreshing by viewModel.isRefreshing.collectAsState()
    val error by viewModel.error.collectAsState()
    val authExpired by viewModel.authExpired.collectAsState()

    // Sorting happens on the screens this one leads to, so the counts here — the
    // Unsorted pile above all — are stale the moment the user comes back. Inside
    // a NavHost this observer fires on returning to this destination, not on
    // every app resume, so it costs one refresh per visit.
    val lifecycleOwner = LocalLifecycleOwner.current
    // The first ON_RESUME lands right after the ViewModel's own initial load, so
    // it is skipped rather than fetching the same two responses twice.
    var skipFirstResume by remember { mutableStateOf(true) }
    LaunchedEffect(lifecycleOwner) {
        val observer = LifecycleEventObserver { _, event ->
            if (event == Lifecycle.Event.ON_RESUME) {
                if (skipFirstResume) skipFirstResume = false else viewModel.refresh()
            }
        }
        lifecycleOwner.lifecycle.addObserver(observer)
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("Phos") },
                actions = {
                    // The badge is the point: a review queue nobody can see the size
                    // of is a review queue nobody opens. The count comes from the
                    // people list that is already loaded, so it costs no extra call.
                    val pending = people.sumOf { it.pendingCount }
                    if (pending > 0) {
                        BadgedBox(
                            badge = { Badge { Text(if (pending > 99) "99+" else "$pending") } },
                        ) {
                            IconButton(onClick = onReviewClick) {
                                Icon(Icons.Default.RateReview, contentDescription = "Review pending shots")
                            }
                        }
                    } else {
                        IconButton(onClick = onReviewClick) {
                            Icon(Icons.Default.RateReview, contentDescription = "Review pending shots")
                        }
                    }
                    IconButton(onClick = onSettingsClick) {
                        Icon(Icons.Default.Settings, contentDescription = "Settings")
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
            if (authExpired) {
                AuthExpiredBanner(onReLogin = {
                    viewModel.reLogin()
                    onReLogin()
                })
            }

            error?.let { ErrorBanner(message = it) }

            PullToRefreshBox(
                isRefreshing = isRefreshing,
                onRefresh = viewModel::refresh,
                modifier = Modifier.fillMaxSize(),
            ) {
                if (people.isEmpty() && unsortedCount == 0 && !isRefreshing) {
                    Box(
                        modifier = Modifier
                            .fillMaxSize()
                            .verticalScroll(rememberScrollState()),
                        contentAlignment = Alignment.Center,
                    ) {
                        Text(
                            text = "No people found.\nPull down to refresh.",
                            textAlign = TextAlign.Center,
                            style = MaterialTheme.typography.bodyLarge,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                } else {
                    LazyVerticalGrid(
                        columns = GridCells.Fixed(2),
                        modifier = Modifier.fillMaxSize(),
                        contentPadding = PaddingValues(8.dp),
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                        verticalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        // First tile, like the web's people page: the pile the
                        // library grows every scan is the one worth opening.
                        if (unsortedCount > 0) {
                            item(key = "unsorted") {
                                UnsortedCard(count = unsortedCount, onClick = onUnsortedClick)
                            }
                        }

                        items(people, key = { it.id }) { person ->
                            PersonCard(
                                person = person,
                                coverUrl = viewModel.buildCoverUrl(person),
                                onClick = { onPersonClick(person.id) },
                            )
                        }
                    }
                }
            }
        }
    }
}

/**
 * The "shots with no person" tile.
 *
 * Deliberately not a [PersonCard] with a fake person: there is no cover face to
 * show, and an icon says "this is a bucket, not somebody" at a glance.
 */
@Composable
private fun UnsortedCard(
    count: Int,
    onClick: () -> Unit,
) {
    Card(
        modifier = Modifier
            .fillMaxWidth()
            .clickable(onClick = onClick),
    ) {
        Column {
            Box(
                modifier = Modifier
                    .fillMaxWidth()
                    .aspectRatio(1f)
                    .clip(RoundedCornerShape(topStart = 12.dp, topEnd = 12.dp))
                    .background(MaterialTheme.colorScheme.surfaceVariant),
                contentAlignment = Alignment.Center,
            ) {
                Icon(
                    imageVector = Icons.AutoMirrored.Filled.HelpOutline,
                    contentDescription = null,
                    modifier = Modifier.size(48.dp),
                    tint = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }

            Column(
                modifier = Modifier.padding(12.dp),
            ) {
                Text(
                    text = "Unsorted",
                    style = MaterialTheme.typography.titleSmall,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    text = if (count == 1) "1 shot" else "$count shots",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
    }
}

@Composable
private fun PersonCard(
    person: Person,
    coverUrl: String?,
    onClick: () -> Unit,
) {
    Card(
        modifier = Modifier
            .fillMaxWidth()
            .clickable(onClick = onClick),
    ) {
        Column {
            Box(
                modifier = Modifier
                    .fillMaxWidth()
                    .aspectRatio(1f)
                    .clip(RoundedCornerShape(topStart = 12.dp, topEnd = 12.dp)),
            ) {
                if (coverUrl != null) {
                    AsyncImage(
                        model = coverUrl,
                        contentDescription = person.name,
                        contentScale = ContentScale.Crop,
                        modifier = Modifier.fillMaxSize(),
                    )
                } else {
                    ShimmerBox(modifier = Modifier.fillMaxSize())
                }
            }

            Column(
                modifier = Modifier.padding(12.dp),
            ) {
                Text(
                    text = person.name ?: "Unknown",
                    style = MaterialTheme.typography.titleSmall,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    text = "${person.shotCount} shots",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
    }
}
