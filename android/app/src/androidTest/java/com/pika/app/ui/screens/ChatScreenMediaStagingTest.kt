package com.pika.app.ui.screens

import android.net.Uri
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class ChatScreenMediaStagingTest {
    @Test
    /** Verifies that only the remaining attachment slots are accepted for staging. */
    fun selectUrisForStaging_respectsRemainingCapacity() {
        val uris = (1..5).map { Uri.parse("content://staging/$it") }

        val accepted = selectUrisForStaging(existingCount = 30, uris = uris)

        assertEquals(listOf(uris[0], uris[1]), accepted)
    }

    @Test
    /** Verifies that no URIs are accepted once the staging cap has already been reached. */
    fun selectUrisForStaging_returnsEmptyWhenAlreadyFull() {
        val uris = listOf(Uri.parse("content://staging/1"))

        val accepted = selectUrisForStaging(existingCount = MAX_STAGED_MEDIA_ITEMS, uris = uris)

        assertTrue(accepted.isEmpty())
    }
}
