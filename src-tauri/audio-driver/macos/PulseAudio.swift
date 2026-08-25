// SPDX-License-Identifier: MIT
//
// Pulse — an input-only virtual microphone for macOS.
//
// The Core Audio property and timing implementation is adapted from Hush by
// Tim Schmolka (MIT). Pulse publishes one 48 kHz mono Float32 input stream.
// Translated PCM arrives from the Pulse app over a loopback-only UDP socket;
// no render/output endpoint is exposed to macOS applications.

import CoreAudio
import CoreAudio.AudioServerPlugIn
import Darwin
import Foundation
import os

// MARK: - Configuration

private let kDeviceName = "Pulse"
private let kDeviceUID = "app.pulse.desktop.virtual-microphone"
private let kModelUID = "app.pulse.desktop.virtual-microphone.model"
private let kManufacturer = "Pulse"
private let kSampleRate: Float64 = 48_000
private let kChannels: UInt32 = 1
private let kBytesPerFrame: UInt32 = kChannels * UInt32(MemoryLayout<Float32>.size)
private let kRingBufferSize: UInt32 = 19_200   // zero-timestamp period, in frames

private enum Obj {
    static let plugIn:      AudioObjectID = AudioObjectID(kAudioObjectPlugInObject) // == 1
    static let device:      AudioObjectID = 2
    static let streamInput: AudioObjectID = 3
}

private let gLog = Logger(subsystem: "app.pulse.desktop", category: "virtual-microphone")

// These live in AudioServerPlugIn.h / CFPlugInCOM.h as C macros that Swift can't
// import, so we recreate them here.
private let kIOOpReadInput: UInt32 = 0x7265_6164 // 'read'

private func makeUUID(_ s: String) -> CFUUID { CFUUIDCreateFromString(kCFAllocatorDefault, s as CFString) }
private let gTypeUUID        = makeUUID("443ABAB8-E7B3-491A-B985-BEB9187030DB") // kAudioServerPlugInTypeUUID
private let gDriverIfaceUUID = makeUUID("EEA5773D-CC43-49F1-8E00-8F96E7D23B17") // kAudioServerPlugInDriverInterfaceUUID
private let gIUnknownUUID    = makeUUID("00000000-0000-0000-C000-000000000046") // IUnknownUUID

// MARK: - Global State

// A lock kept behind a heap pointer so the realtime path never takes `&global`
// (which would trip Swift's exclusivity checks under contention).
private let gLock: UnsafeMutablePointer<os_unfair_lock> = {
    let p = UnsafeMutablePointer<os_unfair_lock>.allocate(capacity: 1)
    p.initialize(to: os_unfair_lock())
    return p
}()
@inline(__always) private func lock()   { os_unfair_lock_lock(gLock) }
@inline(__always) private func unlock() { os_unfair_lock_unlock(gLock) }

private var gRefCount: UInt32 = 0
nonisolated(unsafe) private var gHost: AudioServerPlugInHostRef?

private var gIOCount: UInt64 = 0
private var gHostTicksPerFrame: Float64 = 0
private var gNumberTimeStamps: UInt64 = 0
private var gAnchorHostTime: UInt64 = 0

private let kAudioPort: UInt16 = 41_873
private let kPacketFrames = 480
private let kAudioBufferFrames = 96_000
private let gAudioLock: UnsafeMutablePointer<os_unfair_lock> = {
    let p = UnsafeMutablePointer<os_unfair_lock>.allocate(capacity: 1)
    p.initialize(to: os_unfair_lock())
    return p
}()
private let gAudioBuffer: UnsafeMutablePointer<Float32> = {
    let p = UnsafeMutablePointer<Float32>.allocate(capacity: kAudioBufferFrames)
    p.initialize(repeating: 0, count: kAudioBufferFrames)
    return p
}()
private var gAudioReadIndex = 0
private var gAudioWriteIndex = 0
private var gAudioAvailable = 0
private var gReceiverStarted = false

