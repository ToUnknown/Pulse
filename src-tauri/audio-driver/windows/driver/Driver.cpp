#include <initguid.h>
#include "PulseVirtualMic.h"
#include "NewDelete.h"

UNICODE_STRING g_PulseControlInterface = {};

namespace
{
PDRIVER_DISPATCH g_PortClsDeviceControl = nullptr;
PDRIVER_DISPATCH g_PortClsCleanup = nullptr;
PDRIVER_DISPATCH g_PortClsPnp = nullptr;
PDRIVER_UNLOAD g_PortClsUnload = nullptr;

_Dispatch_type_(IRP_MJ_DEVICE_CONTROL)
DRIVER_DISPATCH PulseDeviceControl;
_Dispatch_type_(IRP_MJ_CLEANUP)
DRIVER_DISPATCH PulseCleanup;
_Dispatch_type_(IRP_MJ_PNP)
DRIVER_DISPATCH PulsePnp;

class PulseAdapter final : public IUnknown, public CUnknown
{
public:
    explicit PulseAdapter(_In_opt_ PUNKNOWN OuterUnknown)
        : CUnknown(OuterUnknown)
    {
    }

    DECLARE_STD_UNKNOWN();
};

STDMETHODIMP PulseAdapter::NonDelegatingQueryInterface(REFIID Interface, PVOID* Object)
{
    if (Object == nullptr)
    {
        return STATUS_INVALID_PARAMETER;
    }
    if (IsEqualGUIDAligned(Interface, IID_IUnknown))
    {
        *Object = static_cast<PUNKNOWN>(this);
        PUNKNOWN(*Object)->AddRef();
        return STATUS_SUCCESS;
    }
    *Object = nullptr;
    return STATUS_INVALID_PARAMETER;
}

NTSTATUS CompleteIrp(_In_ PIRP Irp, _In_ NTSTATUS Status, _In_ ULONG_PTR Information)
{
    Irp->IoStatus.Status = Status;
    Irp->IoStatus.Information = Information;
    IoCompleteRequest(Irp, IO_NO_INCREMENT);
    return Status;
}

NTSTATUS InstallSubdevice(
    _In_ PDEVICE_OBJECT DeviceObject,
    _In_ PIRP Irp,
    _In_ PRESOURCELIST ResourceList,
    _In_ PUNKNOWN Adapter,
    _In_ REFGUID PortClassId,
    _In_ PCWSTR Name,
    _In_ bool Wave,
    _Out_ PUNKNOWN* PortUnknown)
{
    if (PortUnknown == nullptr)
    {
        return STATUS_INVALID_PARAMETER;
    }
    *PortUnknown = nullptr;
    PPORT port = nullptr;
    PUNKNOWN miniport = nullptr;
    NTSTATUS status = PcNewPort(&port, PortClassId);
    if (NT_SUCCESS(status))
    {
        status = Wave ? PulseCreateWaveMiniport(&miniport) : PulseCreateTopologyMiniport(&miniport);
    }
    if (NT_SUCCESS(status))
    {
        status = port->Init(DeviceObject, Irp, miniport, Adapter, ResourceList);
    }
    if (NT_SUCCESS(status))
    {
        status = PcRegisterSubdevice(DeviceObject, const_cast<PWSTR>(Name), port);
    }
    if (NT_SUCCESS(status))
    {
        status = port->QueryInterface(IID_IUnknown, reinterpret_cast<PVOID*>(PortUnknown));
    }
    if (miniport != nullptr)
    {
        miniport->Release();
    }
    if (port != nullptr)
    {
        port->Release();
    }
    return status;
}

_Use_decl_annotations_
NTSTATUS PulseDeviceControl(PDEVICE_OBJECT DeviceObject, PIRP Irp)
{
    PIO_STACK_LOCATION stack = IoGetCurrentIrpStackLocation(Irp);
    const ULONG code = stack->Parameters.DeviceIoControl.IoControlCode;
    const ULONG inputLength = stack->Parameters.DeviceIoControl.InputBufferLength;
    const ULONG outputLength = stack->Parameters.DeviceIoControl.OutputBufferLength;

    if (code == IOCTL_PULSE_GET_VERSION)
    {
        if (outputLength < sizeof(PULSE_VERSION_REPLY) || Irp->AssociatedIrp.SystemBuffer == nullptr)
        {
            return CompleteIrp(Irp, STATUS_BUFFER_TOO_SMALL, 0);
        }
        auto reply = static_cast<PULSE_VERSION_REPLY*>(Irp->AssociatedIrp.SystemBuffer);
        *reply = { PULSE_PROTOCOL_VERSION, PULSE_PACKET_SAMPLES, PULSE_SAMPLE_RATE,
                   PULSE_RING_SAMPLES, 0 };
        return CompleteIrp(Irp, STATUS_SUCCESS, sizeof(*reply));
    }

    if (code == IOCTL_PULSE_GET_CONSUMER_STATE)
    {
        if (outputLength < sizeof(PULSE_CONSUMER_STATE_REPLY) || Irp->AssociatedIrp.SystemBuffer == nullptr)
        {
            return CompleteIrp(Irp, STATUS_BUFFER_TOO_SMALL, 0);
        }
        auto reply = static_cast<PULSE_CONSUMER_STATE_REPLY*>(Irp->AssociatedIrp.SystemBuffer);
        g_PulseAudioRing.GetConsumerState(reply);
        return CompleteIrp(Irp, STATUS_SUCCESS, sizeof(*reply));
    }

    if (code == IOCTL_PULSE_RESET_AUDIO)
    {
        if (inputLength != sizeof(PULSE_RESET_REQUEST) || Irp->AssociatedIrp.SystemBuffer == nullptr)
        {
            return CompleteIrp(Irp, STATUS_INFO_LENGTH_MISMATCH, 0);
        }
        const auto request = static_cast<const PULSE_RESET_REQUEST*>(Irp->AssociatedIrp.SystemBuffer);
        if (request->ProtocolVersion != PULSE_PROTOCOL_VERSION || request->Reserved != 0)
        {
            return CompleteIrp(Irp, STATUS_INVALID_PARAMETER, 0);
        }
        NTSTATUS status = g_PulseAudioRing.AcquireWriter(stack->FileObject);
        if (NT_SUCCESS(status))
        {
            status = g_PulseAudioRing.Reset(stack->FileObject);
        }
        return CompleteIrp(Irp, status, 0);
    }

    if (code == IOCTL_PULSE_WRITE_AUDIO)
    {
        if (inputLength != sizeof(PULSE_AUDIO_PACKET) || Irp->AssociatedIrp.SystemBuffer == nullptr)
        {
            return CompleteIrp(Irp, STATUS_INFO_LENGTH_MISMATCH, 0);
        }
        const auto packet = static_cast<const PULSE_AUDIO_PACKET*>(Irp->AssociatedIrp.SystemBuffer);
        if (packet->ProtocolVersion != PULSE_PROTOCOL_VERSION ||
            packet->SampleCount != PULSE_PACKET_SAMPLES)
        {
            return CompleteIrp(Irp, STATUS_INVALID_PARAMETER, 0);
        }

        NTSTATUS status = g_PulseAudioRing.AcquireWriter(stack->FileObject);
        if (NT_SUCCESS(status))
        {
            status = g_PulseAudioRing.Write(stack->FileObject, packet);
        }
        return CompleteIrp(Irp, status, 0);
    }

    return g_PortClsDeviceControl(DeviceObject, Irp);
}

_Use_decl_annotations_
NTSTATUS PulseCleanup(PDEVICE_OBJECT DeviceObject, PIRP Irp)
{
    PIO_STACK_LOCATION stack = IoGetCurrentIrpStackLocation(Irp);
    g_PulseAudioRing.ReleaseWriter(stack->FileObject);
    return g_PortClsCleanup(DeviceObject, Irp);
}

_Use_decl_annotations_
NTSTATUS PulsePnp(PDEVICE_OBJECT DeviceObject, PIRP Irp)
{
    const UCHAR minor = IoGetCurrentIrpStackLocation(Irp)->MinorFunction;
    if (minor == IRP_MN_STOP_DEVICE || minor == IRP_MN_SURPRISE_REMOVAL || minor == IRP_MN_REMOVE_DEVICE)
    {
        if (g_PulseControlInterface.Buffer != nullptr)
        {
            IoSetDeviceInterfaceState(&g_PulseControlInterface, FALSE);
        }
        g_PulseAudioRing.ResetAll();
    }
    return g_PortClsPnp(DeviceObject, Irp);
}

void PulseUnload(_In_ PDRIVER_OBJECT DriverObject)
{
    g_PulseAudioRing.ResetAll();
    if (g_PulseControlInterface.Buffer != nullptr)
    {
        IoSetDeviceInterfaceState(&g_PulseControlInterface, FALSE);
        RtlFreeUnicodeString(&g_PulseControlInterface);
        RtlZeroMemory(&g_PulseControlInterface, sizeof(g_PulseControlInterface));
    }
    if (g_PortClsUnload != nullptr)
    {
        g_PortClsUnload(DriverObject);
    }
}
}

