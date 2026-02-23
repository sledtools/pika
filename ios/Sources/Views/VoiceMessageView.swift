import SwiftUI
import AVFoundation
import Accelerate

struct VoiceMessageView: View {
    let attachment: ChatMediaAttachment
    let isMine: Bool
    var onDownload: (() -> Void)? = nil

    @State private var player = VoiceMessagePlayer()

    var body: some View {
        if let localPath = attachment.localPath {
            playerContent(localPath: localPath)
        } else {
            downloadRow
        }
    }

    @ViewBuilder
    private func playerContent(localPath: String) -> some View {
        HStack(spacing: 8) {
            Button {
                player.toggle(url: URL(fileURLWithPath: localPath))
            } label: {
                Image(systemName: player.isPlaying ? "pause.fill" : "play.fill")
                    .font(.title3)
                    .foregroundStyle(isMine ? .white : .primary)
                    .frame(width: 28, height: 28)
            }

            StaticWaveformView(
                samples: player.waveformSamples,
                progress: player.progress,
                isMine: isMine,
                maxBarHeight: 24
            )

            Text(player.isPlaying ? formatTime(player.currentTime) : formatTime(player.duration))
                .font(.caption.monospacedDigit())
                .foregroundStyle(isMine ? .white.opacity(0.78) : .secondary)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 10)
        .frame(width: 220)
        .onAppear {
            player.loadWaveform(from: localPath)
        }
    }

    private var downloadRow: some View {
        HStack(spacing: 10) {
            Image(systemName: "waveform")
                .font(.title3)
                .foregroundStyle(isMine ? .white.opacity(0.8) : .secondary)

            VStack(alignment: .leading, spacing: 2) {
                Text("Voice Message")
                    .font(.subheadline)
                    .foregroundStyle(isMine ? .white : .primary)
                Text(attachment.mimeType)
                    .font(.caption2)
                    .foregroundStyle(isMine ? .white.opacity(0.6) : .secondary)
            }

            Spacer(minLength: 0)

            Button {
                onDownload?()
            } label: {
                Image(systemName: "arrow.down.circle")
                    .font(.title3)
                    .foregroundStyle(isMine ? .white : .blue)
            }
            .buttonStyle(.plain)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .frame(maxWidth: .infinity)
    }

    private func formatTime(_ time: TimeInterval) -> String {
        let minutes = Int(time) / 60
        let seconds = Int(time) % 60
        return String(format: "%d:%02d", minutes, seconds)
    }
}

// MARK: - Playback helper

@Observable
@MainActor
final class VoiceMessagePlayer: NSObject {
    private(set) var isPlaying = false
    private(set) var currentTime: TimeInterval = 0
    private(set) var duration: TimeInterval = 0
    private(set) var progress: CGFloat = 0
    private(set) var waveformSamples: [CGFloat] = []

    private var audioPlayer: AVAudioPlayer?
    private var timer: Timer?
    private var currentURL: URL?

    func toggle(url: URL) {
        if isPlaying, currentURL == url {
            pause()
        } else {
            play(url: url)
        }
    }

    func loadWaveform(from path: String) {
        guard waveformSamples.isEmpty else { return }
        let url = URL(fileURLWithPath: path)
        Task.detached { [weak self] in
            let samples = VoiceMessagePlayer.extractWaveform(from: url, sampleCount: 20)
            await MainActor.run {
                self?.waveformSamples = samples
            }
        }
        // Also load duration
        do {
            let player = try AVAudioPlayer(contentsOf: url)
            duration = player.duration
        } catch {
            // Duration will stay 0
        }
    }

    // MARK: - Private

    private func play(url: URL) {
        do {
            let session = AVAudioSession.sharedInstance()
            try session.setCategory(.playAndRecord, mode: .default, options: [.defaultToSpeaker])
            try session.setActive(true)

            let player = try AVAudioPlayer(contentsOf: url)
            player.delegate = self
            player.play()

            audioPlayer = player
            currentURL = url
            duration = player.duration
            isPlaying = true

            startTimer()
        } catch {
            print("VoiceMessagePlayer: playback error: \(error)")
        }
    }

    private func pause() {
        audioPlayer?.pause()
        isPlaying = false
        stopTimer()
    }

    private func startTimer() {
        timer = Timer.scheduledTimer(withTimeInterval: 1.0 / 15.0, repeats: true) { [weak self] _ in
            Task { @MainActor [weak self] in
                self?.updateProgress()
            }
        }
    }

    private func stopTimer() {
        timer?.invalidate()
        timer = nil
    }

    private func updateProgress() {
        guard let player = audioPlayer else { return }
        currentTime = player.currentTime
        progress = duration > 0 ? CGFloat(currentTime / duration) : 0
    }

    private func playbackFinished() {
        isPlaying = false
        currentTime = 0
        progress = 0
        stopTimer()
    }

    // Extract waveform samples from audio file
    private nonisolated static func extractWaveform(from url: URL, sampleCount: Int) -> [CGFloat] {
        guard let audioFile = try? AVAudioFile(forReading: url) else { return [] }
        let format = audioFile.processingFormat
        let frameCount = AVAudioFrameCount(audioFile.length)
        guard frameCount > 0,
              let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: frameCount)
        else { return [] }

        do {
            try audioFile.read(into: buffer)
        } catch {
            return []
        }

        guard let channelData = buffer.floatChannelData?[0] else { return [] }
        let totalFrames = Int(buffer.frameLength)
        let samplesPerBin = max(1, totalFrames / sampleCount)
        var samples: [CGFloat] = []

        for i in 0..<sampleCount {
            let start = i * samplesPerBin
            let end = min(start + samplesPerBin, totalFrames)
            guard start < totalFrames else { break }

            var rms: Float = 0
            vDSP_measqv(channelData.advanced(by: start), 1, &rms, vDSP_Length(end - start))
            rms = sqrtf(rms)
            let db = 20 * log10f(max(rms, 1e-6))
            let normalized = CGFloat(max(0, min(1, (db + 50) / 50)))
            samples.append(normalized)
        }
        return samples
    }
}

extension VoiceMessagePlayer: @preconcurrency AVAudioPlayerDelegate {
    nonisolated func audioPlayerDidFinishPlaying(_ player: AVAudioPlayer, successfully flag: Bool) {
        Task { @MainActor [weak self] in
            self?.playbackFinished()
        }
    }
}

// MARK: - Static waveform

private struct StaticWaveformView: View {
    let samples: [CGFloat]
    let progress: CGFloat
    let isMine: Bool
    var maxBarHeight: CGFloat = 28

    private let barWidth: CGFloat = 3
    private let barSpacing: CGFloat = 2

    var body: some View {
        HStack(alignment: .center, spacing: barSpacing) {
            ForEach(Array(samples.enumerated()), id: \.offset) { index, level in
                let barProgress = samples.isEmpty ? 0 : CGFloat(index) / CGFloat(samples.count)
                let isPlayed = barProgress <= progress
                RoundedRectangle(cornerRadius: 1.5)
                    .fill(barColor(isPlayed: isPlayed))
                    .frame(width: barWidth, height: max(2, level * maxBarHeight))
            }
        }
        .frame(height: maxBarHeight)
    }

    private func barColor(isPlayed: Bool) -> Color {
        if isMine {
            return isPlayed ? .white : .white.opacity(0.4)
        }
        return isPlayed ? .accentColor : Color(uiColor: .tertiaryLabel)
    }
}
