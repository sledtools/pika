package com.pika.app.ui.screens

import android.content.Context
import android.content.Intent
import android.media.MediaMetadataRetriever
import android.media.MediaPlayer
import android.media.MediaRecorder
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.speech.RecognitionListener
import android.speech.RecognizerIntent
import android.speech.SpeechRecognizer
import androidx.compose.foundation.clickable
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.Send
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material.icons.filled.Pause
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.RadioButtonChecked
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.pika.app.rust.VoiceRecordingPhase
import com.pika.app.rust.VoiceRecordingState
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File
import kotlin.math.log10
import kotlin.math.max
import kotlin.random.Random

private const val RECORDER_BAR_WIDTH_DP = 3
private const val RECORDER_BAR_SPACING_DP = 2
private const val MAX_RECORDER_LEVEL_BARS = 42
private const val VOICE_ATTACHMENT_BAR_COUNT = 20
private const val SPEECH_RESTART_DELAY_MS = 250L

internal class AndroidVoiceRecorder(
    private val context: Context,
) {
    private enum class State {
        Idle,
        Recording,
        Paused,
    }

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private val mainHandler = Handler(Looper.getMainLooper())
    private var state = State.Idle
    private var recorder: MediaRecorder? = null
    private var outputFile: File? = null
    private var levelJob: Job? = null
    private var speechRestartJob: Job? = null
    private var speechRecognizer: SpeechRecognizer? = null
    private var isSpeechListening = false
    private var transcriptCallback: ((String) -> Unit)? = null
    private var committedTranscript = ""
    private var partialTranscript = ""
    private var lastDispatchedTranscript = ""

    fun start(
        onLevel: (Float) -> Unit,
        onTranscript: (String) -> Unit,
    ): Boolean {
        if (state != State.Idle) return false

        val file =
            runCatching {
                File.createTempFile(
                    "voice_${System.currentTimeMillis()}_",
                    ".m4a",
                    context.cacheDir,
                )
            }.getOrNull() ?: return false

        val localRecorder = MediaRecorder()
        val started =
            runCatching {
                localRecorder.setAudioSource(MediaRecorder.AudioSource.MIC)
                localRecorder.setOutputFormat(MediaRecorder.OutputFormat.MPEG_4)
                localRecorder.setAudioEncoder(MediaRecorder.AudioEncoder.AAC)
                localRecorder.setAudioChannels(1)
                localRecorder.setAudioSamplingRate(44_100)
                localRecorder.setAudioEncodingBitRate(96_000)
                localRecorder.setOutputFile(file.absolutePath)
                localRecorder.prepare()
                localRecorder.start()
            }.isSuccess

        if (!started) {
            releaseRecorder(localRecorder)
            file.delete()
            return false
        }

        recorder = localRecorder
        outputFile = file
        state = State.Recording
        startLevelPolling(onLevel)
        beginTranscription(onTranscript)
        return true
    }

    fun pause(): Boolean {
        if (state != State.Recording) return false
        val ok = runCatching { recorder?.pause() }.isSuccess
        if (!ok) return false
        state = State.Paused
        stopLevelPolling()
        stopTranscription(destroyRecognizer = false, clearTranscript = false)
        return true
    }

    fun resume(
        onLevel: (Float) -> Unit,
        onTranscript: (String) -> Unit,
    ): Boolean {
        if (state != State.Paused) return false
        val ok = runCatching { recorder?.resume() }.isSuccess
        if (!ok) return false
        state = State.Recording
        startLevelPolling(onLevel)
        resumeTranscription(onTranscript)
        return true
    }

    fun stop(): File? {
        if (state == State.Idle) return null
        stopLevelPolling()
        stopTranscription(destroyRecognizer = true, clearTranscript = true)
        val file = outputFile
        val localRecorder = recorder
        recorder = null
        outputFile = null
        state = State.Idle

        var stopped = false
        if (localRecorder != null) {
            stopped = runCatching { localRecorder.stop() }.isSuccess
            releaseRecorder(localRecorder)
        }

        if (!stopped || file == null || !file.exists() || file.length() <= 0L) {
            file?.delete()
            return null
        }
        return file
    }

    fun cancel() {
        stopLevelPolling()
        stopTranscription(destroyRecognizer = true, clearTranscript = true)
        val localRecorder = recorder
        val file = outputFile
        recorder = null
        outputFile = null
        state = State.Idle

        if (localRecorder != null) {
            runCatching { localRecorder.stop() }
            releaseRecorder(localRecorder)
        }
        file?.delete()
    }

    fun release() {
        cancel()
        scope.cancel()
    }

    private fun startLevelPolling(onLevel: (Float) -> Unit) {
        stopLevelPolling()
        levelJob =
            scope.launch {
                while (isActive && state == State.Recording) {
                    val amplitude = runCatching { recorder?.maxAmplitude ?: 0 }.getOrDefault(0)
                    val normalized = normalizeAmplitude(amplitude)
                    withContext(Dispatchers.Main.immediate) {
                        onLevel(normalized)
                    }
                    delay(100)
                }
            }
    }

    private fun stopLevelPolling() {
        levelJob?.cancel()
        levelJob = null
    }

    private fun beginTranscription(onTranscript: (String) -> Unit) {
        transcriptCallback = onTranscript
        resetTranscriptState()
        startTranscriptionIfAvailable()
    }

    private fun resumeTranscription(onTranscript: (String) -> Unit) {
        transcriptCallback = onTranscript
        startTranscriptionIfAvailable()
    }

    private fun startTranscriptionIfAvailable() {
        if (state != State.Recording) return
        if (!SpeechRecognizer.isRecognitionAvailable(context)) return
        runOnMain {
            if (state != State.Recording) return@runOnMain
            val recognizer = ensureSpeechRecognizer() ?: return@runOnMain
            if (isSpeechListening) {
                runCatching { recognizer.cancel() }
                isSpeechListening = false
            }
            val startSucceeded = runCatching { recognizer.startListening(buildSpeechIntent()) }.isSuccess
            if (startSucceeded) {
                isSpeechListening = true
            } else {
                isSpeechListening = false
                scheduleSpeechRestart()
            }
        }
    }

    private fun buildSpeechIntent(): Intent =
        Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH).apply {
            putExtra(RecognizerIntent.EXTRA_LANGUAGE_MODEL, RecognizerIntent.LANGUAGE_MODEL_FREE_FORM)
            putExtra(RecognizerIntent.EXTRA_PARTIAL_RESULTS, true)
            putExtra(RecognizerIntent.EXTRA_MAX_RESULTS, 1)
            putExtra(RecognizerIntent.EXTRA_CALLING_PACKAGE, context.packageName)
            putExtra(RecognizerIntent.EXTRA_PREFER_OFFLINE, true)
        }

    private fun ensureSpeechRecognizer(): SpeechRecognizer? {
        speechRecognizer?.let { return it }
        val recognizer = runCatching { SpeechRecognizer.createSpeechRecognizer(context) }.getOrNull() ?: return null
        recognizer.setRecognitionListener(
            object : RecognitionListener {
                override fun onReadyForSpeech(params: Bundle?) = Unit

                override fun onBeginningOfSpeech() = Unit

                override fun onRmsChanged(rmsdB: Float) = Unit

                override fun onBufferReceived(buffer: ByteArray?) = Unit

                override fun onEndOfSpeech() {
                    isSpeechListening = false
                }

                override fun onError(error: Int) {
                    isSpeechListening = false
                    if (state != State.Recording) return
                    if (error == SpeechRecognizer.ERROR_INSUFFICIENT_PERMISSIONS) return
                    if (error == SpeechRecognizer.ERROR_CLIENT) return
                    scheduleSpeechRestart()
                }

                override fun onResults(results: Bundle?) {
                    isSpeechListening = false
                    onSpeechResult(extractTopRecognition(results), isFinal = true)
                    if (state == State.Recording) {
                        scheduleSpeechRestart()
                    }
                }

                override fun onPartialResults(partialResults: Bundle?) {
                    onSpeechResult(extractTopRecognition(partialResults), isFinal = false)
                }

                override fun onEvent(
                    eventType: Int,
                    params: Bundle?,
                ) = Unit
            },
        )
        speechRecognizer = recognizer
        return recognizer
    }

    private fun extractTopRecognition(results: Bundle?): String {
        val transcripts =
            results?.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION)
                ?: return ""
        return transcripts.firstOrNull()?.trim().orEmpty()
    }

    private fun onSpeechResult(
        transcript: String,
        isFinal: Boolean,
    ) {
        if (transcript.isBlank()) return
        if (isFinal) {
            committedTranscript = mergeTranscript(committedTranscript, transcript)
            partialTranscript = ""
        } else {
            partialTranscript = transcript
        }
        dispatchTranscriptIfChanged()
    }

    private fun dispatchTranscriptIfChanged() {
        val merged = mergeTranscript(committedTranscript, partialTranscript)
        if (merged == lastDispatchedTranscript) return
        lastDispatchedTranscript = merged
        transcriptCallback?.invoke(merged)
    }

    private fun mergeTranscript(
        committed: String,
        candidate: String,
    ): String {
        val left = committed.trim()
        val right = candidate.trim()
        if (left.isEmpty()) return right
        if (right.isEmpty()) return left
        if (left.equals(right, ignoreCase = true)) return left
        if (left.endsWith(right, ignoreCase = true)) return left
        if (right.startsWith(left, ignoreCase = true)) return right
        if (left.contains(right, ignoreCase = true)) return left
        if (right.contains(left, ignoreCase = true)) return right
        return "$left $right"
    }

    private fun scheduleSpeechRestart() {
        speechRestartJob?.cancel()
        speechRestartJob =
            scope.launch {
                delay(SPEECH_RESTART_DELAY_MS)
                if (state == State.Recording) {
                    startTranscriptionIfAvailable()
                }
            }
    }

    private fun stopTranscription(
        destroyRecognizer: Boolean,
        clearTranscript: Boolean,
    ) {
        speechRestartJob?.cancel()
        speechRestartJob = null
        val recognizer = speechRecognizer
        if (recognizer != null) {
            runOnMain {
                runCatching { recognizer.cancel() }
                isSpeechListening = false
                if (destroyRecognizer) {
                    runCatching { recognizer.destroy() }
                    if (speechRecognizer === recognizer) {
                        speechRecognizer = null
                    }
                }
            }
        } else {
            isSpeechListening = false
        }
        if (destroyRecognizer) {
            transcriptCallback = null
        }
        if (clearTranscript) {
            resetTranscriptState()
        }
    }

    private fun resetTranscriptState() {
        committedTranscript = ""
        partialTranscript = ""
        lastDispatchedTranscript = ""
    }

    private fun runOnMain(block: () -> Unit) {
        if (Looper.myLooper() == Looper.getMainLooper()) {
            block()
        } else {
            mainHandler.post(block)
        }
    }

    private fun releaseRecorder(localRecorder: MediaRecorder) {
        runCatching { localRecorder.reset() }
        runCatching { localRecorder.release() }
    }
}

