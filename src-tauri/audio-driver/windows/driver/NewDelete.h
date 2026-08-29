#pragma once

void* __cdecl operator new(size_t Size, POOL_FLAGS PoolFlags, ULONG Tag);
void __cdecl operator delete(void* Allocation, size_t Size);
void __cdecl operator delete(void* Allocation, POOL_FLAGS PoolFlags, ULONG Tag);

