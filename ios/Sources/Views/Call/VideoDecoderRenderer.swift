import CoreMedia
import CoreVideo
import Foundation
import os
import VideoToolbox

enum VideoDecoderResetReason {
    case formatChange
    case decodeFailure(OSStatus)
}

struct VideoDecoderCounters {
    var formatResets: UInt64 = 0
    var decodeFailureResets: UInt64 = 0
    var decodeFailures: UInt64 = 0
    var droppedUntilKeyframe: UInt64 = 0
}

/// Receives encrypted H.264 NALUs from Rust core, decodes them, and publishes
/// decoded pixel buffers for display.
final class VideoDecoderRenderer: VideoFrameReceiver {
    private static let log = Logger(subsystem: "chat.pika", category: "VideoDecoderRenderer")
    private var decompressionSession: VTDecompressionSession?
    private var formatDescription: CMVideoFormatDescription?
    private var lastSps: Data?
    private var lastPps: Data?
    private var waitingForKeyframeAfterFailure = false
    private var consecutiveDecodeFailures: UInt64 = 0
    private var counters = VideoDecoderCounters()

    /// Called on the main thread when a new decoded frame is available.
    var onDecodedFrame: ((CVPixelBuffer) -> Void)?
    var onDecoderReset: ((VideoDecoderResetReason, VideoDecoderCounters) -> Void)?

    func onVideoFrame(callId: String, payload: Data) {
        processAnnexBPayload(payload)
    }

    func currentCounters() -> VideoDecoderCounters {
        counters
    }

    // MARK: - Annex B Parsing

    private func processAnnexBPayload(_ data: Data) {
        let nalUnits = parseAnnexBNalUnits(data)
        for nalu in nalUnits {
            guard !nalu.isEmpty else { continue }
            let naluType = nalu[0] & 0x1F

            switch naluType {
            case 7: // SPS
                lastSps = nalu
                tryCreateFormatDescription()
            case 8: // PPS
                lastPps = nalu
                tryCreateFormatDescription()
            case 1, 5: // Non-IDR slice, IDR slice
                if waitingForKeyframeAfterFailure, naluType != 5 {
                    bump(&counters.droppedUntilKeyframe)
                    continue
                }
                decodeNalu(nalu, isKeyframe: naluType == 5)
            default:
                break
            }
        }
    }

    private func parseAnnexBNalUnits(_ data: Data) -> [Data] {
        var units: [Data] = []
        var i = 0
        let bytes = Array(data)
        let count = bytes.count

        while i < count {
            // Find start code: 0x00 0x00 0x00 0x01 or 0x00 0x00 0x01
            var startCodeLen = 0
            if i + 3 < count, bytes[i] == 0, bytes[i + 1] == 0, bytes[i + 2] == 0, bytes[i + 3] == 1 {
                startCodeLen = 4
            } else if i + 2 < count, bytes[i] == 0, bytes[i + 1] == 0, bytes[i + 2] == 1 {
                startCodeLen = 3
            }

            if startCodeLen > 0 {
                let naluStart = i + startCodeLen
                // Find next start code or end of data
                var naluEnd = count
                for j in naluStart..<count {
                    if j + 3 < count, bytes[j] == 0, bytes[j + 1] == 0, bytes[j + 2] == 0, bytes[j + 3] == 1 {
                        naluEnd = j
                        break
                    } else if j + 2 < count, bytes[j] == 0, bytes[j + 1] == 0, bytes[j + 2] == 1 {
                        naluEnd = j
                        break
                    }
                }
                if naluEnd > naluStart {
                    units.append(Data(bytes[naluStart..<naluEnd]))
                }
                i = naluEnd
            } else {
                i += 1
            }
        }
        return units
    }

    // MARK: - Format Description & Session

    private func tryCreateFormatDescription() {
        guard let sps = lastSps, let pps = lastPps else { return }

        let spsArr = Array(sps)
        let ppsArr = Array(pps)
        var newFormat: CMFormatDescription?

        spsArr.withUnsafeBufferPointer { spsBuffer in
            ppsArr.withUnsafeBufferPointer { ppsBuffer in
                guard let spsBase = spsBuffer.baseAddress,
                      let ppsBase = ppsBuffer.baseAddress else { return }
                let parameterSets: [UnsafePointer<UInt8>] = [spsBase, ppsBase]
                let parameterSizes: [Int] = [sps.count, pps.count]
                parameterSets.withUnsafeBufferPointer { setsPtr in
                    parameterSizes.withUnsafeBufferPointer { sizesPtr in
                        CMVideoFormatDescriptionCreateFromH264ParameterSets(
                            allocator: kCFAllocatorDefault,
                            parameterSetCount: 2,
                            parameterSetPointers: setsPtr.baseAddress!,
                            parameterSetSizes: sizesPtr.baseAddress!,
                            nalUnitHeaderLength: 4,
                            formatDescriptionOut: &newFormat
                        )
                    }
                }
            }
        }

        guard let newFormat else { return }

        // Only recreate session if format changed
        if let existing = formatDescription,
           CMFormatDescriptionEqual(existing, otherFormatDescription: newFormat) {
            return
        }
        formatDescription = newFormat
        if decompressionSession != nil {
            recordReset(.formatChange)
        }
        _ = createDecompressionSession()
    }

