use std::{ffi::c_void, mem};

use windows::{
    core::{GUID, PCWSTR},
    Win32::{
        Devices::DeviceAndDriverInstallation::{
            SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW,
            SetupDiGetDeviceInterfaceDetailW, DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, HDEVINFO,
            SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W,
        },
        Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE},
        Storage::FileSystem::{
            CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        },
        System::IO::DeviceIoControl,
    },
};

pub(crate) const PROTOCOL_VERSION: u32 = 1;
pub(crate) const SAMPLE_RATE: u32 = 48_000;
pub(crate) const PACKET_SAMPLES: usize = 480;
const AUDIO_PACKET_BYTES: usize = 12 + PACKET_SAMPLES * mem::size_of::<i16>();
const VERSION_RESPONSE_BYTES: usize = 20;
const CONSUMER_STATE_BYTES: usize = 16;

// {DE117D34-8BF5-476A-962E-06D5E5BE5616}
pub(crate) const GUID_DEVINTERFACE_PULSE_VIRTUAL_MIC: GUID = GUID::from_values(
    0xde11_7d34,
    0x8bf5,
    0x476a,
    [0x96, 0x2e, 0x06, 0xd5, 0xe5, 0xbe, 0x56, 0x16],
);

const FILE_DEVICE_UNKNOWN: u32 = 0x22;
const METHOD_BUFFERED: u32 = 0;
const FILE_READ_ACCESS: u32 = 0x0001;
const FILE_WRITE_ACCESS: u32 = 0x0002;

const fn ctl_code(function: u32, access: u32) -> u32 {
    (FILE_DEVICE_UNKNOWN << 16) | (access << 14) | (function << 2) | METHOD_BUFFERED
}