// MARK: - Format helpers

private func mockFormat() -> AudioStreamBasicDescription {
    AudioStreamBasicDescription(
        mSampleRate: kSampleRate,
        mFormatID: kAudioFormatLinearPCM,
        mFormatFlags: kAudioFormatFlagIsFloat | kAudioFormatFlagsNativeEndian | kAudioFormatFlagIsPacked,
        mBytesPerPacket: kBytesPerFrame,
        mFramesPerPacket: 1,
        mBytesPerFrame: kBytesPerFrame,
        mChannelsPerFrame: kChannels,
        mBitsPerChannel: 32,
        mReserved: 0)
}

// MARK: - PropertyValue

/// A property's value paired with how it encodes into a CoreAudio buffer.
///
/// This is the single source of truth for the property system: size, presence,
/// and data all derive from `value(for:)` below, so they can never disagree.
/// The raw-pointer `storeBytes` calls — the only genuinely "unsafe" part of the
/// property layer — are confined to `write(to:)`.
private enum PropertyValue {
    case uint32(UInt32)
    case objectID(AudioObjectID)
    case float64(Float64)
    case string(String)
    case pair(UInt32, UInt32)
    case asbd(AudioStreamBasicDescription)
    case rangedFormat(AudioStreamRangedDescription)
    case valueRange(AudioValueRange)
    case channelLayout(AudioChannelLayout)
    case empty

    var byteSize: UInt32 {
        switch self {
        case .uint32, .objectID: UInt32(MemoryLayout<UInt32>.size)
        case .float64:           UInt32(MemoryLayout<Float64>.size)
        case .string:            UInt32(MemoryLayout<UnsafeMutableRawPointer>.size) // CFStringRef
        case .pair:              UInt32(2 * MemoryLayout<UInt32>.size)
        case .asbd:              UInt32(MemoryLayout<AudioStreamBasicDescription>.size)
        case .rangedFormat:      UInt32(MemoryLayout<AudioStreamRangedDescription>.size)
        case .valueRange:        UInt32(MemoryLayout<AudioValueRange>.size)
        case .channelLayout:     UInt32(MemoryLayout<AudioChannelLayout>.size)
        case .empty:             0
        }
    }

    /// Encodes into `dst`. Callers only invoke this once they've confirmed the
    /// destination buffer is large enough, so the CFString retain can't leak.
    func write(to dst: UnsafeMutableRawPointer) {
        switch self {
        case let .uint32(v):        dst.storeBytes(of: v, as: UInt32.self)
        case let .objectID(v):      dst.storeBytes(of: v, as: AudioObjectID.self)
        case let .float64(v):       dst.storeBytes(of: v, as: Float64.self)
        case let .string(s):
            dst.storeBytes(of: Unmanaged.passRetained(s as CFString).toOpaque(), as: UnsafeMutableRawPointer.self)
        case let .pair(a, b):
            dst.storeBytes(of: a, toByteOffset: 0, as: UInt32.self)
            dst.storeBytes(of: b, toByteOffset: MemoryLayout<UInt32>.size, as: UInt32.self)
        case let .asbd(v):          dst.storeBytes(of: v, as: AudioStreamBasicDescription.self)
        case let .rangedFormat(v):  dst.storeBytes(of: v, as: AudioStreamRangedDescription.self)
        case let .valueRange(v):    dst.storeBytes(of: v, as: AudioValueRange.self)
        case let .channelLayout(v): dst.storeBytes(of: v, as: AudioChannelLayout.self)
        case .empty:                break
        }
    }
}

// MARK: - Interface vtable

private let gInterface: UnsafeMutablePointer<AudioServerPlugInDriverInterface> = {
    let p = UnsafeMutablePointer<AudioServerPlugInDriverInterface>.allocate(capacity: 1)
    p.initialize(to: makeInterface())
    return p
}()