#pragma code_seg("INIT")
extern "C" NTSTATUS DriverEntry(PDRIVER_OBJECT DriverObject, PUNICODE_STRING RegistryPath)
{
    g_PulseAudioRing.Initialize();
    NTSTATUS status = PcInitializeAdapterDriver(DriverObject, RegistryPath, PulseAddDevice);
    if (!NT_SUCCESS(status))
    {
        return status;
    }

    g_PortClsDeviceControl = DriverObject->MajorFunction[IRP_MJ_DEVICE_CONTROL];
    g_PortClsCleanup = DriverObject->MajorFunction[IRP_MJ_CLEANUP];
    g_PortClsPnp = DriverObject->MajorFunction[IRP_MJ_PNP];
    g_PortClsUnload = DriverObject->DriverUnload;
    DriverObject->MajorFunction[IRP_MJ_DEVICE_CONTROL] = PulseDeviceControl;
    DriverObject->MajorFunction[IRP_MJ_CLEANUP] = PulseCleanup;
    DriverObject->MajorFunction[IRP_MJ_PNP] = PulsePnp;
    DriverObject->DriverUnload = PulseUnload;
    return STATUS_SUCCESS;
}

extern "C" NTSTATUS PulseAddDevice(PDRIVER_OBJECT DriverObject, PDEVICE_OBJECT PhysicalDeviceObject)
{
    NTSTATUS status = IoRegisterDeviceInterface(
        PhysicalDeviceObject,
        &GUID_DEVINTERFACE_PULSE_VIRTUAL_MIC,
        nullptr,
        &g_PulseControlInterface);
    if (!NT_SUCCESS(status))
    {
        return status;
    }

    status = PcAddAdapterDevice(DriverObject, PhysicalDeviceObject, PulseStartDevice, 2, 0);
    if (!NT_SUCCESS(status))
    {
        RtlFreeUnicodeString(&g_PulseControlInterface);
        RtlZeroMemory(&g_PulseControlInterface, sizeof(g_PulseControlInterface));
    }
    return status;
}

