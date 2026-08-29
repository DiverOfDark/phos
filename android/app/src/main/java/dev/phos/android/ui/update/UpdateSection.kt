package dev.phos.android.ui.update

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.hilt.navigation.compose.hiltViewModel
import dev.phos.android.ui.common.MonoBody
import dev.phos.android.ui.common.PhosColors
import dev.phos.android.ui.common.PhosLabel
import dev.phos.android.ui.common.PhosMonoText
import dev.phos.android.ui.common.PhosOutlinedButton
import dev.phos.android.ui.common.PhosPrimaryButton
import dev.phos.android.ui.common.SignalDot
import dev.phos.android.update.InstallState
import dev.phos.android.update.UpdateState
import java.util.Locale

/**
 * The "App update" block on the settings screen.
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
    val c = PhosColors.current
    val update = state.update

    Column(
        modifier = modifier.fillMaxWidth(),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        PhosLabel("App update")

        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            PhosMonoText(
                text = buildString {
                    append("this build ")
                    append(state.runningVersionName)
                    append(" · ")
                    append(
                        when (update) {
                            is UpdateState.Available -> "server ships ${update.versionName}"
                            UpdateState.Checking -> "checking the server"
                            is UpdateState.Failed -> "server unreachable"
                            else -> "build ${state.runningVersionCode}"
                        }
                    )
                },
                color = c.textSecondary,
                style = MonoBody,
                maxLines = 2,
                modifier = Modifier.weight(1f),
            )

            when (update) {
                is UpdateState.Available -> InstallControl(
                    available = update,
                    install = state.install,
                    onInstall = { viewModel.install(update) },
                    onDismiss = viewModel::dismiss,
                    onGrantPermission = { context.startActivity(viewModel.permissionSettingsIntent()) },
                )

                UpdateState.UpToDate -> Text(
                    text = "UP TO DATE",
                    style = MonoBody,
                    color = c.ready,
                )

                UpdateState.Checking -> SignalDot(color = c.building, size = 6.dp, pulsing = true)

                // Never checked — either the app just started or there is no server
                // yet. Says nothing rather than guessing.
                UpdateState.Unknown -> Unit

                is UpdateState.Failed -> Unit
            }
        }

        if (update is UpdateState.Failed) {
            PhosMonoText(text = update.message, color = c.error, maxLines = 2)
        }

        InstallDetail(state.install, c.textSecondary, c.error)

        Text(
            text = "APKs are checksum- and signature-verified before install.",
            style = MaterialTheme.typography.bodySmall,
            color = c.textSecondary,
        )

        PhosOutlinedButton(
            onClick = viewModel::check,
            enabled = !state.busy,
            modifier = Modifier.fillMaxWidth(),
        ) {
            Text(
                text = "Check for updates",
                style = MaterialTheme.typography.bodySmall,
                color = c.textSecondary,
            )
        }
    }
}

/** The button (or the state that replaces it) for an update that exists. */
@Composable
private fun InstallControl(
    available: UpdateState.Available,
    install: InstallState,
    onInstall: () -> Unit,
    onDismiss: () -> Unit,
    onGrantPermission: () -> Unit,
) {
    val c = PhosColors.current
    when (install) {
        InstallState.Idle -> PhosPrimaryButton(onClick = onInstall) {
            Text(
                text = "Install ${available.versionName}",
                style = MaterialTheme.typography.labelMedium,
                color = c.signalFg,
            )
        }

        is InstallState.Downloading,
        InstallState.Verifying,
        InstallState.AwaitingConfirmation,
        -> SignalDot(color = c.building, size = 6.dp, pulsing = true)

        InstallState.Installed -> Text("INSTALLED", style = MonoBody, color = c.ready)

        InstallState.Declined,
        is InstallState.Failed,
        -> PhosOutlinedButton(onClick = onDismiss) {
            Text("Try again", style = MaterialTheme.typography.bodySmall, color = c.textSecondary)
        }

        is InstallState.Refused -> PhosOutlinedButton(onClick = onDismiss) {
            Text("Dismiss", style = MaterialTheme.typography.bodySmall, color = c.error)
        }

        InstallState.PermissionRequired -> PhosOutlinedButton(onClick = onGrantPermission) {
            Text("Open settings", style = MaterialTheme.typography.bodySmall, color = c.textSecondary)
        }
    }
}

/** The sentence that explains whatever the install control is currently doing. */
@Composable
private fun InstallDetail(
    install: InstallState,
    mutedColor: androidx.compose.ui.graphics.Color,
    errorColor: androidx.compose.ui.graphics.Color,
) {
    val c = PhosColors.current
    when (install) {
        is InstallState.Downloading -> Column(Modifier.fillMaxWidth()) {
            Box(
                modifier = Modifier
                    .fillMaxWidth()
                    .height(2.dp)
                    .clip(RoundedCornerShape(1.dp))
                    .background(c.raised),
            ) {
                Box(
                    modifier = Modifier
                        .fillMaxWidth(install.fraction.coerceIn(0f, 1f))
                        .height(2.dp)
                        .background(c.signal),
                )
            }
            Spacer(Modifier.height(4.dp))
            PhosMonoText(
                text = "downloading ${formatSize(install.bytesRead)} of ${formatSize(install.totalBytes)}",
                color = mutedColor,
            )
        }

        // Named, not hidden behind a spinner: this is the step that makes installing
        // a network download defensible, and the user should see that it happens.
        InstallState.Verifying -> PhosMonoText(
            text = "checking the download's checksum and signing certificate",
            color = mutedColor,
        )

        InstallState.AwaitingConfirmation -> PhosMonoText(
            text = "waiting for Android's install confirmation",
            color = mutedColor,
        )

        InstallState.Installed -> PhosMonoText(text = "restart Phos to use it", color = mutedColor)

        InstallState.Declined -> PhosMonoText(text = "install cancelled", color = mutedColor)

        // A verification failure. Never auto-retried: the artefact is not what it
        // said it was, and downloading it again is unlikely to change that.
        is InstallState.Refused -> PhosMonoText(text = install.reason, color = errorColor, maxLines = 3)

        is InstallState.Failed -> PhosMonoText(text = install.reason, color = mutedColor, maxLines = 3)

        InstallState.PermissionRequired -> PhosMonoText(
            text = "Android needs permission to install apps from Phos",
            color = mutedColor,
            maxLines = 2,
        )

        InstallState.Idle -> Unit
    }
}

/** MB to one decimal — the only unit an APK is ever worth quoting in. */
private fun formatSize(bytes: Long): String =
    String.format(Locale.getDefault(), "%.1f MB", bytes / 1_048_576.0)
