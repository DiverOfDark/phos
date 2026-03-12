package dev.phos.android.ui.common

import androidx.compose.runtime.Composable
import androidx.navigation.NavType
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import androidx.navigation.compose.rememberNavController
import androidx.navigation.navArgument
import dev.phos.android.ui.auth.LoginScreen
import dev.phos.android.ui.browser.BrowserScreen
import dev.phos.android.ui.people.PeopleScreen
import dev.phos.android.ui.settings.SettingsScreen

object Routes {
    const val LOGIN = "login"
    const val PEOPLE = "people"
    const val BROWSER = "browser/{personId}"
    const val SETTINGS = "settings"

    fun browser(personId: String) = "browser/$personId"
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
                    navController.navigate(Routes.browser(personId))
                },
                onSettingsClick = {
                    navController.navigate(Routes.SETTINGS)
                },
            )
        }

        composable(
            route = Routes.BROWSER,
            arguments = listOf(navArgument("personId") { type = NavType.StringType }),
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