private let gDriverRef: UnsafeMutablePointer<UnsafeMutablePointer<AudioServerPlugInDriverInterface>?> = {
    let pp = UnsafeMutablePointer<UnsafeMutablePointer<AudioServerPlugInDriverInterface>?>.allocate(capacity: 1)
    pp.initialize(to: gInterface)
    return pp
}()

private func makeInterface() -> AudioServerPlugInDriverInterface {
    AudioServerPlugInDriverInterface(
        _reserved: nil,
        QueryInterface: { (driver, uuid, outInterface) in PulseQueryInterface(driver, uuid, outInterface) },
        AddRef:  { _ in lock(); if gRefCount < UInt32.max { gRefCount += 1 }; let r = gRefCount; unlock(); return r },
        Release: { _ in lock(); if gRefCount > 0 { gRefCount -= 1 }; let r = gRefCount; unlock(); return r },
        Initialize: { (_, host) in
            gHost = host
            startAudioReceiver()
            gLog.info("Pulse initialized (device \"\(kDeviceName, privacy: .public)\", \(kSampleRate) Hz, \(kChannels) ch)")
            return noErr
        },
        CreateDevice: { _, _, _, _ in kAudioHardwareUnsupportedOperationError },
        DestroyDevice: { _, _ in kAudioHardwareUnsupportedOperationError },
        AddDeviceClient: { _, _, _ in noErr },
        RemoveDeviceClient: { _, _, _ in noErr },
        PerformDeviceConfigurationChange: { _, _, _, _ in noErr },
        AbortDeviceConfigurationChange: { _, _, _, _ in noErr },
        HasProperty: { (_, obj, _, addr) in DarwinBoolean(PulseHasProperty(obj, addr)) },
        IsPropertySettable: { (_, obj, _, addr, outSettable) in PulseIsSettable(obj, addr, outSettable) },
        GetPropertyDataSize: { (_, obj, _, addr, _, _, outSize) in PulseGetPropertyDataSize(obj, addr, outSize) },
        GetPropertyData: { (_, obj, _, addr, _, qd, inSize, outSize, outData) in
            PulseGetPropertyData(obj, addr, qd, inSize, outSize, outData)
        },
        SetPropertyData: { (_, obj, _, addr, _, _, inSize, inData) in
            PulseSetPropertyData(obj, addr, inSize, inData)
        },
        StartIO: { (_, device, _) in PulseStartIO(device) },
        StopIO:  { (_, device, _) in PulseStopIO(device) },
        GetZeroTimeStamp: { (_, device, _, outSampleTime, outHostTime, outSeed) in
            PulseGetZeroTimeStamp(device, outSampleTime, outHostTime, outSeed)
        },
        WillDoIOOperation: { (_, _, _, opID, outWillDo, outWillDoInPlace) in
            let willDo = (opID == kIOOpReadInput)
            outWillDo.pointee = DarwinBoolean(willDo)
            outWillDoInPlace.pointee = DarwinBoolean(true)
            return noErr
        },
        BeginIOOperation: { _, _, _, _, _, _ in noErr },
        DoIOOperation: { (_, _, _, _, opID, frameSize, _, ioMainBuffer, _) in
            if opID == kIOOpReadInput, let buf = ioMainBuffer {
                readAudio(into: buf.assumingMemoryBound(to: Float32.self), frameCount: Int(frameSize))
            }
            return noErr
        },
        EndIOOperation: { _, _, _, _, _, _ in noErr })
}

// MARK: - Factory (entry point named in Info.plist)

@_cdecl("PulseCreate")
public func PulseCreate(_ allocator: CFAllocator?, _ requestedTypeUUID: CFUUID?) -> UnsafeMutableRawPointer? {
    guard let requested = requestedTypeUUID,
          CFEqual(requested, gTypeUUID) else { return nil }
    return UnsafeMutableRawPointer(gDriverRef)
}

// MARK: - COM

