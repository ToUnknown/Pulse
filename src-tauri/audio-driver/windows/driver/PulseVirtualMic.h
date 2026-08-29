#pragma once

#include <ntddk.h>
#include <portcls.h>
#define _NEW_DELETE_OPERATORS_
#include <stdunk.h>
#include <ks.h>
#include <ksmedia.h>

#define PULSE_POOL_TAG 'MluP'
#define PULSE_PROTOCOL_VERSION 1u
#define PULSE_SAMPLE_RATE 48000u
#define PULSE_PACKET_SAMPLES 480u
#define PULSE_RING_SAMPLES 48000u
#define PULSE_PACKET_BYTES (PULSE_PACKET_SAMPLES * sizeof(INT16))

#define PULSE_WAVE_NAME L"PulseWave"
#define PULSE_TOPOLOGY_NAME L"PulseTopology"

// {DE117D34-8BF5-476A-962E-06D5E5BE5616}
DEFINE_GUIDSTRUCT("DE117D34-8BF5-476A-962E-06D5E5BE5616", GUID_DEVINTERFACE_PULSE_VIRTUAL_MIC);
#define GUID_DEVINTERFACE_PULSE_VIRTUAL_MIC DEFINE_GUIDNAMED(GUID_DEVINTERFACE_PULSE_VIRTUAL_MIC)

// {14E66A37-CA62-46B9-B7B9-461210775888}
DEFINE_GUIDSTRUCT("14E66A37-CA62-46B9-B7B9-461210775888", KSNAME_PulseWave);
#define KSNAME_PulseWave DEFINE_GUIDNAMED(KSNAME_PulseWave)

// {3BB345A0-B900-41F4-8E3A-4A9627FDD728}
DEFINE_GUIDSTRUCT("3BB345A0-B900-41F4-8E3A-4A9627FDD728", KSNAME_PulseTopology);
#define KSNAME_PulseTopology DEFINE_GUIDNAMED(KSNAME_PulseTopology)

#define IOCTL_PULSE_GET_VERSION CTL_CODE(FILE_DEVICE_UNKNOWN, 0x800, METHOD_BUFFERED, FILE_READ_ACCESS)
#define IOCTL_PULSE_GET_CONSUMER_STATE CTL_CODE(FILE_DEVICE_UNKNOWN, 0x801, METHOD_BUFFERED, FILE_READ_ACCESS)
#define IOCTL_PULSE_RESET_AUDIO CTL_CODE(FILE_DEVICE_UNKNOWN, 0x802, METHOD_BUFFERED, FILE_READ_ACCESS | FILE_WRITE_ACCESS)
#define IOCTL_PULSE_WRITE_AUDIO CTL_CODE(FILE_DEVICE_UNKNOWN, 0x803, METHOD_BUFFERED, FILE_WRITE_ACCESS)

#pragma pack(push, 1)
struct PULSE_VERSION_REPLY
{
    UINT32 ProtocolVersion;
    UINT32 PacketSamples;
    UINT32 SampleRate;
    UINT32 RingCapacitySamples;
    UINT32 Flags;
};

struct PULSE_CONSUMER_STATE_REPLY
{
    UINT32 ProtocolVersion;
    UINT32 ActiveCaptureStreams;
    UINT64 StateGeneration;
};

struct PULSE_RESET_REQUEST
{
    UINT32 ProtocolVersion;
    UINT32 Reserved;
};

struct PULSE_AUDIO_PACKET
{
    UINT32 ProtocolVersion;
    UINT32 SequenceNumber;
    UINT32 SampleCount;
    INT16 Samples[PULSE_PACKET_SAMPLES];
};
#pragma pack(pop)

static_assert(sizeof(PULSE_AUDIO_PACKET) == 972, "The user-mode packet ABI must remain fixed.");

class PulseAudioRing
{
public:
    void Initialize();
    NTSTATUS AcquireWriter(_In_ PFILE_OBJECT FileObject);
    void ReleaseWriter(_In_ PFILE_OBJECT FileObject);
    NTSTATUS Write(_In_ PFILE_OBJECT FileObject, _In_ const PULSE_AUDIO_PACKET* Packet);
    NTSTATUS Reset(_In_ PFILE_OBJECT FileObject);
    void ResetAll();
    void SetCaptureRunning(_In_ bool Running);
    void GetConsumerState(_Out_ PULSE_CONSUMER_STATE_REPLY* State);
    void StartReader(_Out_ UINT64* Cursor, _Out_ UINT64* ResetGeneration);
    void Read(_Inout_ UINT64* Cursor, _Inout_ UINT64* ResetGeneration,
              _Out_writes_(SampleCount) INT16* Samples, _In_ ULONG SampleCount);

private:
    void ResetLocked();

    KSPIN_LOCK m_Lock;
    PFILE_OBJECT m_Writer;
    UINT64 m_WritePosition;
    UINT64 m_ResetGeneration;
    UINT64 m_StateGeneration;
    ULONG m_ActiveCaptureStreams;
    INT16 m_Samples[PULSE_RING_SAMPLES];
};

extern PulseAudioRing g_PulseAudioRing;
extern UNICODE_STRING g_PulseControlInterface;

extern "C" DRIVER_INITIALIZE DriverEntry;
extern "C" NTSTATUS PulseAddDevice(_In_ PDRIVER_OBJECT DriverObject, _In_ PDEVICE_OBJECT PhysicalDeviceObject);
extern "C" NTSTATUS PulseStartDevice(_In_ PDEVICE_OBJECT DeviceObject, _In_ PIRP Irp,
                                      _In_ PRESOURCELIST ResourceList);

NTSTATUS PulseCreateWaveMiniport(_Out_ PUNKNOWN* Unknown);
NTSTATUS PulseCreateTopologyMiniport(_Out_ PUNKNOWN* Unknown);

extern PPCFILTER_DESCRIPTOR g_PulseWaveFilterDescriptor;
extern PPCFILTER_DESCRIPTOR g_PulseTopologyFilterDescriptor;
