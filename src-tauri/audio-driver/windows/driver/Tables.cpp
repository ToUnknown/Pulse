#include "PulseVirtualMic.h"

namespace
{
KSDATAFORMAT_WAVEFORMATEXTENSIBLE PulseNativeFormat =
{
    {
        sizeof(KSDATAFORMAT_WAVEFORMATEXTENSIBLE),
        0,
        0,
        0,
        STATICGUIDOF(KSDATAFORMAT_TYPE_AUDIO),
        STATICGUIDOF(KSDATAFORMAT_SUBTYPE_PCM),
        STATICGUIDOF(KSDATAFORMAT_SPECIFIER_WAVEFORMATEX)
    },
    {
        {
            WAVE_FORMAT_EXTENSIBLE,
            1,
            PULSE_SAMPLE_RATE,
            PULSE_SAMPLE_RATE * sizeof(INT16),
            sizeof(INT16),
            16,
            sizeof(WAVEFORMATEXTENSIBLE) - sizeof(WAVEFORMATEX)
        },
        16,
        KSAUDIO_SPEAKER_MONO,
        STATICGUIDOF(KSDATAFORMAT_SUBTYPE_PCM)
    }
};

KSDATARANGE_AUDIO PulseCaptureRange =
{
    {
        sizeof(KSDATARANGE_AUDIO),
        0,
        0,
        0,
        STATICGUIDOF(KSDATAFORMAT_TYPE_AUDIO),
        STATICGUIDOF(KSDATAFORMAT_SUBTYPE_PCM),
        STATICGUIDOF(KSDATAFORMAT_SPECIFIER_WAVEFORMATEX)
    },
    1,
    16,
    16,
    PULSE_SAMPLE_RATE,
    PULSE_SAMPLE_RATE
};

KSDATARANGE PulseAnalogRange =
{
    sizeof(KSDATARANGE),
    0,
    0,
    0,
    STATICGUIDOF(KSDATAFORMAT_TYPE_AUDIO),
    STATICGUIDOF(KSDATAFORMAT_SUBTYPE_ANALOG),
    STATICGUIDOF(KSDATAFORMAT_SPECIFIER_NONE)
};

PKSDATARANGE PulseCaptureRanges[] = { reinterpret_cast<PKSDATARANGE>(&PulseCaptureRange) };
PKSDATARANGE PulseAnalogRanges[] = { &PulseAnalogRange };

PCPIN_DESCRIPTOR PulseWavePins[] =
{
    {
        0, 0, 0, nullptr,
        {
            0, nullptr,
            0, nullptr,
            ARRAYSIZE(PulseAnalogRanges), PulseAnalogRanges,
            KSPIN_DATAFLOW_IN,
            KSPIN_COMMUNICATION_NONE,
            &KSCATEGORY_AUDIO,
            nullptr,
            0
        }
    },
    {
        8, 8, 0, nullptr,
        {
            0, nullptr,
            0, nullptr,
            ARRAYSIZE(PulseCaptureRanges), PulseCaptureRanges,
            KSPIN_DATAFLOW_OUT,
            KSPIN_COMMUNICATION_SINK,
            &KSCATEGORY_AUDIO,
            &KSAUDFNAME_RECORDING_CONTROL,
            0
        }
    }
};

PCNODE_DESCRIPTOR PulseWaveNodes[] =
{
    { 0, nullptr, &KSNODETYPE_ADC, nullptr }
};

PCCONNECTION_DESCRIPTOR PulseWaveConnections[] =
{
    { PCFILTER_NODE, 0, 0, 1 },
    { 0, 0, PCFILTER_NODE, 1 }
};

PCFILTER_DESCRIPTOR PulseWaveFilter =
{
    0,
    nullptr,
    sizeof(PCPIN_DESCRIPTOR),
    ARRAYSIZE(PulseWavePins),
    PulseWavePins,
    sizeof(PCNODE_DESCRIPTOR),
    ARRAYSIZE(PulseWaveNodes),
    PulseWaveNodes,
    ARRAYSIZE(PulseWaveConnections),
    PulseWaveConnections,
    0,
    nullptr
};

PCPIN_DESCRIPTOR PulseTopologyPins[] =
{
    {
        0, 0, 0, nullptr,
        {
            0, nullptr,
            0, nullptr,
            ARRAYSIZE(PulseAnalogRanges), PulseAnalogRanges,
            KSPIN_DATAFLOW_IN,
            KSPIN_COMMUNICATION_NONE,
            &KSNODETYPE_MICROPHONE,
            nullptr,
            0
        }
    },
    {
        0, 0, 0, nullptr,
        {
            0, nullptr,
            0, nullptr,
            ARRAYSIZE(PulseAnalogRanges), PulseAnalogRanges,
            KSPIN_DATAFLOW_OUT,
            KSPIN_COMMUNICATION_NONE,
            &KSCATEGORY_AUDIO,
            nullptr,
            0
        }
    }
};

PCCONNECTION_DESCRIPTOR PulseTopologyConnections[] =
{
    { PCFILTER_NODE, 0, PCFILTER_NODE, 1 }
};

PCFILTER_DESCRIPTOR PulseTopologyFilter =
{
    0,
    nullptr,
    sizeof(PCPIN_DESCRIPTOR),
    ARRAYSIZE(PulseTopologyPins),
    PulseTopologyPins,
    0,
    0,
    nullptr,
    ARRAYSIZE(PulseTopologyConnections),
    PulseTopologyConnections,
    0,
    nullptr
};
}

PCFILTER_DESCRIPTOR g_PulseWaveFilterDescriptor = &PulseWaveFilter;
PCFILTER_DESCRIPTOR g_PulseTopologyFilterDescriptor = &PulseTopologyFilter;

extern "C" PKSDATAFORMAT_WAVEFORMATEXTENSIBLE PulseGetNativeFormat()
{
    return &PulseNativeFormat;
}

