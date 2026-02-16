package com.pika.app.ui.theme

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color

private val LightColors =
    lightColorScheme(
        primary = PikaBlue,
        surface = Color.White,
        background = PikaBg,
    )

private val DarkColors =
    darkColorScheme(
        primary = PikaBlueDark,
        surface = Color(0xFF1E1E1E),
        background = PikaBgDark,
    )

@Composable
fun PikaTheme(content: @Composable () -> Unit) {
    val colorScheme = if (isSystemInDarkTheme()) DarkColors else LightColors

    MaterialTheme(
        colorScheme = colorScheme,
        typography = androidx.compose.material3.Typography(),
        content = content,
    )
}

