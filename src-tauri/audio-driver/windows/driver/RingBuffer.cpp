#include "PulseVirtualMic.h"

PulseAudioRing g_PulseAudioRing;

void PulseAudioRing::Initialize()
{
    KeInitializeSpinLock(&m_Lock);
    m_Writer = nullptr;
    m_WritePosition = 0;
    m_ResetGeneration = 1;
    m_StateGeneration = 1;
    m_ActiveCaptureStreams = 0;
    RtlZeroMemory(m_Samples, sizeof(m_Samples));
}

NTSTATUS PulseAudioRing::AcquireWriter(PFILE_OBJECT FileObject)
{
    KIRQL oldIrql;
    KeAcquireSpinLock(&m_Lock, &oldIrql);
    NTSTATUS status = STATUS_SUCCESS;
    if (m_Writer == nullptr)
    {
        m_Writer = FileObject;
    }
    else if (m_Writer != FileObject)
    {
        status = STATUS_SHARING_VIOLATION;
    }
    KeReleaseSpinLock(&m_Lock, oldIrql);
    return status;
}

void PulseAudioRing::ReleaseWriter(PFILE_OBJECT FileObject)
{
    KIRQL oldIrql;
    KeAcquireSpinLock(&m_Lock, &oldIrql);
    if (m_Writer == FileObject)
    {
        m_Writer = nullptr;
        ResetLocked();
    }
    KeReleaseSpinLock(&m_Lock, oldIrql);
}

NTSTATUS PulseAudioRing::Write(PFILE_OBJECT FileObject, const PULSE_AUDIO_PACKET* Packet)
{
    KIRQL oldIrql;
    KeAcquireSpinLock(&m_Lock, &oldIrql);
    if (m_Writer != FileObject)
    {
        KeReleaseSpinLock(&m_Lock, oldIrql);
        return STATUS_SHARING_VIOLATION;
    }

    for (ULONG index = 0; index < PULSE_PACKET_SAMPLES; ++index)
    {
        m_Samples[(m_WritePosition + index) % PULSE_RING_SAMPLES] = Packet->Samples[index];
    }
    m_WritePosition += PULSE_PACKET_SAMPLES;
    KeReleaseSpinLock(&m_Lock, oldIrql);
    return STATUS_SUCCESS;
}

NTSTATUS PulseAudioRing::Reset(PFILE_OBJECT FileObject)
{
    KIRQL oldIrql;
    KeAcquireSpinLock(&m_Lock, &oldIrql);
    if (m_Writer != FileObject)
    {
        KeReleaseSpinLock(&m_Lock, oldIrql);
        return STATUS_SHARING_VIOLATION;
    }
    ResetLocked();
    KeReleaseSpinLock(&m_Lock, oldIrql);
    return STATUS_SUCCESS;
}

void PulseAudioRing::ResetAll()
{
    KIRQL oldIrql;
    KeAcquireSpinLock(&m_Lock, &oldIrql);
    m_Writer = nullptr;
    ResetLocked();
    KeReleaseSpinLock(&m_Lock, oldIrql);
}

void PulseAudioRing::SetCaptureRunning(bool Running)
{
    KIRQL oldIrql;
    KeAcquireSpinLock(&m_Lock, &oldIrql);
    if (Running)
    {
        ++m_ActiveCaptureStreams;
    }
    else if (m_ActiveCaptureStreams != 0)
    {
        --m_ActiveCaptureStreams;
    }
    ++m_StateGeneration;
    KeReleaseSpinLock(&m_Lock, oldIrql);
}

void PulseAudioRing::GetConsumerState(PULSE_CONSUMER_STATE_REPLY* State)
{
    KIRQL oldIrql;
    KeAcquireSpinLock(&m_Lock, &oldIrql);
    State->ProtocolVersion = PULSE_PROTOCOL_VERSION;
    State->ActiveCaptureStreams = m_ActiveCaptureStreams;
    State->StateGeneration = m_StateGeneration;
    KeReleaseSpinLock(&m_Lock, oldIrql);
}

void PulseAudioRing::StartReader(UINT64* Cursor, UINT64* ResetGeneration)
{
    KIRQL oldIrql;
    KeAcquireSpinLock(&m_Lock, &oldIrql);
    *Cursor = m_WritePosition;
    *ResetGeneration = m_ResetGeneration;
    KeReleaseSpinLock(&m_Lock, oldIrql);
}

void PulseAudioRing::Read(UINT64* Cursor, UINT64* ResetGeneration, INT16* Samples, ULONG SampleCount)
{
    KIRQL oldIrql;
    KeAcquireSpinLock(&m_Lock, &oldIrql);

    if (*ResetGeneration != m_ResetGeneration)
    {
        *Cursor = m_WritePosition;
        *ResetGeneration = m_ResetGeneration;
    }

    const UINT64 oldest = m_WritePosition > PULSE_RING_SAMPLES
        ? m_WritePosition - PULSE_RING_SAMPLES
        : 0;
    if (*Cursor < oldest)
    {
        *Cursor = oldest;
    }

    const UINT64 available64 = m_WritePosition - *Cursor;
    const ULONG available = available64 > MAXULONG ? MAXULONG : static_cast<ULONG>(available64);
    const ULONG copied = min(available, SampleCount);
    for (ULONG index = 0; index < copied; ++index)
    {
        Samples[index] = m_Samples[(*Cursor + index) % PULSE_RING_SAMPLES];
    }
    *Cursor += copied;
    if (copied < SampleCount)
    {
        RtlZeroMemory(Samples + copied, (SampleCount - copied) * sizeof(INT16));
    }

    KeReleaseSpinLock(&m_Lock, oldIrql);
}

void PulseAudioRing::ResetLocked()
{
    ++m_ResetGeneration;
    RtlZeroMemory(m_Samples, sizeof(m_Samples));
}
