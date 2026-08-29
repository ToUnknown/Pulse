#include "PulseVirtualMic.h"
#include "MiniportWaveRT.h"
#include "Stream.h"

namespace
{
constexpr LONGLONG PacketPeriod100ns = 100000;

void FreeNotificationList(_Inout_ LIST_ENTRY* Head)
{
    while (!IsListEmpty(Head))
    {
        PLIST_ENTRY entry = RemoveHeadList(Head);
        auto notification = CONTAINING_RECORD(entry, PulseNotificationEntry, ListEntry);
        ExFreePoolWithTag(notification, PULSE_POOL_TAG);
    }
}
}

#pragma code_seg()
PulseWaveRTStream::~PulseWaveRTStream()
{
    KIRQL oldIrql;
    KeAcquireSpinLock(&m_PositionLock, &oldIrql);
    const bool wasRunning = m_State == KSSTATE_RUN;
    m_State = KSSTATE_STOP;
    PEX_TIMER timerToDelete = m_Timer;
    m_Timer = nullptr;
    if (timerToDelete != nullptr)
    {
        ExCancelTimer(timerToDelete, nullptr);
    }
    KeReleaseSpinLock(&m_PositionLock, oldIrql);

    if (timerToDelete != nullptr)
    {
        ExDeleteTimer(timerToDelete, TRUE, TRUE, nullptr);
    }
    if (wasRunning)
    {
        g_PulseAudioRing.SetCaptureRunning(false);
    }
    FreeNotificationList(&m_NotificationList);
    if (m_Miniport != nullptr)
    {
        m_Miniport->StreamClosed();
        m_Miniport->Release();
        m_Miniport = nullptr;
    }
}

#pragma code_seg("PAGE")
NTSTATUS PulseWaveRTStream::Init(
    PulseWaveRTMiniport* Miniport,
    PPORTWAVERTSTREAM PortStream,
    PKSDATAFORMAT DataFormat)
{
    PAGED_CODE();
    if (Miniport == nullptr || PortStream == nullptr || DataFormat == nullptr)
    {
        return STATUS_INVALID_PARAMETER;
    }
    m_Miniport = Miniport;
    m_Miniport->AddRef();
    m_PortStream = PortStream;
    g_PulseAudioRing.StartReader(&m_RingCursor, &m_ResetGeneration);

    m_Timer = ExAllocateTimer(PulseStreamTimer, this, EX_TIMER_HIGH_RESOLUTION);
    if (m_Timer == nullptr)
    {
        return STATUS_INSUFFICIENT_RESOURCES;
    }
    return STATUS_SUCCESS;
}

NTSTATUS PulseWaveRTStream::AllocateBufferWithNotification(
    ULONG NotificationCount,
    ULONG RequestedSize,
    PMDL* AudioBufferMdl,
    ULONG* ActualSize,
    ULONG* OffsetFromFirstPage,
    MEMORY_CACHING_TYPE* CacheType)
{
    PAGED_CODE();
    if (NotificationCount == 0 || NotificationCount > 64 || RequestedSize < sizeof(INT16) ||
        AudioBufferMdl == nullptr || ActualSize == nullptr || OffsetFromFirstPage == nullptr || CacheType == nullptr ||
        m_BufferMdl != nullptr)
    {
        return STATUS_INVALID_PARAMETER;
    }

    const ULONG size = PULSE_PACKET_BYTES * NotificationCount;
    PHYSICAL_ADDRESS highestAddress = {};
    highestAddress.QuadPart = MAXULONG;
    PMDL mdl = m_PortStream->AllocatePagesForMdl(highestAddress, size);
    if (mdl == nullptr)
    {
        return STATUS_INSUFFICIENT_RESOURCES;
    }
    BYTE* buffer = static_cast<BYTE*>(m_PortStream->MapAllocatedPages(mdl, MmCached));
    if (buffer == nullptr)
    {
        m_PortStream->FreePagesFromMdl(mdl);
        return STATUS_INSUFFICIENT_RESOURCES;
    }

    RtlZeroMemory(buffer, size);
    m_BufferMdl = mdl;
    m_Buffer = buffer;
    m_BufferSize = size;
    m_Notifications = NotificationCount;
    *AudioBufferMdl = mdl;
    *ActualSize = size;
    *OffsetFromFirstPage = 0;
    *CacheType = MmCached;
    return STATUS_SUCCESS;
}