private func PulseQueryInterface(_ driver: UnsafeMutableRawPointer?,
                                _ uuid: CFUUIDBytes,
                                _ outInterface: UnsafeMutablePointer<LPVOID?>?) -> HRESULT {
    guard driver == UnsafeMutableRawPointer(gDriverRef), let outInterface else {
        return HRESULT(kAudioHardwareBadObjectError)
    }
    let requested = CFUUIDCreateFromUUIDBytes(kCFAllocatorDefault, uuid)
    if CFEqual(requested, gDriverIfaceUUID) || CFEqual(requested, gIUnknownUUID) {
        lock(); if gRefCount < UInt32.max { gRefCount += 1 }; unlock()
        outInterface.pointee = UnsafeMutableRawPointer(gDriverRef)
        return HRESULT(0) // S_OK
    }
    return HRESULT(bitPattern: 0x8000_0004) // E_NOINTERFACE
}

// MARK: - Property access

private func PulseHasProperty(_ obj: AudioObjectID, _ addr: UnsafePointer<AudioObjectPropertyAddress>?) -> Bool {
    guard let a = addr?.pointee else { return false }
    return value(for: obj, a.mSelector, scope: a.mScope, qualifier: nil) != nil
}

private func PulseIsSettable(_ obj: AudioObjectID,
                            _ addr: UnsafePointer<AudioObjectPropertyAddress>?,
                            _ outSettable: UnsafeMutablePointer<DarwinBoolean>?) -> OSStatus {
    guard let sel = addr?.pointee.mSelector else { return kAudioHardwareBadObjectError }
    switch sel {
    case kAudioDevicePropertyNominalSampleRate,
         kAudioStreamPropertyVirtualFormat,
         kAudioStreamPropertyPhysicalFormat,
         kAudioStreamPropertyIsActive:
        outSettable?.pointee = true
    default:
        outSettable?.pointee = false
    }
    return noErr
}

