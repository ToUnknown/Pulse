#pragma once

#include "PulseVirtualMic.h"

class PulseWaveRTStream;

class PulseWaveRTMiniport final : public IMiniportWaveRT, public CUnknown
{
public:
    explicit PulseWaveRTMiniport(_In_opt_ PUNKNOWN OuterUnknown)
        : CUnknown(OuterUnknown), m_AllocatedStreams(0)
    {
    }

    DECLARE_STD_UNKNOWN();
    IMP_IMiniportWaveRT;

    NTSTATUS IsFormatSupported(_In_ ULONG Pin, _In_ BOOLEAN Capture, _In_ PKSDATAFORMAT DataFormat);
    void StreamClosed();

private:
    volatile LONG m_AllocatedStreams;
};