VOID PulseWaveRTStream::FreeBufferWithNotification(PMDL Mdl, ULONG Size)
{
    UNREFERENCED_PARAMETER(Size);
    PAGED_CODE();
    FreeAudioBuffer(Mdl, Size);
}

#pragma code_seg()
NTSTATUS PulseWaveRTStream::RegisterNotificationEvent(PKEVENT NotificationEvent)
{
    if (NotificationEvent == nullptr)
    {
        return STATUS_INVALID_PARAMETER;
    }
    auto notification = static_cast<PulseNotificationEntry*>(
        ExAllocatePool2(POOL_FLAG_NON_PAGED, sizeof(PulseNotificationEntry), PULSE_POOL_TAG));
    if (notification == nullptr)
    {
        return STATUS_INSUFFICIENT_RESOURCES;
    }
    notification->Event = NotificationEvent;

    KIRQL oldIrql;
    KeAcquireSpinLock(&m_NotificationLock, &oldIrql);
    for (PLIST_ENTRY entry = m_NotificationList.Flink; entry != &m_NotificationList; entry = entry->Flink)
    {
        auto current = CONTAINING_RECORD(entry, PulseNotificationEntry, ListEntry);
        if (current->Event == NotificationEvent)
        {
            KeReleaseSpinLock(&m_NotificationLock, oldIrql);
            ExFreePoolWithTag(notification, PULSE_POOL_TAG);
            return STATUS_OBJECT_NAME_COLLISION;
        }
    }
    InsertTailList(&m_NotificationList, &notification->ListEntry);
    KeReleaseSpinLock(&m_NotificationLock, oldIrql);
    return STATUS_SUCCESS;
}

NTSTATUS PulseWaveRTStream::UnregisterNotificationEvent(PKEVENT NotificationEvent)
{
    KIRQL oldIrql;
    KeAcquireSpinLock(&m_NotificationLock, &oldIrql);
    for (PLIST_ENTRY entry = m_NotificationList.Flink; entry != &m_NotificationList; entry = entry->Flink)
    {
        auto current = CONTAINING_RECORD(entry, PulseNotificationEntry, ListEntry);
        if (current->Event == NotificationEvent)
        {
            RemoveEntryList(entry);
            KeReleaseSpinLock(&m_NotificationLock, oldIrql);
            ExFreePoolWithTag(current, PULSE_POOL_TAG);
            return STATUS_SUCCESS;
        }
    }
    KeReleaseSpinLock(&m_NotificationLock, oldIrql);
    return STATUS_NOT_FOUND;
}

#pragma code_seg("PAGE")
NTSTATUS PulseWaveRTStream::GetClockRegister(PKSRTAUDIO_HWREGISTER Register)
{
    UNREFERENCED_PARAMETER(Register);
    PAGED_CODE();
    return STATUS_NOT_IMPLEMENTED;
}

NTSTATUS PulseWaveRTStream::GetPositionRegister(PKSRTAUDIO_HWREGISTER Register)
{
    UNREFERENCED_PARAMETER(Register);
    PAGED_CODE();
    return STATUS_NOT_IMPLEMENTED;
}

VOID PulseWaveRTStream::GetHWLatency(PKSRTAUDIO_HWLATENCY Latency)
{
    PAGED_CODE();
    if (Latency != nullptr)
    {
        Latency->ChipsetDelay = 0;
        Latency->CodecDelay = 0;
        Latency->FifoSize = PULSE_PACKET_BYTES;
    }
}

VOID PulseWaveRTStream::FreeAudioBuffer(PMDL Mdl, ULONG Size)
{
    UNREFERENCED_PARAMETER(Size);
    PAGED_CODE();
    if (Mdl != nullptr)
    {
        if (m_Buffer != nullptr)
        {
            m_PortStream->UnmapAllocatedPages(m_Buffer, Mdl);
        }
        m_PortStream->FreePagesFromMdl(Mdl);
    }
    m_BufferMdl = nullptr;
    m_Buffer = nullptr;
    m_BufferSize = 0;
    m_Notifications = 0;
}