/// The single source of truth for every property Pulse exposes. Returns `nil`
/// for anything unsupported (which maps to `kAudioHardwareUnknownPropertyError`).
private func value(for object: AudioObjectID,
                   _ selector: AudioObjectPropertySelector,
                   scope: AudioObjectPropertyScope,
                   qualifier: UnsafeRawPointer?) -> PropertyValue? {
    switch object {
    case Obj.plugIn:
        switch selector {
        case kAudioObjectPropertyBaseClass:    return .uint32(UInt32(kAudioObjectClassID))
        case kAudioObjectPropertyClass:        return .uint32(UInt32(kAudioPlugInClassID))
        case kAudioObjectPropertyOwner:        return .objectID(AudioObjectID(kAudioObjectUnknown))
        case kAudioObjectPropertyManufacturer: return .string(kManufacturer)
        case kAudioObjectPropertyOwnedObjects,
             kAudioPlugInPropertyDeviceList:   return .objectID(Obj.device)
        case kAudioPlugInPropertyTranslateUIDToDevice:
            let uid = qualifier?.load(as: CFString.self)
            let match = uid.map { CFEqual($0, kDeviceUID as CFString) } ?? false
            return .objectID(match ? Obj.device : AudioObjectID(kAudioObjectUnknown))
        case kAudioPlugInPropertyResourceBundle: return .string("")
        default: return nil
        }

    case Obj.device:
        switch selector {
        case kAudioObjectPropertyBaseClass:    return .uint32(UInt32(kAudioObjectClassID))
        case kAudioObjectPropertyClass:        return .uint32(UInt32(kAudioDeviceClassID))
        case kAudioObjectPropertyOwner:        return .objectID(Obj.plugIn)
        case kAudioObjectPropertyName:         return .string(kDeviceName)
        case kAudioObjectPropertyManufacturer: return .string(kManufacturer)
        case kAudioObjectPropertyOwnedObjects,
             kAudioDevicePropertyStreams:
            return scope == kAudioObjectPropertyScopeOutput ? .empty : .objectID(Obj.streamInput)
        case kAudioDevicePropertyDeviceUID:      return .string(kDeviceUID)
        case kAudioDevicePropertyModelUID:       return .string(kModelUID)
        case kAudioDevicePropertyTransportType:  return .uint32(UInt32(kAudioDeviceTransportTypeVirtual))
        case kAudioDevicePropertyRelatedDevices: return .objectID(Obj.device)
        case kAudioDevicePropertyClockDomain:    return .uint32(0)
        case kAudioDevicePropertyDeviceIsAlive:  return .uint32(1)
        case kAudioDevicePropertyDeviceIsRunning:
            lock(); let running = gIOCount > 0; unlock()
            return .uint32(running ? 1 : 0)
        case kAudioDevicePropertyDeviceCanBeDefaultDevice,
             kAudioDevicePropertyDeviceCanBeDefaultSystemDevice: return .uint32(1)
        case kAudioDevicePropertyLatency,
             kAudioDevicePropertySafetyOffset,
             kAudioDevicePropertyIsHidden:       return .uint32(0)
        case kAudioObjectPropertyControlList:    return .empty
        case kAudioDevicePropertyNominalSampleRate: return .float64(kSampleRate)
        case kAudioDevicePropertyAvailableNominalSampleRates:
            return .valueRange(AudioValueRange(mMinimum: kSampleRate, mMaximum: kSampleRate))
        case kAudioDevicePropertyPreferredChannelsForStereo: return .pair(1, 1)
        case kAudioDevicePropertyPreferredChannelLayout:
            var layout = AudioChannelLayout()
            layout.mChannelLayoutTag = kAudioChannelLayoutTag_Mono
            return .channelLayout(layout)
        case kAudioDevicePropertyZeroTimeStampPeriod: return .uint32(kRingBufferSize)
        default: return nil
        }

    case Obj.streamInput:
        switch selector {
        case kAudioObjectPropertyBaseClass:      return .uint32(UInt32(kAudioObjectClassID))
        case kAudioObjectPropertyClass:          return .uint32(UInt32(kAudioStreamClassID))
        case kAudioObjectPropertyOwner:          return .objectID(Obj.device)
        case kAudioObjectPropertyOwnedObjects:   return .empty
        case kAudioStreamPropertyIsActive:       return .uint32(1)
        case kAudioStreamPropertyDirection:      return .uint32(1) // 1 = input
        case kAudioStreamPropertyTerminalType:   return .uint32(UInt32(kAudioStreamTerminalTypeMicrophone))
        case kAudioStreamPropertyStartingChannel: return .uint32(1)
        case kAudioStreamPropertyLatency:        return .uint32(0)
        case kAudioStreamPropertyVirtualFormat,
             kAudioStreamPropertyPhysicalFormat: return .asbd(mockFormat())
        case kAudioStreamPropertyAvailableVirtualFormats,
             kAudioStreamPropertyAvailablePhysicalFormats:
            return .rangedFormat(AudioStreamRangedDescription(
                mFormat: mockFormat(),
                mSampleRateRange: AudioValueRange(mMinimum: kSampleRate, mMaximum: kSampleRate)))
        default: return nil
        }

    default:
        return nil
    }
}

private func PulseGetPropertyDataSize(_ obj: AudioObjectID,
                                     _ addr: UnsafePointer<AudioObjectPropertyAddress>?,
                                     _ outSize: UnsafeMutablePointer<UInt32>?) -> OSStatus {
    guard let a = addr?.pointee else { return kAudioHardwareBadObjectError }
    guard let v = value(for: obj, a.mSelector, scope: a.mScope, qualifier: nil) else {
        return kAudioHardwareUnknownPropertyError
    }
    outSize?.pointee = v.byteSize
    return noErr
}

