#pragma once

#include "PulseVirtualMic.h"

class PulseWaveRTMiniport;

struct PulseNotificationEntry
{
    LIST_ENTRY ListEntry;
    PKEVENT Event;
};

void PulseStreamTimer(_In_ PEX_TIMER Timer, _In_opt_ PVOID Context);

class PulseWaveRTStream final :
    public IMiniportWaveRTStreamNotification,
    public IMiniportWaveRTInputStream,
    public CUnknown
{
public:
    explicit PulseWaveRTStream(_In_opt_ PUNKNOWN OuterUnknown)
        : CUnknown(OuterUnknown)
    {
    }
    ~PulseWaveRTStream();

    DECLARE_STD_UNKNOWN();
    IMP_IMiniportWaveRTStream;
    IMP_IMiniportWaveRTStreamNotification;
    IMP_IMiniportWaveRTInputStream;
    IMP_IMiniportWaveRT;

    NTSTATUS Init(_In_ PulseWaveRTMiniport* Miniport,
                  _In_ PPORTWAVERTSTREAM PortStream,
                  _In_ PKSDATAFORMAT DataFormat);
    void TransferPacket();

private:
    friend void PulseStreamTimer(_In_ PEX_TIMER Timer, _In_opt_ PVOID Context);

    PulseWaveRTMiniport* m_Miniport = nullptr;
    PPORTWAVERTSTREAM m_PortStream = nullptr;
    PEX_TIMER m_Timer = nullptr;
    PMDL m_BufferMdl = nullptr;
    BYTE* m_Buffer = nullptr;
    ULONG m_BufferSize = 0;
    ULONG m_Notifications = 0;
    KSSTATE m_State = KSSTATE_STOP;
    UINT64 m_RingCursor = 0;
    UINT64 m_ResetGeneration = 0;
    UINT64 m_LinearPosition = 0;
    LONGLONG m_PacketCounter = 0;
    ULONG m_LastReadPacket = MAXULONG;
    ULONG64 m_LastPacketQpc = 0;
    KSPIN_LOCK m_PositionLock;
    KSPIN_LOCK m_NotificationLock;
    LIST_ENTRY m_NotificationList;
};