internal class VoiceAttachmentPlayer {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private var player: MediaPlayer? = null
    private var tickerJob: Job? = null
    private var currentPath: String? = null

    var isPlaying by mutableStateOf(false)
        private set
    var progress by mutableFloatStateOf(0f)
        private set
    var currentSeconds by mutableFloatStateOf(0f)
        private set
    var durationSeconds by mutableFloatStateOf(0f)
        private set

    fun ensureMetadata(path: String) {
        if (currentPath == path && durationSeconds > 0f) return
        durationSeconds = readDurationSeconds(path)
        if (currentPath == null) {
            currentPath = path
        }
    }

    fun toggle(path: String) {
        if (isPlaying && currentPath == path) {
            pause()
            return
        }
        play(path)
    }

    fun release() {
        stopTicker()
        releasePlayer()
        scope.cancel()
    }

    private fun play(path: String) {
        if (currentPath != path) {
            stopTicker()
            releasePlayer()
            progress = 0f
            currentSeconds = 0f
            durationSeconds = readDurationSeconds(path)
        }

        val existing = player
        if (existing != null && currentPath == path) {
            runCatching { existing.start() }
            isPlaying = true
            startTicker()
            return
        }

        val created =
            runCatching {
                MediaPlayer().apply {
                    setDataSource(path)
                    prepare()
                    setOnCompletionListener {
                        this@VoiceAttachmentPlayer.isPlaying = false
                        this@VoiceAttachmentPlayer.currentSeconds = 0f
                        this@VoiceAttachmentPlayer.progress = 0f
                        this@VoiceAttachmentPlayer.stopTicker()
                        runCatching { seekTo(0) }
                    }
                }
            }.getOrNull() ?: return

        player = created
        currentPath = path
        durationSeconds = max(durationSeconds, created.duration / 1_000f)
        runCatching { created.start() }
        isPlaying = true
        startTicker()
    }

