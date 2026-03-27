package dev.phos.android.ui.settings

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.hilt.navigation.compose.hiltViewModel
import dev.phos.android.data.update.UpdateState

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
            // Update section
            UpdateListItem(
                updateState = uiState.updateState,
                currentVersion = uiState.currentVersion,
                onCheck = viewModel::checkForUpdate,
                onDownload = viewModel::downloadUpdate,
                onInstall = viewModel::installUpdate,
            )

            HorizontalDivider()

            ListItem(
                headlineContent = { Text("Server") },
                supportingContent = { Text(uiState.serverUrl.ifBlank { "Not configured" }) },
            )

            HorizontalDivider()

            ListItem(
                headlineContent = { Text("Sync over Wi-Fi only") },
                supportingContent = { Text("Only sync metadata when connected to Wi-Fi") },
                trailingContent = {
                    Switch(
                        checked = uiState.wifiOnlySync,
                        onCheckedChange = viewModel::setWifiOnlySync,
                    )
                },
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

            ListItem(
                headlineContent = { Text("Metadata cache") },
                supportingContent = { Text("Clear locally cached people, shots, and files") },
                trailingContent = {
                    if (uiState.isClearingMetadata) {
                        CircularProgressIndicator(
                            modifier = Modifier.padding(8.dp),
                            strokeWidth = 2.dp,
                        )
                    }
                },
                modifier = Modifier.clickable(enabled = !uiState.isClearingMetadata) {
                    viewModel.clearMetadataCache()
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

@Composable
private fun UpdateListItem(
    updateState: UpdateState,
    currentVersion: String,
    onCheck: () -> Unit,
    onDownload: () -> Unit,
    onInstall: () -> Unit,
) {
    when (updateState) {
        is UpdateState.Idle, is UpdateState.UpToDate -> {
            ListItem(
                headlineContent = { Text("App version") },
                supportingContent = {
                    val status = if (updateState is UpdateState.UpToDate) "Up to date" else "Check for updates"
                    Text("Phos v$currentVersion — $status")
                },
                modifier = Modifier.clickable(onClick = onCheck),
            )
        }

        is UpdateState.Checking -> {
            ListItem(
                headlineContent = { Text("App version") },
                supportingContent = { Text("Phos v$currentVersion — Checking...") },
                trailingContent = {
                    CircularProgressIndicator(
                        modifier = Modifier.size(24.dp),
                        strokeWidth = 2.dp,
                    )
                },
            )
        }

        is UpdateState.Available -> {
            ListItem(
                headlineContent = { Text("Update available") },
                supportingContent = { Text("Version ${updateState.version}") },
                trailingContent = {
                    Button(onClick = onDownload) {
                        Text("Download")
                    }
                },
            )
        }

        is UpdateState.Downloading -> {
            ListItem(
                headlineContent = { Text("Downloading update...") },
                supportingContent = {
                    Column {
                        Spacer(modifier = Modifier.height(4.dp))
                        LinearProgressIndicator(
                            progress = { updateState.progress },
                            modifier = Modifier.fillMaxWidth(),
                        )
                        Spacer(modifier = Modifier.height(4.dp))
                        Text("${(updateState.progress * 100).toInt()}%")
                    }
                },
            )
        }

        is UpdateState.ReadyToInstall -> {
            ListItem(
                headlineContent = { Text("Update ready") },
                supportingContent = { Text("Download complete") },
                trailingContent = {
                    Button(onClick = onInstall) {
                        Text("Install")
                    }
                },
            )
        }

        is UpdateState.Error -> {
            ListItem(
                headlineContent = { Text("Update check failed") },
                supportingContent = {
                    Text(
                        updateState.message,
                        color = MaterialTheme.colorScheme.error,
                    )
                },
                modifier = Modifier.clickable(onClick = onCheck),
            )
        }
    }
}