extern "C" NTSTATUS PulseStartDevice(PDEVICE_OBJECT DeviceObject, PIRP Irp, PRESOURCELIST ResourceList)
{
    auto adapter = new (POOL_FLAG_NON_PAGED, PULSE_POOL_TAG) PulseAdapter(nullptr);
    if (adapter == nullptr)
    {
        return STATUS_INSUFFICIENT_RESOURCES;
    }
    adapter->AddRef();

    PUNKNOWN topologyPort = nullptr;
    PUNKNOWN wavePort = nullptr;

    NTSTATUS status = InstallSubdevice(
        DeviceObject, Irp, ResourceList, adapter, CLSID_PortTopology, PULSE_TOPOLOGY_NAME, false,
        &topologyPort);
    if (NT_SUCCESS(status))
    {
        status = InstallSubdevice(
            DeviceObject, Irp, ResourceList, adapter, CLSID_PortWaveRT, PULSE_WAVE_NAME, true,
            &wavePort);
    }
    if (NT_SUCCESS(status))
    {
        status = PcRegisterPhysicalConnection(DeviceObject, topologyPort, 1, wavePort, 0);
    }
    if (NT_SUCCESS(status))
    {
        status = IoSetDeviceInterfaceState(&g_PulseControlInterface, TRUE);
    }
    if (wavePort != nullptr)
    {
        wavePort->Release();
    }
    if (topologyPort != nullptr)
    {
        topologyPort->Release();
    }
    adapter->Release();
    return status;
}

#pragma code_seg()