NTSTATUS PulseWaveRTStream::AllocateAudioBuffer(
    ULONG RequestedSize,
    PMDL* AudioBufferMdl,
    ULONG* ActualSize,
    ULONG* OffsetFromFirstPage,
    MEMORY_CACHING_TYPE* CacheType)
{
    PAGED_CODE();
    if (RequestedSize < PULSE_PACKET_BYTES || AudioBufferMdl == nullptr || ActualSize == nullptr ||
        OffsetFromFirstPage == nullptr || CacheType == nullptr || m_BufferMdl != nullptr)
    {
        return STATUS_INVALID_PARAMETER;
    }
    RequestedSize -= RequestedSize % sizeof(INT16);
    PHYSICAL_ADDRESS highestAddress = {};
    highestAddress.QuadPart = MAXULONG;
    PMDL mdl = m_PortStream->AllocatePagesForMdl(highestAddress, RequestedSize);
    if (mdl == nullptr)
    {
        return STATUS_INSUFFICIENT_RESOURCES;
    }
    BYTE* buffer = static_cast<BYTE*>(m_PortStream->MapAllocatedPages(mdl, MmCached));
    if (buffer == nullptr)
    {
        m_PortStream->FreePagesFromMdl(mdl);
        return STATUS_INSUFFICIENT_RESOURCES;
    }
    RtlZeroMemory(buffer, RequestedSize);
    m_BufferMdl = mdl;
    m_Buffer = buffer;
    m_BufferSize = RequestedSize;
    m_Notifications = 0;
    *AudioBufferMdl = mdl;
    *ActualSize = RequestedSize;
    *OffsetFromFirstPage = 0;
    *CacheType = MmCached;
    return STATUS_SUCCESS;
}

#pragma code_seg()
NTSTATUS PulseWaveRTStream::SetState(KSSTATE State)
{
    if (State < KSSTATE_STOP || State > KSSTATE_RUN)
    {
        return STATUS_INVALID_PARAMETER;
    }

    KIRQL oldIrql;
    KeAcquireSpinLock(&m_PositionLock, &oldIrql);
    const KSSTATE oldState = m_State;
    if (m_Timer == nullptr)
    {
        KeReleaseSpinLock(&m_PositionLock, oldIrql);
        return STATUS_DEVICE_NOT_READY;
    }
    m_State = State;
    if (State == KSSTATE_STOP)
    {
        m_LinearPosition = 0;
        m_PacketCounter = 0;
        m_LastReadPacket = MAXULONG;
        m_LastPacketQpc = 0;
    }
    if (oldState == KSSTATE_RUN && State != KSSTATE_RUN)
    {
        ExCancelTimer(m_Timer, nullptr);
        g_PulseAudioRing.SetCaptureRunning(false);
    }
    else if (oldState != KSSTATE_RUN && State == KSSTATE_RUN)
    {
        g_PulseAudioRing.StartReader(&m_RingCursor, &m_ResetGeneration);
        g_PulseAudioRing.SetCaptureRunning(true);
        ExSetTimer(m_Timer, -PacketPeriod100ns, PacketPeriod100ns, nullptr);
    }
    KeReleaseSpinLock(&m_PositionLock, oldIrql);
    return STATUS_SUCCESS;
}

#pragma code_seg("PAGE")
NTSTATUS PulseWaveRTStream::SetFormat(PKSDATAFORMAT DataFormat)
{
    PAGED_CODE();
    return m_Miniport->IsFormatSupported(1, TRUE, DataFormat);
}

STDMETHODIMP PulseWaveRTStream::NonDelegatingQueryInterface(REFIID Interface, PVOID* Object)
{
    PAGED_CODE();
    if (Object == nullptr)
    {
        return STATUS_INVALID_PARAMETER;
    }
    if (IsEqualGUIDAligned(Interface, IID_IUnknown) ||
        IsEqualGUIDAligned(Interface, IID_IMiniportWaveRTStream))
    {
        *Object = static_cast<PMINIPORTWAVERTSTREAM>(this);
    }
    else if (IsEqualGUIDAligned(Interface, IID_IMiniportWaveRTStreamNotification))
    {
        *Object = static_cast<PMINIPORTWAVERTSTREAMNOTIFICATION>(this);
    }
    else if (IsEqualGUIDAligned(Interface, IID_IMiniportWaveRTInputStream))
    {
        *Object = static_cast<PMINIPORTWAVERTINPUTSTREAM>(this);
    }
    else
    {
        *Object = nullptr;
        return STATUS_INVALID_PARAMETER;
    }
    PUNKNOWN(*Object)->AddRef();
    return STATUS_SUCCESS;
}

