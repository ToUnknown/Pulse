#include "PulseVirtualMic.h"
#include "NewDelete.h"
#include "MiniportTopology.h"

#pragma code_seg("PAGE")
NTSTATUS PulseCreateTopologyMiniport(PUNKNOWN* Unknown)
{
    PAGED_CODE();
    if (Unknown == nullptr)
    {
        return STATUS_INVALID_PARAMETER;
    }

    auto miniport = new (POOL_FLAG_PAGED, PULSE_POOL_TAG) PulseTopologyMiniport(nullptr);
    if (miniport == nullptr)
    {
        return STATUS_INSUFFICIENT_RESOURCES;
    }
    miniport->AddRef();
    *Unknown = reinterpret_cast<PUNKNOWN>(miniport);
    return STATUS_SUCCESS;
}

STDMETHODIMP PulseTopologyMiniport::DataRangeIntersection(
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
    UNREFERENCED_PARAMETER(OutputBufferLength);
    UNREFERENCED_PARAMETER(ResultantFormat);
    UNREFERENCED_PARAMETER(ResultantFormatLength);
    PAGED_CODE();
    return STATUS_NOT_IMPLEMENTED;
}

STDMETHODIMP PulseTopologyMiniport::GetDescription(PPCFILTER_DESCRIPTOR OutFilterDescriptor)
{
    PAGED_CODE();
    if (OutFilterDescriptor == nullptr)
    {
        return STATUS_INVALID_PARAMETER;
    }
    *OutFilterDescriptor = g_PulseTopologyFilterDescriptor;
    return STATUS_SUCCESS;
}

STDMETHODIMP PulseTopologyMiniport::Init(
    PUNKNOWN UnknownAdapter,
    PRESOURCELIST ResourceList,
    PPORTTOPOLOGY Port)
{
    UNREFERENCED_PARAMETER(UnknownAdapter);
    UNREFERENCED_PARAMETER(ResourceList);
    UNREFERENCED_PARAMETER(Port);
    PAGED_CODE();
    return STATUS_SUCCESS;
}

STDMETHODIMP PulseTopologyMiniport::NonDelegatingQueryInterface(REFIID Interface, PVOID* Object)
{
    PAGED_CODE();
    if (Object == nullptr)
    {
        return STATUS_INVALID_PARAMETER;
    }

    if (IsEqualGUIDAligned(Interface, IID_IUnknown) ||
        IsEqualGUIDAligned(Interface, IID_IMiniport) ||
        IsEqualGUIDAligned(Interface, IID_IMiniportTopology))
    {
        *Object = static_cast<PMINIPORTTOPOLOGY>(this);
        PUNKNOWN(*Object)->AddRef();
        return STATUS_SUCCESS;
    }

    *Object = nullptr;
    return STATUS_INVALID_PARAMETER;
}

#pragma code_seg()
