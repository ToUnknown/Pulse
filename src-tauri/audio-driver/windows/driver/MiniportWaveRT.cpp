#include "PulseVirtualMic.h"
#include "NewDelete.h"
#include "MiniportWaveRT.h"
#include "Stream.h"

extern "C" PKSDATAFORMAT_WAVEFORMATEXTENSIBLE PulseGetNativeFormat();

#pragma code_seg("PAGE")
NTSTATUS PulseCreateWaveMiniport(PUNKNOWN* Unknown)
{
    PAGED_CODE();
    if (Unknown == nullptr)
    {
        return STATUS_INVALID_PARAMETER;
    }

    auto miniport = new (POOL_FLAG_NON_PAGED, PULSE_POOL_TAG) PulseWaveRTMiniport(nullptr);
    if (miniport == nullptr)
    {
        return STATUS_INSUFFICIENT_RESOURCES;
    }
    miniport->AddRef();
    *Unknown = reinterpret_cast<PUNKNOWN>(miniport);
    return STATUS_SUCCESS;
}

STDMETHODIMP PulseWaveRTMiniport::DataRangeIntersection(
    ULONG PinId,
    PKSDATARANGE ClientDataRange,
    PKSDATARANGE MyDataRange,
    ULONG OutputBufferLength,
    PVOID ResultantFormat,
    PULONG ResultantFormatLength)
{
    UNREFERENCED_PARAMETER(PinId);
    UNREFERENCED_PARAMETER(ClientDataRange);
    UNREFERENCED_PARAMETER(MyDataRange);
    UNREFERENCED_PARAMETER(ResultantFormat);
    PAGED_CODE();

    const ULONG required = sizeof(KSDATAFORMAT_WAVEFORMATEXTENSIBLE);
    if (ResultantFormatLength == nullptr)
    {
        return STATUS_INVALID_PARAMETER;
    }
    *ResultantFormatLength = required;
    if (OutputBufferLength == 0)
    {
        return STATUS_BUFFER_OVERFLOW;
    }
    if (OutputBufferLength < required)
    {
        return STATUS_BUFFER_TOO_SMALL;
    }
    return STATUS_NOT_IMPLEMENTED;
}

STDMETHODIMP PulseWaveRTMiniport::GetDescription(PPCFILTER_DESCRIPTOR OutFilterDescriptor)
{
    PAGED_CODE();
    if (OutFilterDescriptor == nullptr)
    {
        return STATUS_INVALID_PARAMETER;
    }
    *OutFilterDescriptor = g_PulseWaveFilterDescriptor;
    return STATUS_SUCCESS;
}

STDMETHODIMP PulseWaveRTMiniport::Init(
    PUNKNOWN UnknownAdapter,
    PRESOURCELIST ResourceList,
    PPORTWAVERT Port)
{
    UNREFERENCED_PARAMETER(UnknownAdapter);
    UNREFERENCED_PARAMETER(ResourceList);
    UNREFERENCED_PARAMETER(Port);
    PAGED_CODE();
    return STATUS_SUCCESS;
}

STDMETHODIMP PulseWaveRTMiniport::NewStream(
    PMINIPORTWAVERTSTREAM* OutStream,
    PPORTWAVERTSTREAM PortStream,
    ULONG Pin,
    BOOLEAN Capture,
    PKSDATAFORMAT DataFormat)
{
    PAGED_CODE();
    if (OutStream == nullptr || PortStream == nullptr || DataFormat == nullptr)
    {
        return STATUS_INVALID_PARAMETER;
    }
    *OutStream = nullptr;

    NTSTATUS status = IsFormatSupported(Pin, Capture, DataFormat);
    if (!NT_SUCCESS(status))
    {
        return status;
    }

    const LONG streams = InterlockedIncrement(&m_AllocatedStreams);
    if (streams > 8)
    {
        InterlockedDecrement(&m_AllocatedStreams);
        return STATUS_INSUFFICIENT_RESOURCES;
    }

    auto stream = new (POOL_FLAG_NON_PAGED, PULSE_POOL_TAG) PulseWaveRTStream(nullptr);
    if (stream == nullptr)
    {
        InterlockedDecrement(&m_AllocatedStreams);
        return STATUS_INSUFFICIENT_RESOURCES;
    }
    stream->AddRef();
    status = stream->Init(this, PortStream, DataFormat);
    if (NT_SUCCESS(status))
    {
        *OutStream = static_cast<PMINIPORTWAVERTSTREAM>(stream);
        (*OutStream)->AddRef();
    }
    else
    {
        InterlockedDecrement(&m_AllocatedStreams);
    }
    stream->Release();
    return status;
}

STDMETHODIMP PulseWaveRTMiniport::GetDeviceDescription(PDEVICE_DESCRIPTION Description)
{
    PAGED_CODE();
    if (Description == nullptr)
    {
        return STATUS_INVALID_PARAMETER;
    }
    RtlZeroMemory(Description, sizeof(*Description));
    Description->Master = TRUE;
    Description->ScatterGather = TRUE;
    Description->Dma32BitAddresses = TRUE;
    Description->InterfaceType = PCIBus;
    Description->MaximumLength = MAXULONG;
    return STATUS_SUCCESS;
}

STDMETHODIMP PulseWaveRTMiniport::NonDelegatingQueryInterface(REFIID Interface, PVOID* Object)
{
    PAGED_CODE();
    if (Object == nullptr)
    {
        return STATUS_INVALID_PARAMETER;
    }
    if (IsEqualGUIDAligned(Interface, IID_IUnknown) ||
        IsEqualGUIDAligned(Interface, IID_IMiniport) ||
        IsEqualGUIDAligned(Interface, IID_IMiniportWaveRT))
    {
        *Object = static_cast<PMINIPORTWAVERT>(this);
        PUNKNOWN(*Object)->AddRef();
        return STATUS_SUCCESS;
    }
    *Object = nullptr;
    return STATUS_INVALID_PARAMETER;
}

NTSTATUS PulseWaveRTMiniport::IsFormatSupported(ULONG Pin, BOOLEAN Capture, PKSDATAFORMAT DataFormat)
{
    PAGED_CODE();
    if (Pin != 1 || !Capture ||
        !IsEqualGUIDAligned(DataFormat->MajorFormat, KSDATAFORMAT_TYPE_AUDIO) ||
        !IsEqualGUIDAligned(DataFormat->SubFormat, KSDATAFORMAT_SUBTYPE_PCM) ||
        !IsEqualGUIDAligned(DataFormat->Specifier, KSDATAFORMAT_SPECIFIER_WAVEFORMATEX))
    {
        return STATUS_NO_MATCH;
    }

    const PWAVEFORMATEX format = GetWaveFormatEx(DataFormat);
    if (format == nullptr || format->nChannels != 1 || format->nSamplesPerSec != PULSE_SAMPLE_RATE ||
        format->wBitsPerSample != 16 || format->nBlockAlign != sizeof(INT16) ||
        format->nAvgBytesPerSec != PULSE_SAMPLE_RATE * sizeof(INT16))
    {
        return STATUS_NO_MATCH;
    }
    if (format->wFormatTag != WAVE_FORMAT_PCM && format->wFormatTag != WAVE_FORMAT_EXTENSIBLE)
    {
        return STATUS_NO_MATCH;
    }
    return STATUS_SUCCESS;
}

void PulseWaveRTMiniport::StreamClosed()
{
    InterlockedDecrement(&m_AllocatedStreams);
}

#pragma code_seg()
