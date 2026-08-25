package dev.phos.android.ui.settings

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.hilt.navigation.compose.hiltViewModel
import dev.phos.android.ui.update.UpdateSection

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SettingsScreen(
    onBack: () -> Unit,
    onLogout: () -> Unit,
    viewModel: SettingsViewModel = hiltViewModel(),
) {
    val uiState by viewModel.uiState.collectAsState()

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("Settings") },
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
            // In-app update: what this build is, and what the configured server ships.
            // Owns its own ViewModel because the update state is app-wide (the
            // app-start check has usually already answered by the time this opens).
            UpdateSection(modifier = Modifier.padding(horizontal = 16.dp, vertical = 12.dp))

            HorizontalDivider()

            ListItem(
                headlineContent = { Text("Server") },
                supportingContent = { Text(uiState.serverUrl.ifBlank { "Not configured" }) },
            )

            HorizontalDivider()

            ListItem(
                headlineContent = { Text("Image cache") },
                supportingContent = { Text(uiState.cacheSize) },
                trailingContent = {
                    if (uiState.isClearing) {
                        CircularProgressIndicator(
                            modifier = Modifier.padding(8.dp),
                            strokeWidth = 2.dp,
                        )
                    }
                },
                modifier = Modifier.clickable(enabled = !uiState.isClearing) {
                    viewModel.clearCache()
                },
            )

            HorizontalDivider()

            // Two taps, because the delete cannot be undone: the first counts the
            // duplicate boxes, the second removes exactly that many.
            ListItem(
                headlineContent = { Text("Duplicate face boxes") },
                supportingContent = {
                    Text(
                        uiState.dedupeMessage
                            ?: when {
                                uiState.dedupePending > 0 ->
                                    "${uiState.dedupePending} found — tap again to remove. " +
                                        "This can't be undone."
                                else ->
                                    "Collapse overlapping rectangles on one face. Boxes " +
                                        "assigned to different people are never merged, and " +
                                        "reviewed shots are left alone."
                            },
                        color = if (uiState.dedupePending > 0) {
                            MaterialTheme.colorScheme.error
                        } else {
                            MaterialTheme.colorScheme.onSurfaceVariant
                        },
                    )
                },
                trailingContent = {
                    if (uiState.dedupeBusy) {
                        CircularProgressIndicator(
                            modifier = Modifier.padding(8.dp),
                            strokeWidth = 2.dp,
                        )
                    }
                },
                modifier = Modifier.clickable(enabled = !uiState.dedupeBusy) {
                    if (uiState.dedupePending > 0) {
                        viewModel.removeDuplicateFaces()
                    } else {
                        viewModel.findDuplicateFaces()
                    }
                },
            )

            HorizontalDivider()

            ListItem(
                headlineContent = {
                    Text(
                        "Sign out",
                        color = MaterialTheme.colorScheme.error,
                    )
                },
                modifier = Modifier.clickable {
                    viewModel.logout()
                    onLogout()
                },
            )
        }
    }
}
