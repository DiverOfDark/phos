package dev.phos.android.ui.people

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
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
import dev.phos.android.R
import dev.phos.android.domain.model.Person
import dev.phos.android.ui.common.AuthExpiredBanner
import dev.phos.android.ui.common.ErrorBanner
import dev.phos.android.ui.common.MonoSmall
import dev.phos.android.ui.common.PhosColors
import dev.phos.android.ui.common.PhosLabel
import dev.phos.android.ui.common.PhosMonoText
import dev.phos.android.ui.common.PhosOutlinedButton
import dev.phos.android.ui.common.PhosSearchField
import dev.phos.android.ui.common.PhosTopBar
import dev.phos.android.ui.common.SignalDot
import dev.phos.android.ui.common.relativeSince

/**
 * People — a wall of faces.
 *
 * The design system keeps imagery out of the console, but that rule was written
 * for an infrastructure product. Here the imagery *is* the data: a face is what
 * the eye matches a name to, so the crop gets the tile and the counts go
 * underneath in mono.
 */
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
    val lastSyncedAt by viewModel.lastSyncedAt.collectAsState()
    val sort by viewModel.sort.collectAsState()
    val query by viewModel.query.collectAsState()
    val c = PhosColors.current

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

    val pending = people.sumOf { it.pendingCount }

    val visible = remember(people, query, sort) {
        val q = query.trim().lowercase()
        val filtered = if (q.isEmpty()) people
        else people.filter { (it.name ?: "unnamed").lowercase().contains(q) }
        when (sort) {
            // Clusters with no name have nothing to sort by, so they go last
            // rather than clumping under an empty string at the top.
            PeopleSort.NAME -> filtered.sortedWith(
                compareBy(nullsLast()) { it.name?.lowercase() }
            )
            PeopleSort.PENDING -> filtered.sortedByDescending { it.pendingCount }
            PeopleSort.SHOTS -> filtered.sortedByDescending { it.shotCount }
        }
    }

    Scaffold(
        containerColor = c.base,
        topBar = {
            PhosTopBar {
                AsyncImage(
                    model = R.mipmap.ic_launcher,
                    contentDescription = null,
                    modifier = Modifier.size(20.dp),
                )
                Text(
                    text = "Phos",
                    style = MaterialTheme.typography.titleLarge,
                    color = c.textPrimary,
                    modifier = Modifier.weight(1f),
                )
                PhosOutlinedButton(onClick = onReviewClick) {
                    Text("Review", style = MaterialTheme.typography.bodySmall, color = c.textSecondary)
                    if (pending > 0) {
                        PhosMonoText(
                            text = if (pending > 99) "99+" else "$pending",
                            color = c.degraded,
                        )
                    }
                }
                PhosOutlinedButton(onClick = onSettingsClick) {
                    PhosMonoText("≡", color = c.textSecondary, style = MonoSmall)
                }
            }
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
                        Column(
                            horizontalAlignment = Alignment.CenterHorizontally,
                            verticalArrangement = Arrangement.spacedBy(8.dp),
                        ) {
                            SignalDot(color = c.stopped, size = 10.dp)
                            Text(
                                text = "Nothing indexed yet",
                                style = MaterialTheme.typography.titleMedium,
                                color = c.textPrimary,
                            )
                            Text(
                                text = "Scan a library on the server, then pull down.",
                                textAlign = TextAlign.Center,
                                style = MaterialTheme.typography.bodySmall,
                                color = c.textSecondary,
                            )
                        }
                    }
                } else {
                    LazyVerticalGrid(
                        columns = GridCells.Adaptive(minSize = 108.dp),
                        modifier = Modifier.fillMaxSize(),
                        contentPadding = PaddingValues(16.dp),
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                        verticalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        // Search and sort. Past about thirty faces, scanning stops
                        // working and the library needs a way to be asked.
                        item(key = "filter", span = { GridItemSpan(maxLineSpan) }) {
                            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                                PhosSearchField(
                                    value = query,
                                    onValueChange = viewModel::setQuery,
                                    placeholder = "Search people…",
                                )
                                Row(
                                    verticalAlignment = Alignment.CenterVertically,
                                    horizontalArrangement = Arrangement.spacedBy(12.dp),
                                ) {
                                    PhosLabel("Sort")
                                    for (option in PeopleSort.entries) {
                                        Text(
                                            text = option.label,
                                            style = MonoSmall,
                                            color = if (sort == option) c.signal else c.textTertiary,
                                            modifier = Modifier
                                                .clickable { viewModel.setSort(option) }
                                                .padding(vertical = 4.dp),
                                        )
                                    }
                                }
                            }
                        }

                        // The pile the library grows every scan is the one worth
                        // opening, so it leads the wall.
                        if (unsortedCount > 0 && query.isBlank()) {
                            item(key = "unsorted") {
                                UnsortedTile(count = unsortedCount, onClick = onUnsortedClick)
                            }
                        }

                        items(visible, key = { it.id }) { person ->
                            PersonTile(
                                person = person,
                                faceUrl = viewModel.buildFaceUrl(person),
                                onClick = { onPersonClick(person.id) },
                            )
                        }

                        if (visible.isEmpty() && query.isNotBlank()) {
                            item(key = "empty", span = { GridItemSpan(maxLineSpan) }) {
                                PhosMonoText(
                                    text = "nobody matches “$query”",
                                    modifier = Modifier.padding(vertical = 24.dp),
                                )
                            }
                        }

                        item(key = "sync", span = { GridItemSpan(maxLineSpan) }) {
                            Row(
                                modifier = Modifier
                                    .fillMaxWidth()
                                    .padding(vertical = 12.dp),
                                verticalAlignment = Alignment.CenterVertically,
                                horizontalArrangement = Arrangement.spacedBy(8.dp),
                            ) {
                                SignalDot(
                                    color = if (lastSyncedAt != null) c.ready else c.stopped,
                                    size = 6.dp,
                                )
                                PhosMonoText(
                                    text = "synced ${relativeSince(lastSyncedAt)} · " +
                                        if (query.isBlank()) "${people.size} people"
                                        else "${visible.size} of ${people.size} shown",
                                )
                            }
                        }
                    }
                }
            }
        }
    }
}

