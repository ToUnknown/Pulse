#pragma once

#include "PulseVirtualMic.h"

class PulseTopologyMiniport final : public IMiniportTopology, public CUnknown
{
public:
    explicit PulseTopologyMiniport(_In_opt_ PUNKNOWN OuterUnknown)
        : CUnknown(OuterUnknown)
    {
    }

    DECLARE_STD_UNKNOWN();
    IMP_IMiniportTopology;
};

