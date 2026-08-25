#include "PulseVirtualMic.h"
#include "NewDelete.h"

void* __cdecl operator new(size_t Size, POOL_FLAGS PoolFlags, ULONG Tag)
{
    return ExAllocatePool2(PoolFlags, Size, Tag);
}

void __cdecl operator delete(void* Allocation)
{
    if (Allocation != nullptr)
    {
        ExFreePoolWithTag(Allocation, PULSE_POOL_TAG);
    }
}

void __cdecl operator delete(void* Allocation, size_t Size)
{
    UNREFERENCED_PARAMETER(Size);
    operator delete(Allocation);
}

void __cdecl operator delete(void* Allocation, POOL_FLAGS PoolFlags, ULONG Tag)
{
    UNREFERENCED_PARAMETER(PoolFlags);
    if (Allocation != nullptr)
    {
        ExFreePoolWithTag(Allocation, Tag);
    }
}