#pragma code_seg()
NTSTATUS PulseWaveRTStream::GetPosition(KSAUDIO_POSITION* Position)
{
    if (Position == nullptr)
    {
        return STATUS_INVALID_PARAMETER;
    }
    KIRQL oldIrql;
    KeAcquireSpinLock(&m_PositionLock, &oldIrql);
    const ULONG offset = m_BufferSize == 0 ? 0 : static_cast<ULONG>(m_LinearPosition % m_BufferSize);
    Position->PlayOffset = offset;
    Position->WriteOffset = offset;
    KeReleaseSpinLock(&m_PositionLock, oldIrql);
    return STATUS_SUCCESS;
}

NTSTATUS PulseWaveRTStream::GetReadPacket(
    ULONG* PacketNumber,
    DWORD* Flags,
    ULONG64* PerformanceCounterValue,
    BOOL* MoreData)
{
    if (PacketNumber == nullptr || Flags == nullptr || PerformanceCounterValue == nullptr || MoreData == nullptr)
    {
        return STATUS_INVALID_PARAMETER;
    }
    if (m_Notifications == 0)
    {
        return STATUS_NOT_SUPPORTED;
    }

    KIRQL oldIrql;
    KeAcquireSpinLock(&m_PositionLock, &oldIrql);
    if (m_State < KSSTATE_PAUSE)
    {
        KeReleaseSpinLock(&m_PositionLock, oldIrql);
        return STATUS_INVALID_DEVICE_STATE;
    }
    const ULONG available = static_cast<ULONG>(m_PacketCounter - 1);
    if (available == m_LastReadPacket)
    {
        KeReleaseSpinLock(&m_PositionLock, oldIrql);
        return STATUS_DEVICE_NOT_READY;
    }
    m_LastReadPacket = available;
    *PacketNumber = available;
    *PerformanceCounterValue = m_LastPacketQpc;
    *Flags = 0;
    *MoreData = FALSE;
    KeReleaseSpinLock(&m_PositionLock, oldIrql);
    return STATUS_SUCCESS;
}

void PulseWaveRTStream::TransferPacket()
{
    BYTE* destination = nullptr;
    ULONG firstBytes = 0;
    ULONG secondBytes = 0;

    KIRQL oldIrql;
    KeAcquireSpinLock(&m_PositionLock, &oldIrql);
    if (m_State != KSSTATE_RUN || m_Buffer == nullptr || m_BufferSize < PULSE_PACKET_BYTES)
    {
        KeReleaseSpinLock(&m_PositionLock, oldIrql);
        return;
    }
    const ULONG offset = static_cast<ULONG>(m_LinearPosition % m_BufferSize);
    destination = m_Buffer + offset;
    firstBytes = min(PULSE_PACKET_BYTES, m_BufferSize - offset);
    secondBytes = PULSE_PACKET_BYTES - firstBytes;
    KeReleaseSpinLock(&m_PositionLock, oldIrql);

    g_PulseAudioRing.Read(&m_RingCursor, &m_ResetGeneration,
                          reinterpret_cast<INT16*>(destination), firstBytes / sizeof(INT16));
    if (secondBytes != 0)
    {
        g_PulseAudioRing.Read(&m_RingCursor, &m_ResetGeneration,
                              reinterpret_cast<INT16*>(m_Buffer), secondBytes / sizeof(INT16));
    }

    LARGE_INTEGER qpc = KeQueryPerformanceCounter(nullptr);
    KeAcquireSpinLock(&m_PositionLock, &oldIrql);
    if (m_State == KSSTATE_RUN)
    {
        m_LinearPosition += PULSE_PACKET_BYTES;
        ++m_PacketCounter;
        m_LastPacketQpc = static_cast<ULONG64>(qpc.QuadPart);
    }
    KeReleaseSpinLock(&m_PositionLock, oldIrql);

    KeAcquireSpinLock(&m_NotificationLock, &oldIrql);
    for (PLIST_ENTRY entry = m_NotificationList.Flink; entry != &m_NotificationList; entry = entry->Flink)
    {
        auto notification = CONTAINING_RECORD(entry, PulseNotificationEntry, ListEntry);
        KeSetEvent(notification->Event, 0, FALSE);
    }
    KeReleaseSpinLock(&m_NotificationLock, oldIrql);
}

_Use_decl_annotations_
void PulseStreamTimer(PEX_TIMER Timer, PVOID Context)
{
    UNREFERENCED_PARAMETER(Timer);
    auto stream = static_cast<PulseWaveRTStream*>(Context);
    if (stream != nullptr)
    {
        stream->TransferPacket();
    }
}