/** One face, its name, and the two counts worth knowing at a glance. */
@Composable
private fun PersonTile(
    person: Person,
    faceUrl: String?,
    onClick: () -> Unit,
) {
    val c = PhosColors.current
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(4.dp))
            .background(c.surface)
            .border(1.dp, c.line, RoundedCornerShape(4.dp))
            .clickable(onClick = onClick),
    ) {
        Box(
            modifier = Modifier
                .fillMaxWidth()
                .aspectRatio(1f)
                .background(c.raised),
        ) {
            if (faceUrl != null) {
                AsyncImage(
                    model = faceUrl,
                    contentDescription = person.name,
                    contentScale = ContentScale.Crop,
                    modifier = Modifier.fillMaxSize(),
                )
            } else {
                Text(
                    text = (person.name ?: "?").take(1).uppercase(),
                    style = MaterialTheme.typography.headlineSmall,
                    color = c.textTertiary,
                    modifier = Modifier.align(Alignment.Center),
                )
            }

            // Work waiting on this person, without reading a column.
            if (person.pendingCount > 0) {
                Row(
                    modifier = Modifier
                        .align(Alignment.TopEnd)
                        .padding(4.dp)
                        .clip(RoundedCornerShape(2.dp))
                        .background(c.base)
                        .border(1.dp, c.line, RoundedCornerShape(2.dp))
                        .padding(horizontal = 4.dp, vertical = 1.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(3.dp),
                ) {
                    SignalDot(color = c.degraded, size = 5.dp)
                    Text("${person.pendingCount}", style = MonoSmall, color = c.degraded)
                }
            }
        }

        Column(modifier = Modifier.padding(horizontal = 6.dp, vertical = 5.dp)) {
            Text(
                text = person.name ?: "unnamed cluster",
                style = MaterialTheme.typography.labelMedium,
                color = if (person.name != null) c.textPrimary else c.textTertiary,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            PhosMonoText("${person.shotCount} · ${person.faceCount}")
        }
    }
}

/**
 * The "shots with no person" tile.
 *
 * Deliberately not a [PersonTile] with a fake person: there is no face to show,
 * and the dashed square says "this is a bucket, not somebody" at a glance.
 */
@Composable
private fun UnsortedTile(
    count: Int,
    onClick: () -> Unit,
) {
    val c = PhosColors.current
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(4.dp))
            .background(c.surface)
            .border(1.dp, c.line, RoundedCornerShape(4.dp))
            .clickable(onClick = onClick),
    ) {
        Box(
            modifier = Modifier
                .fillMaxWidth()
                .aspectRatio(1f)
                .background(c.raised),
            contentAlignment = Alignment.Center,
        ) {
            Text(
                text = "?",
                style = MaterialTheme.typography.headlineSmall,
                color = c.textTertiary,
            )
        }
        Column(modifier = Modifier.padding(horizontal = 6.dp, vertical = 5.dp)) {
            Text(
                text = "Unsorted",
                style = MaterialTheme.typography.labelMedium,
                color = c.textPrimary,
                maxLines = 1,
            )
            PhosMonoText("$count · no person")
        }
    }
}