private func PulseGetPropertyData(_ obj: AudioObjectID,
                                 _ addr: UnsafePointer<AudioObjectPropertyAddress>?,
                                 _ qualData: UnsafeRawPointer?,
                                 _ inDataSize: UInt32,
                                 _ outSize: UnsafeMutablePointer<UInt32>?,
                                 _ outData: UnsafeMutableRawPointer?) -> OSStatus {
    guard let a = addr?.pointee else { return kAudioHardwareBadObjectError }
    guard let v = value(for: obj, a.mSelector, scope: a.mScope, qualifier: qualData) else {
        return kAudioHardwareUnknownPropertyError
    }
    // Write only when the client's buffer can hold the whole value (all values
    // Pulse exposes are scalar or single-element, so it's all-or-nothing).
    if let outData, inDataSize >= v.byteSize {
        v.write(to: outData)
        outSize?.pointee = v.byteSize
    } else {
        outSize?.pointee = 0
    }
    return noErr
}

private func PulseSetPropertyData(_ obj: AudioObjectID,
                                 _ addr: UnsafePointer<AudioObjectPropertyAddress>?,
                                 _ inDataSize: UInt32,
                                 _ inData: UnsafeRawPointer?) -> OSStatus {
    guard let sel = addr?.pointee.mSelector else { return kAudioHardwareBadObjectError }
    switch sel {
    case kAudioDevicePropertyNominalSampleRate:
        let rate = inData?.load(as: Float64.self) ?? 0
        return rate == kSampleRate ? noErr : kAudioHardwareIllegalOperationError
    case kAudioStreamPropertyVirtualFormat, kAudioStreamPropertyPhysicalFormat:
        guard let f = inData?.load(as: AudioStreamBasicDescription.self) else { return kAudioHardwareIllegalOperationError }
        return (f.mSampleRate == kSampleRate && f.mChannelsPerFrame == kChannels) ? noErr : kAudioHardwareIllegalOperationError
    case kAudioStreamPropertyIsActive:
        return noErr
    default:
        return kAudioHardwareUnknownPropertyError
    }
}

// MARK: - Pulse audio transport

private func startAudioReceiver() {
    lock()
    if gReceiverStarted {
        unlock()
        return
    }
    gReceiverStarted = true
    unlock()

    Thread.detachNewThread {
        audioReceiverLoop()
    }
}

private func audioReceiverLoop() {
    let socketFD = socket(AF_INET, SOCK_DGRAM, 0)
    guard socketFD >= 0 else {
        gLog.error("Could not create Pulse loopback audio socket")
        return
    }

    var address = sockaddr_in()
    address.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
    address.sin_family = sa_family_t(AF_INET)
    address.sin_port = kAudioPort.bigEndian
    address.sin_addr = in_addr(s_addr: inet_addr("127.0.0.1"))
    let bindStatus = withUnsafePointer(to: &address) { pointer in
        pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { socketAddress in
            Darwin.bind(socketFD, socketAddress, socklen_t(MemoryLayout<sockaddr_in>.size))
        }
    }
    guard bindStatus == 0 else {
        gLog.error("Could not bind Pulse loopback audio socket on port \(kAudioPort)")
        Darwin.close(socketFD)
        return
    }

    let packet = UnsafeMutablePointer<Float32>.allocate(capacity: kPacketFrames)
    packet.initialize(repeating: 0, count: kPacketFrames)
    defer {
        packet.deinitialize(count: kPacketFrames)
        packet.deallocate()
        Darwin.close(socketFD)
    }

    while true {
        let byteCount = Darwin.recv(
            socketFD,
            UnsafeMutableRawPointer(packet),
            kPacketFrames * MemoryLayout<Float32>.size,
            0)
        if byteCount < 0 {
            if errno == EINTR { continue }
            usleep(10_000)
            continue
        }
        if byteCount == 1 {
            clearAudio()
            continue
        }
        if byteCount == 0 || byteCount % MemoryLayout<Float32>.size != 0 {
            continue
        }
        appendAudio(packet, sampleCount: byteCount / MemoryLayout<Float32>.size)
    }
}