    private func createDecompressionSession() -> Bool {
        if let session = decompressionSession {
            VTDecompressionSessionInvalidate(session)
            decompressionSession = nil
        }

        guard let formatDescription else { return false }

        let attrs: [NSString: Any] = [
            kCVPixelBufferPixelFormatTypeKey: kCVPixelFormatType_32BGRA
        ]

        var session: VTDecompressionSession?
        let status = VTDecompressionSessionCreate(
            allocator: kCFAllocatorDefault,
            formatDescription: formatDescription,
            decoderSpecification: nil,
            imageBufferAttributes: attrs as CFDictionary,
            outputCallback: nil,
            decompressionSessionOut: &session
        )
        guard status == noErr else {
            Self.log.error("VTDecompressionSessionCreate failed: \(status)")
            return false
        }
        decompressionSession = session
        return true
    }

    // MARK: - Decode

    private func decodeNalu(_ nalu: Data, isKeyframe: Bool) {
        guard let formatDescription else { return }
        if decompressionSession == nil {
            _ = createDecompressionSession()
        }
        guard let session = decompressionSession else { return }

        // Wrap NALU with AVCC length prefix (4 bytes, big-endian)
        var avccData = Data()
        var naluLength = UInt32(nalu.count).bigEndian
        withUnsafeBytes(of: &naluLength) { avccData.append(contentsOf: $0) }
        avccData.append(nalu)

        let avccLength = avccData.count
        var blockBuffer: CMBlockBuffer?
        avccData.withUnsafeMutableBytes { rawPtr in
            guard let baseAddress = rawPtr.baseAddress else { return }
            CMBlockBufferCreateWithMemoryBlock(
                allocator: kCFAllocatorDefault,
                memoryBlock: nil,
                blockLength: avccLength,
                blockAllocator: kCFAllocatorDefault,
                customBlockSource: nil,
                offsetToData: 0,
                dataLength: avccLength,
                flags: 0,
                blockBufferOut: &blockBuffer
            )
            if let blockBuffer {
                CMBlockBufferReplaceDataBytes(
                    with: baseAddress,
                    blockBuffer: blockBuffer,
                    offsetIntoDestination: 0,
                    dataLength: avccLength
                )
            }
        }

        guard let blockBuffer else { return }

        var sampleBuffer: CMSampleBuffer?
        var sampleSize = avccData.count
        CMSampleBufferCreateReady(
            allocator: kCFAllocatorDefault,
            dataBuffer: blockBuffer,
            formatDescription: formatDescription,
            sampleCount: 1,
            sampleTimingEntryCount: 0,
            sampleTimingArray: nil,
            sampleSizeEntryCount: 1,
            sampleSizeArray: &sampleSize,
            sampleBufferOut: &sampleBuffer
        )

        guard let sampleBuffer else { return }

        var flags: VTDecodeInfoFlags = []
        VTDecompressionSessionDecodeFrame(
            session,
            sampleBuffer: sampleBuffer,
            flags: [._EnableAsynchronousDecompression],
            infoFlagsOut: &flags
        ) { [weak self] status, _, imageBuffer, _, _ in
            guard let self else { return }
            guard status == noErr, let imageBuffer else {
                self.handleDecodeFailure(status)
                return
            }
            self.consecutiveDecodeFailures = 0
            if isKeyframe {
                self.waitingForKeyframeAfterFailure = false
            }
            DispatchQueue.main.async {
                self.onDecodedFrame?(imageBuffer)
            }
        }
    }

    private func handleDecodeFailure(_ status: OSStatus) {
        bump(&counters.decodeFailures)
        bump(&consecutiveDecodeFailures)
        waitingForKeyframeAfterFailure = true
        recordReset(.decodeFailure(status))
        _ = createDecompressionSession()
    }

    private func recordReset(_ reason: VideoDecoderResetReason) {
        switch reason {
        case .formatChange:
            bump(&counters.formatResets)
        case .decodeFailure:
            bump(&counters.decodeFailureResets)
        }
        onDecoderReset?(reason, counters)
    }

    private func bump(_ value: inout UInt64) {
        if value < UInt64.max {
            value += 1
        }
    }
}

#if DEBUG
extension VideoDecoderRenderer {
    func debugInjectDecodeFailure(_ status: OSStatus = -1) {
        handleDecodeFailure(status)
    }

    func debugCanAttemptDecode(naluType: UInt8) -> Bool {
        !(waitingForKeyframeAfterFailure && naluType != 5)
    }
}
#endif
