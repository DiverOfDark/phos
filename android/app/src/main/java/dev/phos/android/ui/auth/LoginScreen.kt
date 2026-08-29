package dev.phos.android.ui.auth

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.dp
import androidx.hilt.navigation.compose.hiltViewModel
import coil3.compose.AsyncImage
import dev.phos.android.BuildConfig
import dev.phos.android.R
import dev.phos.android.ui.common.MonoBody
import dev.phos.android.ui.common.PhosColors
import dev.phos.android.ui.common.PhosLabel
import dev.phos.android.ui.common.PhosMonoText
import dev.phos.android.ui.common.PhosOutlinedButton
import dev.phos.android.ui.common.PhosPrimaryButton
import dev.phos.android.ui.common.SignalDot

/**
 * Login — server first, then the identity provider.
 *
 * The OIDC fields stay visible but quiet: most installs auto-detect, and the two
 * people who need to type an issuer by hand should not have to go find a
 * disclosure triangle to do it.
 */
@Composable
fun LoginScreen(
    onLoginSuccess: () -> Unit,
    viewModel: LoginViewModel = hiltViewModel(),
) {
    val uiState by viewModel.uiState.collectAsState()
    val authIntent by viewModel.authIntent.collectAsState()
    val context = LocalContext.current
    val c = PhosColors.current

    val authLauncher = rememberLauncherForActivityResult(
        contract = ActivityResultContracts.StartActivityForResult(),
    ) { result ->
        viewModel.handleAuthResult(result.data)
    }

    LaunchedEffect(authIntent) {
        authIntent?.let {
            authLauncher.launch(it)
            viewModel.clearAuthIntent()
        }
    }

    LaunchedEffect(uiState.isLoggedIn) {
        if (uiState.isLoggedIn) {
            onLoginSuccess()
        }
    }

    Scaffold(containerColor = c.base) { innerPadding ->
        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(innerPadding)
                .padding(32.dp),
            verticalArrangement = Arrangement.Center,
        ) {
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(16.dp),
            ) {
                AsyncImage(
                    model = R.mipmap.ic_launcher,
                    contentDescription = null,
                    modifier = Modifier.size(40.dp),
                )
                Column {
                    Text(
                        text = "Phos",
                        style = MaterialTheme.typography.headlineMedium,
                        color = c.textPrimary,
                    )
                    PhosMonoText("android client")
                }
            }

            Spacer(modifier = Modifier.height(32.dp))

            PhosLabel("Server")
            Spacer(modifier = Modifier.height(8.dp))
            PhosField(
                value = uiState.serverUrl,
                onValueChange = viewModel::updateServerUrl,
                placeholder = "https://phos.example.com",
                mono = true,
            )

            Spacer(modifier = Modifier.height(8.dp))

            PhosOutlinedButton(
                onClick = viewModel::fetchAuthConfig,
                enabled = uiState.serverUrl.isNotBlank() && !uiState.isFetchingConfig,
                modifier = Modifier.fillMaxWidth(),
            ) {
                if (uiState.isFetchingConfig) {
                    SignalDot(color = c.building, size = 6.dp, pulsing = true)
                }
                Text(
                    text = if (uiState.isFetchingConfig) "Detecting…" else "Auto-detect auth config",
                    style = MaterialTheme.typography.bodySmall,
                    color = c.textSecondary,
                )
            }

            Spacer(modifier = Modifier.height(24.dp))

            PhosLabel("OIDC issuer")
            Spacer(modifier = Modifier.height(8.dp))
            PhosField(
                value = uiState.oidcIssuer,
                onValueChange = viewModel::updateOidcIssuer,
                placeholder = "https://auth.example.com",
                mono = true,
            )

            Spacer(modifier = Modifier.height(16.dp))

            PhosLabel("OIDC client id")
            Spacer(modifier = Modifier.height(8.dp))
            PhosField(
                value = uiState.oidcClientId,
                onValueChange = viewModel::updateOidcClientId,
                placeholder = "phos-android",
                mono = true,
                enabled = uiState.oidcIssuer.isNotBlank(),
            )

            Spacer(modifier = Modifier.height(24.dp))

            uiState.error?.let {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    SignalDot(color = c.error, size = 6.dp)
                    Text(text = it, style = MonoBody, color = c.error)
                }
                Spacer(modifier = Modifier.height(16.dp))
            }
            if (uiState.error == null) {
                uiState.info?.let {
                    Row(
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        SignalDot(color = c.ready, size = 6.dp)
                        Text(text = it, style = MonoBody, color = c.textSecondary)
                    }
                    Spacer(modifier = Modifier.height(16.dp))
                }
            }

            PhosPrimaryButton(
                onClick = { viewModel.startLogin(context) },
                enabled = !uiState.isLoading && uiState.serverUrl.isNotBlank(),
                modifier = Modifier.fillMaxWidth(),
            ) {
                Text(
                    text = when {
                        uiState.isLoading -> "Connecting…"
                        uiState.oidcIssuer.isBlank() -> "Connect"
                        else -> "Sign in with SSO"
                    },
                    style = MaterialTheme.typography.labelLarge,
                    color = c.signalFg,
                )
            }

            Spacer(modifier = Modifier.height(16.dp))

            Text(
                text = "Opens your identity provider in the browser. No password is stored in the app.",
                style = MaterialTheme.typography.bodySmall,
                color = c.textSecondary,
            )

            Spacer(modifier = Modifier.height(8.dp))

            PhosMonoText("phos-android ${BuildConfig.VERSION_NAME} · build ${BuildConfig.VERSION_CODE}")
        }
    }
}

/**
 * A 2dp-radius input on the base layer.
 *
 * Material's [androidx.compose.material3.OutlinedTextField] brings a floating
 * label, a 4dp-inset outline and 56dp of height with it; the system asks for a
 * plain rectangle with an uppercase mono label above, so this draws that.
 */
@Composable
private fun PhosField(
    value: String,
    onValueChange: (String) -> Unit,
    placeholder: String,
    modifier: Modifier = Modifier,
    mono: Boolean = false,
    enabled: Boolean = true,
) {
    val c = PhosColors.current
    val textStyle = if (mono) {
        MonoBody.copy(color = c.textPrimary)
    } else {
        MaterialTheme.typography.bodyMedium.copy(color = c.textPrimary)
    }
    Column(modifier = modifier.fillMaxWidth()) {
        BasicTextField(
            value = value,
            onValueChange = onValueChange,
            enabled = enabled,
            singleLine = true,
            textStyle = textStyle,
            cursorBrush = androidx.compose.ui.graphics.SolidColor(c.signal),
            keyboardOptions = androidx.compose.foundation.text.KeyboardOptions(imeAction = ImeAction.Next),
            modifier = Modifier
                .fillMaxWidth()
                .background(c.surface, RoundedCornerShape(2.dp))
                .border(1.dp, c.line, RoundedCornerShape(2.dp))
                .padding(horizontal = 12.dp, vertical = 12.dp),
            decorationBox = { inner ->
                if (value.isEmpty()) {
                    Text(text = placeholder, style = textStyle.copy(color = c.textTertiary))
                }
                inner()
            },
        )
    }
}
