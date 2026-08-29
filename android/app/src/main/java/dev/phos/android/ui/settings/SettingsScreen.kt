package dev.phos.android.ui.settings

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import androidx.hilt.navigation.compose.hiltViewModel
import dev.phos.android.ui.common.MonoSmall
import dev.phos.android.ui.common.PhosColors
import dev.phos.android.ui.common.PhosDivider
import dev.phos.android.ui.common.PhosMonoText
import dev.phos.android.ui.common.PhosTopBar
import dev.phos.android.ui.common.SignalDot
import dev.phos.android.ui.update.UpdateSection

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SettingsScreen(
    onBack: () -> Unit,
    onLogout: () -> Unit,
    viewModel: SettingsViewModel = hiltViewModel(),
) {
    val uiState by viewModel.uiState.collectAsState()
    val c = PhosColors.current

    Scaffold(
        containerColor = c.base,
        topBar = {
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
                    text = "Settings",
                    style = MaterialTheme.typography.titleMedium,
                    color = c.textPrimary,
                )
            }
        },
    ) { innerPadding ->
        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(innerPadding)
                .verticalScroll(rememberScrollState()),
        ) {
            // In-app update: what this build is, and what the configured server ships.
            // Owns its own ViewModel because the update state is app-wide (the
            // app-start check has usually already answered by the time this opens).
            UpdateSection(modifier = Modifier.padding(16.dp))

            PhosDivider()

            SettingsRow(title = "Server", detail = uiState.serverUrl.ifBlank { "not configured" })

            SettingsRow(
                title = "Image cache",
                detail = if (uiState.isClearing) "clearing…" else "${uiState.cacheSize} — tap to clear",
                busy = uiState.isClearing,
                onClick = if (uiState.isClearing) null else viewModel::clearCache,
            )

            // Two taps, because the delete cannot be undone: the first counts the
            // duplicate boxes, the second removes exactly that many.
            SettingsRow(
                title = "Duplicate face boxes",
                detail = uiState.dedupeMessage
                    ?: if (uiState.dedupePending > 0) {
                        "${uiState.dedupePending} found — tap again to remove. This can't be undone."
                    } else {
                        "Collapse overlapping rectangles on one face. Two taps: count, then remove."
                    },
                detailColor = if (uiState.dedupePending > 0) c.degraded else c.textSecondary,
                detailMono = false,
                busy = uiState.dedupeBusy,
                onClick = if (uiState.dedupeBusy) {
                    null
                } else {
                    {
                        if (uiState.dedupePending > 0) {
                            viewModel.removeDuplicateFaces()
                        } else {
                            viewModel.findDuplicateFaces()
                        }
                    }
                },
            )

            SettingsRow(
                title = "Sign out",
                titleColor = c.error,
                onClick = {
                    viewModel.logout()
                    onLogout()
                },
            )
        }
    }
}

/**
 * One setting: a title, a line of detail underneath, an optional pulsing dot
 * while the row is working. No switches — every one of these does something the
 * moment it is tapped, and a switch would promise a state it does not hold.
 */
@Composable
private fun SettingsRow(
    title: String,
    detail: String? = null,
    detailColor: Color? = null,
    detailMono: Boolean = true,
    busy: Boolean = false,
    titleColor: Color? = null,
    onClick: (() -> Unit)? = null,
) {
    val c = PhosColors.current
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .then(if (onClick != null) Modifier.clickable(onClick = onClick) else Modifier)
            .padding(16.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Column(modifier = Modifier.weight(1f)) {
            Text(
                text = title,
                style = MaterialTheme.typography.bodyMedium,
                color = titleColor ?: c.textPrimary,
            )
            if (detail != null) {
                if (detailMono) {
                    PhosMonoText(
                        text = detail,
                        color = detailColor ?: c.textTertiary,
                        maxLines = 2,
                        modifier = Modifier.padding(top = 2.dp),
                    )
                } else {
                    Text(
                        text = detail,
                        style = MaterialTheme.typography.bodySmall,
                        color = detailColor ?: c.textSecondary,
                        modifier = Modifier.padding(top = 2.dp),
                    )
                }
            }
        }
        if (busy) SignalDot(color = c.building, size = 6.dp, pulsing = true)
    }
    PhosDivider()
}
