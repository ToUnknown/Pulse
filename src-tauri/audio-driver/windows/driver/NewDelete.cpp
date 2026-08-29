#include "PulseVirtualMic.h"
#include "NewDelete.h"

void* __cdecl operator new(size_t Size, POOL_FLAGS PoolFlags, ULONG Tag)
{
    void* allocation = ExAllocatePool2(PoolFlags, Size, Tag);
    if (allocation != nullptr)
    {
        RtlZeroMemory(allocation, Size);
    }
    return allocation;
}

void __cdecl operator delete(void* Allocation, size_t Size)
{
    UNREFERENCED_PARAMETER(Size);
    if (Allocation != nullptr)
    {
        ExFreePool(Allocation);
    }
}

void __cdecl operator delete(void* Allocation, POOL_FLAGS PoolFlags, ULONG Tag)
{
    UNREFERENCED_PARAMETER(PoolFlags);
    if (Allocation != nullptr)
    {
        ExFreePoolWithTag(Allocation, Tag);
    }
}

