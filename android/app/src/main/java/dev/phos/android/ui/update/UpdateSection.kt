package dev.phos.android.ui.update

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.hilt.navigation.compose.hiltViewModel
import dev.phos.android.update.InstallState
import dev.phos.android.update.UpdateState
import java.util.Locale

/**
 * The "App version / check for updates" block on the settings screen.
 *
 * Always shows what is running, because that is the fact a self-hosting user needs
 * when something looks wrong; shows what is available only when there genuinely is
 * something newer.
 */
@Composable
fun UpdateSection(
    modifier: Modifier = Modifier,
    viewModel: UpdateViewModel = hiltViewModel(),
) {
    val state by viewModel.uiState.collectAsState()
    val context = LocalContext.current

    Column(modifier = modifier.fillMaxWidth()) {
        Text("App version", style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.Medium)
        Spacer(Modifier.height(4.dp))
        Text(
            "Running ${state.runningVersionName} (build ${state.runningVersionCode})",
            style = MaterialTheme.typography.bodyMedium,
        )

        Spacer(Modifier.height(8.dp))

        when (val update = state.update) {
            is UpdateState.Available -> AvailableRow(
                available = update,
                install = state.install,
                onInstall = { viewModel.install(update) },
                onDismiss = viewModel::dismiss,
                onGrantPermission = { context.startActivity(viewModel.permissionSettingsIntent()) },
            )

            UpdateState.UpToDate -> Muted("This is the newest build the server has.")

            UpdateState.Checking -> Muted("Checking…")

            // Never checked — either the app just started or there is no server yet.
            // Says nothing rather than guessing.
            UpdateState.Unknown -> Unit

            is UpdateState.Failed -> Muted(update.message)
        }

        Spacer(Modifier.height(8.dp))
        OutlinedButton(
            onClick = viewModel::check,
            enabled = !state.busy,
            modifier = Modifier.fillMaxWidth(),
        ) {
            Text("Check for updates")
        }
    }
}

@Composable
private fun AvailableRow(
    available: UpdateState.Available,
    install: InstallState,
    onInstall: () -> Unit,
    onDismiss: () -> Unit,
    onGrantPermission: () -> Unit,
) {
    Column(Modifier.fillMaxWidth()) {
        Text(
            "${available.versionName} (build ${available.versionCode}) is available — " +
                formatSize(available.sizeBytes),
            style = MaterialTheme.typography.bodyMedium,
            fontWeight = FontWeight.Medium,
        )
        Spacer(Modifier.height(8.dp))

        when (install) {
            InstallState.Idle -> Button(onClick = onInstall, modifier = Modifier.fillMaxWidth()) {
                Text("Download and install")
            }

            is InstallState.Downloading -> Column(Modifier.fillMaxWidth()) {
                LinearProgressIndicator(
                    progress = { install.fraction },
                    modifier = Modifier.fillMaxWidth(),
                )
                Spacer(Modifier.height(4.dp))
                Muted("Downloading ${formatSize(install.bytesRead)} of ${formatSize(install.totalBytes)}")
            }

            InstallState.Verifying ->
                // Named, not hidden behind a spinner: this is the step that makes
                // installing a network download defensible, and the user should see
                // that it happens.
                Muted("Checking the download's checksum and signing certificate…")

            InstallState.AwaitingConfirmation ->
                Muted("Waiting for Android's install confirmation.")

            InstallState.Installed -> Muted("Installed. Restart Phos to use it.")

            InstallState.Declined -> Column {
                Muted("Install cancelled.")
                TextButton(onClick = onDismiss) { Text("Try again") }
            }

            // A verification failure. Rendered in the error colour and never auto-retried:
            // the artefact is not what it said it was, and downloading it again is unlikely
            // to change that.
            is InstallState.Refused -> Column {
                Loud(install.reason)
                TextButton(onClick = onDismiss) { Text("Dismiss") }
            }

            is InstallState.Failed -> Column {
                Muted(install.reason)
                TextButton(onClick = onDismiss) { Text("Try again") }
            }

            InstallState.PermissionRequired -> Column {
                Muted(
                    "Android needs permission to install apps from Phos. " +
                        "This is the switch that lets the app update itself."
                )
                TextButton(onClick = onGrantPermission) { Text("Open settings") }
            }
        }
    }
}

@Composable
private fun Muted(text: String) {
    Text(
        text,
        style = MaterialTheme.typography.bodySmall,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
    )
}

@Composable
private fun Loud(text: String) {
    Text(
        text,
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(8.dp))
            .background(MaterialTheme.colorScheme.errorContainer)
            .padding(12.dp),
        style = MaterialTheme.typography.bodySmall,
        color = MaterialTheme.colorScheme.onErrorContainer,
    )
}

/** MB to one decimal — the only unit an APK is ever worth quoting in. */
private fun formatSize(bytes: Long): String =
    String.format(Locale.getDefault(), "%.1f MB", bytes / 1_048_576.0)
