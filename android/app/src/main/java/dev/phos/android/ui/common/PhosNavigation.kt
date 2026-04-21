package dev.phos.android.ui.common

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.navigation.NavType
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import androidx.navigation.compose.rememberNavController
import androidx.navigation.navArgument
import dev.phos.android.ui.auth.LoginScreen
import dev.phos.android.ui.browser.BrowserScreen
import dev.phos.android.ui.grid.PersonGridScreen
import dev.phos.android.ui.people.PeopleScreen
import dev.phos.android.ui.settings.SettingsScreen

object Routes {
    const val LOGIN = "login"
    const val PEOPLE = "people"
    const val GRID = "grid/{personId}"
    const val BROWSER = "browser/{personId}?shot={shot}"
    const val SETTINGS = "settings"

    fun grid(personId: String) = "grid/$personId"
    fun browser(personId: String, shotIndex: Int = -1) = "browser/$personId?shot=$shotIndex"
}

@Composable
fun PhosNavigation() {
    val navController = rememberNavController()

    NavHost(
        navController = navController,
        startDestination = Routes.LOGIN,
    ) {
        composable(Routes.LOGIN) {
            LoginScreen(
                onLoginSuccess = {
                    navController.navigate(Routes.PEOPLE) {
                        popUpTo(Routes.LOGIN) { inclusive = true }
                    }
                }
            )
        }

        composable(Routes.PEOPLE) {
            PeopleScreen(
                onPersonClick = { personId ->
                    navController.navigate(Routes.grid(personId))
                },
                onSettingsClick = {
                    navController.navigate(Routes.SETTINGS)
                },
                onReLogin = {
                    navController.navigate(Routes.LOGIN) {
                        popUpTo(0) { inclusive = true }
                    }
                },
            )
        }

        composable(
            route = Routes.GRID,
            arguments = listOf(navArgument("personId") { type = NavType.StringType }),
        ) { backStackEntry ->
            val personId = backStackEntry.arguments?.getString("personId") ?: return@composable
            PersonGridScreen(
                onBack = { navController.popBackStack() },
                onTileClick = { shotIndex ->
                    navController.navigate(Routes.browser(personId, shotIndex))
                },
            )
        }

        composable(
            route = Routes.BROWSER,
            arguments = listOf(
                navArgument("personId") { type = NavType.StringType },
                navArgument("shot") {
                    type = NavType.IntType
                    defaultValue = -1
                },
            ),
        ) {
            BrowserScreen(
                onBack = { navController.popBackStack() },
            )
        }

        composable(Routes.SETTINGS) {
            SettingsScreen(
                onBack = { navController.popBackStack() },
                onLogout = {
                    navController.navigate(Routes.LOGIN) {
                        popUpTo(0) { inclusive = true }
                    }
                },
            )
        }
    }
}

@Composable
fun AuthExpiredBanner(
    onReLogin: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Box(
        modifier = modifier
            .fillMaxWidth()
            .background(MaterialTheme.colorScheme.errorContainer)
            .clickable(onClick = onReLogin)
            .padding(12.dp),
    ) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(
                text = "Session expired.",
                color = MaterialTheme.colorScheme.onErrorContainer,
                style = MaterialTheme.typography.bodySmall,
                modifier = Modifier.weight(1f),
            )
            Spacer(modifier = Modifier.width(8.dp))
            Text(
                text = "Sign in",
                color = MaterialTheme.colorScheme.error,
                style = MaterialTheme.typography.labelMedium,
            )
        }
    }
}