    private fun pause() {
        val local = player ?: return
        runCatching { local.pause() }
        isPlaying = false
        stopTicker()
    }

    private fun startTicker() {
        stopTicker()
        tickerJob =
            scope.launch {
                while (isActive && isPlaying) {
                    val local = player
                    if (local == null) {
                        isPlaying = false
                        break
                    }
                    currentSeconds = local.currentPosition / 1_000f
                    val duration = if (durationSeconds <= 0f) 1f else durationSeconds
                    progress = (currentSeconds / duration).coerceIn(0f, 1f)
                    delay(66)
                }
            }
    }

    private fun stopTicker() {
        tickerJob?.cancel()
        tickerJob = null
    }

    private fun releasePlayer() {
        val local = player ?: return
        runCatching { local.stop() }
        runCatching { local.release() }
        player = null
    }

    private fun readDurationSeconds(path: String): Float {
        val retriever = MediaMetadataRetriever()
        return try {
            retriever.setDataSource(path)
            val ms =
                retriever
                    .extractMetadata(MediaMetadataRetriever.METADATA_KEY_DURATION)
                    ?.toLongOrNull()
                    ?: 0L
            ms / 1_000f
        } catch (_: Throwable) {
            0f
        } finally {
            runCatching { retriever.release() }
        }
    }
}