const IOCTL_GET_VERSION: u32 = ctl_code(0x800, FILE_READ_ACCESS);
const IOCTL_GET_CONSUMER_STATE: u32 = ctl_code(0x801, FILE_READ_ACCESS);
const IOCTL_RESET_AUDIO: u32 = ctl_code(0x802, FILE_READ_ACCESS | FILE_WRITE_ACCESS);
const IOCTL_WRITE_AUDIO: u32 = ctl_code(0x803, FILE_WRITE_ACCESS);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DriverVersion {
    pub(crate) protocol_version: u32,
    pub(crate) packet_samples: u32,
    pub(crate) sample_rate: u32,
    pub(crate) ring_capacity_samples: u32,
    pub(crate) flags: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ConsumerState {
    pub(crate) protocol_version: u32,
    pub(crate) active_capture_streams: u32,
    pub(crate) state_generation: u64,
}

impl ConsumerState {
    pub(crate) fn is_active(self) -> bool {
        self.active_capture_streams != 0
    }
}

pub(crate) struct PulseDriver {
    handle: HANDLE,
}

unsafe impl Send for PulseDriver {}

impl PulseDriver {
    pub(crate) fn open_monitor() -> Result<Self, String> {
        Self::open(false)
    }

    pub(crate) fn open_writer() -> Result<Self, String> {
        let driver = Self::open(true)?;
        let version = driver.version()?;
        if version.protocol_version != PROTOCOL_VERSION
            || version.packet_samples != PACKET_SAMPLES as u32
            || version.sample_rate != SAMPLE_RATE
        {
            return Err(format!(
                "Pulse virtual microphone protocol mismatch: driver v{} at {} Hz with {} samples, app v{} at {} Hz with {} samples",
                version.protocol_version,
                version.sample_rate,
                version.packet_samples,
                PROTOCOL_VERSION,
                SAMPLE_RATE,
                PACKET_SAMPLES
            ));
        }
        driver.reset()?;
        Ok(driver)
    }

    fn open(writer: bool) -> Result<Self, String> {
        let path = interface_path()?;
        let access = if writer {
            GENERIC_READ.0 | GENERIC_WRITE.0
        } else {
            GENERIC_READ.0
        };
        let handle = unsafe {
            CreateFileW(
                PCWSTR(path.as_ptr()),
                access,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        }
        .map_err(|error| format!("could not open the Pulse virtual microphone: {error}"))?;
        Ok(Self { handle })
    }

    pub(crate) fn version(&self) -> Result<DriverVersion, String> {
        let mut response = [0_u8; VERSION_RESPONSE_BYTES];
        self.ioctl(IOCTL_GET_VERSION, &[], &mut response)?;
        parse_version(&response)
    }

    pub(crate) fn consumer_state(&self) -> Result<ConsumerState, String> {
        let mut response = [0_u8; CONSUMER_STATE_BYTES];
        self.ioctl(IOCTL_GET_CONSUMER_STATE, &[], &mut response)?;
        parse_consumer_state(&response)
    }

    pub(crate) fn reset(&self) -> Result<(), String> {
        let request = [PROTOCOL_VERSION.to_le_bytes(), 0_u32.to_le_bytes()].concat();
        self.ioctl(IOCTL_RESET_AUDIO, &request, &mut [])
    }

    pub(crate) fn write_audio(&self, sequence: u32, samples: &[i16]) -> Result<(), String> {
        let packet = encode_audio_packet(sequence, samples)?;
        self.ioctl(IOCTL_WRITE_AUDIO, &packet, &mut [])
    }

    fn ioctl(&self, code: u32, input: &[u8], output: &mut [u8]) -> Result<(), String> {
        let mut returned = 0_u32;
        unsafe {
            DeviceIoControl(
                self.handle,
                code,
                (!input.is_empty()).then_some(input.as_ptr().cast::<c_void>()),
                input.len() as u32,
                (!output.is_empty()).then_some(output.as_mut_ptr().cast::<c_void>()),
                output.len() as u32,
                Some(&mut returned),
                None,
            )
        }
        .map_err(|error| format!("Pulse virtual microphone transport failed: {error}"))?;
        if returned as usize != output.len() {
            return Err(format!(
                "Pulse virtual microphone returned {returned} bytes, expected {}",
                output.len()
            ));
        }
        Ok(())
    }
}

impl Drop for PulseDriver {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

pub(crate) fn interface_available() -> bool {
    interface_path().is_ok()
}

fn interface_path() -> Result<Vec<u16>, String> {
    let devices = unsafe {
        SetupDiGetClassDevsW(
            Some(&GUID_DEVINTERFACE_PULSE_VIRTUAL_MIC),
            PCWSTR::null(),
            None,
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        )
    }
    .map_err(|error| format!("could not find the Pulse virtual microphone: {error}"))?;
    let result = interface_path_from_set(devices);
    unsafe {
        let _ = SetupDiDestroyDeviceInfoList(devices);
    }
    result
}

fn interface_path_from_set(devices: HDEVINFO) -> Result<Vec<u16>, String> {
    let mut interface = SP_DEVICE_INTERFACE_DATA {
        cbSize: mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
        ..Default::default()
    };
    unsafe {
        SetupDiEnumDeviceInterfaces(
            devices,
            None,
            &GUID_DEVINTERFACE_PULSE_VIRTUAL_MIC,
            0,
            &mut interface,
        )
    }
    .map_err(|error| format!("the Pulse virtual microphone interface is unavailable: {error}"))?;

    let mut required = 0_u32;
    let _ = unsafe {
        SetupDiGetDeviceInterfaceDetailW(devices, &interface, None, 0, Some(&mut required), None)
    };
    if required < mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32 {
        return Err("Windows returned an invalid Pulse device-interface path".to_string());
    }

    let mut storage = vec![0_u8; required as usize];
    let detail = storage
        .as_mut_ptr()
        .cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
    unsafe {
        (*detail).cbSize = mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
        SetupDiGetDeviceInterfaceDetailW(devices, &interface, Some(detail), required, None, None)
    }
    .map_err(|error| format!("could not read the Pulse device-interface path: {error}"))?;

    let path = unsafe { PCWSTR((*detail).DevicePath.as_ptr()).to_string() }
        .map_err(|error| format!("Pulse returned an invalid device-interface path: {error}"))?;
    Ok(path.encode_utf16().chain(Some(0)).collect())
}

pub(crate) fn encode_audio_packet(sequence: u32, samples: &[i16]) -> Result<Vec<u8>, String> {
    if samples.len() != PACKET_SAMPLES {
        return Err(format!(
            "Pulse audio packets require exactly {PACKET_SAMPLES} samples, received {}",
            samples.len()
        ));
    }
    let mut packet = Vec::with_capacity(AUDIO_PACKET_BYTES);
    packet.extend_from_slice(&PROTOCOL_VERSION.to_le_bytes());
    packet.extend_from_slice(&sequence.to_le_bytes());
    packet.extend_from_slice(&(samples.len() as u32).to_le_bytes());
    for sample in samples {
        packet.extend_from_slice(&sample.to_le_bytes());
    }
    debug_assert_eq!(packet.len(), AUDIO_PACKET_BYTES);
    Ok(packet)
}

fn parse_version(bytes: &[u8]) -> Result<DriverVersion, String> {
    if bytes.len() != VERSION_RESPONSE_BYTES {
        return Err(
            "the Pulse virtual microphone returned an invalid version response".to_string(),
        );
    }
    Ok(DriverVersion {
        protocol_version: read_u32(bytes, 0),
        packet_samples: read_u32(bytes, 4),
        sample_rate: read_u32(bytes, 8),
        ring_capacity_samples: read_u32(bytes, 12),
        flags: read_u32(bytes, 16),
    })
}

pub(crate) fn parse_consumer_state(bytes: &[u8]) -> Result<ConsumerState, String> {
    if bytes.len() != CONSUMER_STATE_BYTES {
        return Err(
            "the Pulse virtual microphone returned an invalid consumer-state response".to_string(),
        );
    }
    let state = ConsumerState {
        protocol_version: read_u32(bytes, 0),
        active_capture_streams: read_u32(bytes, 4),
        state_generation: read_u64(bytes, 8),
    };
    if state.protocol_version != PROTOCOL_VERSION {
        return Err(format!(
            "Pulse virtual microphone protocol mismatch: driver v{}, app v{}",
            state.protocol_version, PROTOCOL_VERSION
        ));
    }
    Ok(state)
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("u32 field"))
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().expect("u64 field"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_encoding_is_fixed_size_little_endian_pcm16() {
        let mut samples = [0_i16; PACKET_SAMPLES];
        samples[0] = i16::MIN;
        samples[1] = 0x1234;
        samples[PACKET_SAMPLES - 1] = i16::MAX;

        let packet = encode_audio_packet(0x89ab_cdef, &samples).expect("valid packet");

        assert_eq!(packet.len(), AUDIO_PACKET_BYTES);
        assert_eq!(&packet[0..4], &PROTOCOL_VERSION.to_le_bytes());
        assert_eq!(&packet[4..8], &0x89ab_cdef_u32.to_le_bytes());
        assert_eq!(&packet[8..12], &(PACKET_SAMPLES as u32).to_le_bytes());
        assert_eq!(&packet[12..14], &i16::MIN.to_le_bytes());
        assert_eq!(&packet[14..16], &0x1234_i16.to_le_bytes());
        assert_eq!(&packet[AUDIO_PACKET_BYTES - 2..], &i16::MAX.to_le_bytes());
    }

    #[test]
    fn packet_encoding_rejects_partial_periods() {
        assert!(encode_audio_packet(0, &[0; PACKET_SAMPLES - 1]).is_err());
        assert!(encode_audio_packet(0, &[0; PACKET_SAMPLES + 1]).is_err());
    }

    #[test]
    fn consumer_state_parses_active_count_and_generation() {
        let mut bytes = [0_u8; CONSUMER_STATE_BYTES];
        bytes[0..4].copy_from_slice(&PROTOCOL_VERSION.to_le_bytes());
        bytes[4..8].copy_from_slice(&3_u32.to_le_bytes());
        bytes[8..16].copy_from_slice(&0x0102_0304_0506_0708_u64.to_le_bytes());

        let state = parse_consumer_state(&bytes).expect("valid state");

        assert!(state.is_active());
        assert_eq!(state.active_capture_streams, 3);
        assert_eq!(state.state_generation, 0x0102_0304_0506_0708);
    }

    #[test]
    fn consumer_state_rejects_unknown_protocol_versions() {
        let mut bytes = [0_u8; CONSUMER_STATE_BYTES];
        bytes[0..4].copy_from_slice(&(PROTOCOL_VERSION + 1).to_le_bytes());
        assert!(parse_consumer_state(&bytes).is_err());
    }

    #[test]
    #[ignore = "requires an installed test-signed PulseVirtualMic driver"]
    fn driver_writer_ownership_hands_off_after_handle_close() {
        let first = PulseDriver::open_writer().expect("first writer");
        assert!(
            PulseDriver::open_writer().is_err(),
            "the driver must reject a second producer"
        );
        drop(first);
        let recovered = PulseDriver::open_writer().expect("writer after owner close");
        recovered.reset().expect("reset after writer handoff");
    }
}
