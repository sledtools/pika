package com.pika.app.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import coil.compose.SubcomposeAsyncImage

@Composable
fun Avatar(
    name: String?,
    npub: String,
    pictureUrl: String?,
    size: Dp = 44.dp,
) {
    if (!pictureUrl.isNullOrBlank()) {
        val placeholder: @Composable () -> Unit = { InitialsCircle(name, npub, size) }
        SubcomposeAsyncImage(
            model = pictureUrl,
            contentDescription = name ?: npub,
            contentScale = ContentScale.Crop,
            modifier = Modifier.size(size).clip(CircleShape),
            loading = { placeholder() },
            error = { placeholder() },
        )
    } else {
        InitialsCircle(name = name, npub = npub, size = size)
    }
}

@Composable
private fun InitialsCircle(name: String?, npub: String, size: Dp) {
    val initial = (name ?: npub).take(1).uppercase()
    Box(
        modifier = Modifier
            .size(size)
            .clip(CircleShape)
            .background(MaterialTheme.colorScheme.primaryContainer),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            initial,
            fontSize = (size.value * 0.4f).sp,
            color = MaterialTheme.colorScheme.onPrimaryContainer,
        )
    }
}