private func clearAudio() {
    os_unfair_lock_lock(gAudioLock)
    gAudioReadIndex = 0
    gAudioWriteIndex = 0
    gAudioAvailable = 0
    os_unfair_lock_unlock(gAudioLock)
}

private func appendAudio(_ samples: UnsafePointer<Float32>, sampleCount: Int) {
    let retainedCount = min(sampleCount, kAudioBufferFrames)
    let sourceOffset = sampleCount - retainedCount

    os_unfair_lock_lock(gAudioLock)
    let overflow = max(0, gAudioAvailable + retainedCount - kAudioBufferFrames)
    gAudioReadIndex = (gAudioReadIndex + overflow) % kAudioBufferFrames
    gAudioAvailable -= overflow
    for index in 0..<retainedCount {
        gAudioBuffer[gAudioWriteIndex] = samples[sourceOffset + index]
        gAudioWriteIndex = (gAudioWriteIndex + 1) % kAudioBufferFrames
    }
    gAudioAvailable += retainedCount
    os_unfair_lock_unlock(gAudioLock)
}

@inline(__always)
private func readAudio(into output: UnsafeMutablePointer<Float32>, frameCount: Int) {
    os_unfair_lock_lock(gAudioLock)
    let readable = min(frameCount, gAudioAvailable)
    for index in 0..<readable {
        output[index] = gAudioBuffer[gAudioReadIndex]
        gAudioReadIndex = (gAudioReadIndex + 1) % kAudioBufferFrames
    }
    gAudioAvailable -= readable
    os_unfair_lock_unlock(gAudioLock)

    if readable < frameCount {
        memset(output.advanced(by: readable), 0, (frameCount - readable) * MemoryLayout<Float32>.size)
    }
}

// MARK: - IO

private func PulseStartIO(_ device: AudioObjectID) -> OSStatus {
    guard device == Obj.device else { return kAudioHardwareBadObjectError }
    lock()
    if gIOCount == 0 {
        var tb = mach_timebase_info_data_t()
        mach_timebase_info(&tb)
        let hostClockFrequency = (Float64(tb.denom) / Float64(tb.numer)) * 1.0e9
        gHostTicksPerFrame = hostClockFrequency / kSampleRate
        gNumberTimeStamps = 0
        gAnchorHostTime = mach_absolute_time()
    }
    gIOCount += 1
    unlock()
    return noErr
}

private func PulseStopIO(_ device: AudioObjectID) -> OSStatus {
    guard device == Obj.device else { return kAudioHardwareBadObjectError }
    lock(); if gIOCount > 0 { gIOCount -= 1 }; unlock()
    return noErr
}

private func PulseGetZeroTimeStamp(_ device: AudioObjectID,
                                  _ outSampleTime: UnsafeMutablePointer<Float64>?,
                                  _ outHostTime: UnsafeMutablePointer<UInt64>?,
                                  _ outSeed: UnsafeMutablePointer<UInt64>?) -> OSStatus {
    guard device == Obj.device else { return kAudioHardwareBadObjectError }
    lock()
    let currentHostTime = mach_absolute_time()
    let hostTicksPerRingBuffer = gHostTicksPerFrame * Float64(kRingBufferSize)
    let nextTicks = Float64(gNumberTimeStamps + 1) * hostTicksPerRingBuffer
    let nextHostTime = gAnchorHostTime + UInt64(nextTicks)
    if currentHostTime >= nextHostTime { gNumberTimeStamps += 1 }
    outSampleTime?.pointee = Float64(gNumberTimeStamps * UInt64(kRingBufferSize))
    outHostTime?.pointee = gAnchorHostTime + UInt64(Float64(gNumberTimeStamps) * hostTicksPerRingBuffer)
    outSeed?.pointee = 1
    unlock()
    return noErr
}