@Composable
internal fun VoiceRecordingComposer(
    recording: VoiceRecordingState,
    onSend: () -> Unit,
    onCancel: () -> Unit,
    onTogglePause: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val isPaused = recording.phase == VoiceRecordingPhase.PAUSED
    Column(
        modifier =
            modifier
                .fillMaxWidth()
                .padding(horizontal = 12.dp, vertical = 10.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        if (recording.transcript.isNotBlank()) {
            Box(
                modifier =
                    Modifier
                        .fillMaxWidth()
                        .heightIn(max = 60.dp)
                        .verticalScroll(rememberScrollState()),
            ) {
                Text(
                    text = recording.transcript,
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }

        Row(
            modifier = Modifier.fillMaxWidth(),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Row(
                modifier = Modifier.width(64.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                Box(
                    modifier =
                        Modifier
                            .size(8.dp)
                            .clip(RoundedCornerShape(4.dp))
                            .background(MaterialTheme.colorScheme.error.copy(alpha = if (isPaused) 0.45f else 1f)),
                )
                Text(
                    text = formatVoiceDuration(recording.durationSecs.toFloat()),
                    style =
                        MaterialTheme.typography.bodyMedium.copy(
                            fontFamily = FontFamily.Monospace,
                        ),
                )
            }
            VoiceWaveformBars(
                levels = recording.levels,
                modifier = Modifier.weight(1f),
                playedProgress = 1f,
                playedColor = MaterialTheme.colorScheme.primary.copy(alpha = 0.9f),
                unplayedColor = MaterialTheme.colorScheme.primary.copy(alpha = 0.42f),
            )
        }

        Row(
            modifier = Modifier.fillMaxWidth(),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.SpaceBetween,
        ) {
            RecordingControlIcon(
                icon = Icons.Default.Delete,
                contentDescription = "Delete recording",
                tint = MaterialTheme.colorScheme.onSurfaceVariant,
                onClick = onCancel,
            )
            RecordingControlIcon(
                icon = if (isPaused) Icons.Default.RadioButtonChecked else Icons.Default.Pause,
                contentDescription = if (isPaused) "Resume recording" else "Pause recording",
                tint = MaterialTheme.colorScheme.onSurface,
                onClick = onTogglePause,
            )
            RecordingControlIcon(
                icon = Icons.AutoMirrored.Filled.Send,
                contentDescription = "Send recording",
                tint = MaterialTheme.colorScheme.primary,
                onClick = onSend,
            )
        }
    }
}

@Composable
private fun RecordingControlIcon(
    icon: ImageVector,
    contentDescription: String,
    tint: Color,
    onClick: () -> Unit,
) {
    Box(
        modifier =
            Modifier
                .size(36.dp)
                .clip(CircleShape)
                .clickable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Icon(
            imageVector = icon,
            contentDescription = contentDescription,
            tint = tint,
            modifier = Modifier.size(24.dp),
        )
    }
}

@Composable
internal fun VoiceAttachmentPlayerRow(
    localPath: String,
    isMine: Boolean,
    modifier: Modifier = Modifier,
) {
    val player = remember(localPath) { VoiceAttachmentPlayer() }
    val waveform = remember(localPath) { generateWaveform(localPath) }

    DisposableEffect(localPath) {
        player.ensureMetadata(localPath)
        onDispose {
            player.release()
        }
    }

    Row(
        modifier = modifier.widthIn(max = 220.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        IconButton(
            onClick = { player.toggle(localPath) },
            modifier = Modifier.size(30.dp),
        ) {
            Icon(
                imageVector = if (player.isPlaying) Icons.Default.Pause else Icons.Default.PlayArrow,
                contentDescription = if (player.isPlaying) "Pause voice message" else "Play voice message",
                tint =
                    if (isMine) {
                        MaterialTheme.colorScheme.onPrimary
                    } else {
                        MaterialTheme.colorScheme.onSurface
                    },
            )
        }

        VoiceWaveformBars(
            levels = waveform,
            modifier = Modifier.weight(1f),
            playedProgress = player.progress,
            playedColor =
                if (isMine) {
                    MaterialTheme.colorScheme.onPrimary
                } else {
                    MaterialTheme.colorScheme.primary
                },
            unplayedColor =
                if (isMine) {
                    MaterialTheme.colorScheme.onPrimary.copy(alpha = 0.4f)
                } else {
                    MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = 0.58f)
                },
        )

        Text(
            text = formatVoiceDuration(if (player.isPlaying) player.currentSeconds else player.durationSeconds),
            style =
                MaterialTheme.typography.labelSmall.copy(
                    fontFamily = FontFamily.Monospace,
                ),
            color =
                if (isMine) {
                    MaterialTheme.colorScheme.onPrimary.copy(alpha = 0.86f)
                } else {
                    MaterialTheme.colorScheme.onSurfaceVariant
                },
            maxLines = 1,
            overflow = TextOverflow.Clip,
        )
    }
}

@Composable
private fun VoiceWaveformBars(
    levels: List<Float>,
    modifier: Modifier,
    playedProgress: Float,
    playedColor: Color,
    unplayedColor: Color,
) {
    val normalized = levels.map { it.coerceIn(0f, 1f) }
    val visible =
        if (normalized.size > MAX_RECORDER_LEVEL_BARS) {
            normalized.takeLast(MAX_RECORDER_LEVEL_BARS)
        } else {
            normalized
        }
    val safePlayedProgress = playedProgress.coerceIn(0f, 1f)

    Box(modifier = modifier.height(28.dp), contentAlignment = Alignment.CenterEnd) {
        Row(
            horizontalArrangement = Arrangement.spacedBy(RECORDER_BAR_SPACING_DP.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            visible.forEachIndexed { index, level ->
                val relative = if (visible.isEmpty()) 0f else index.toFloat() / visible.size.toFloat()
                val barColor = if (relative <= safePlayedProgress) playedColor else unplayedColor
                Box(
                    modifier =
                        Modifier
                            .width(RECORDER_BAR_WIDTH_DP.dp)
                            .height((4f + (level * 20f)).dp)
                            .clip(RoundedCornerShape(1.5.dp))
                            .background(barColor),
                )
            }
        }
    }
}

private fun normalizeAmplitude(amplitude: Int): Float {
    if (amplitude <= 0) return 0f
    val ratio = max(amplitude.toFloat() / 32_767f, 1e-6f)
    val db = 20f * log10(ratio)
    return ((db + 50f) / 50f).coerceIn(0f, 1f)
}

private fun formatVoiceDuration(seconds: Float): String {
    val totalSeconds = max(seconds.toInt(), 0)
    val minutes = totalSeconds / 60
    val secs = totalSeconds % 60
    return "%d:%02d".format(minutes, secs)
}

private fun generateWaveform(path: String): List<Float> {
    val file = File(path)
    val seed = path.hashCode().toLong() xor file.length()
    val rnd = Random(seed)
    return List(VOICE_ATTACHMENT_BAR_COUNT) {
        0.16f + (rnd.nextFloat() * 0.84f)
    }
}
