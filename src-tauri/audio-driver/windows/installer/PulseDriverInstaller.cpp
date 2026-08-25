#include <windows.h>
#include <initguid.h>
#include <cfgmgr32.h>
#include <devguid.h>
#include <devpkey.h>
#include <newdev.h>
#include <setupapi.h>

#include <filesystem>
#include <fstream>
#include <iostream>
#include <optional>
#include <string>
#include <vector>

namespace
{
constexpr wchar_t PulseHardwareId[] = L"ROOT\\PulseVirtualMic";
constexpr wchar_t PulseService[] = L"PulseVirtualMic";
constexpr wchar_t LegacyHardwareId[] = L"VBAudioVACWDM";
constexpr wchar_t LegacyService[] = L"VBAudioVACMME";

struct Result
{
    bool Ok = false;
    bool RestartRequired = false;
    DWORD ErrorCode = ERROR_SUCCESS;
    bool DevicePresent = false;
    std::wstring InfName;
    std::wstring Message;
};

std::wstring WindowsMessage(DWORD error)
{
    wchar_t* buffer = nullptr;
    const DWORD length = FormatMessageW(
        FORMAT_MESSAGE_ALLOCATE_BUFFER | FORMAT_MESSAGE_FROM_SYSTEM | FORMAT_MESSAGE_IGNORE_INSERTS,
        nullptr,
        error,
        0,
        reinterpret_cast<wchar_t*>(&buffer),
        0,
        nullptr);
    std::wstring message = length == 0 ? L"Windows error " + std::to_wstring(error) : std::wstring(buffer, length);
    if (buffer != nullptr)
    {
        LocalFree(buffer);
    }
    while (!message.empty() && (message.back() == L'\r' || message.back() == L'\n' || message.back() == L' '))
    {
        message.pop_back();
    }
    return message;
}

std::string Utf8(const std::wstring& value)
{
    if (value.empty())
    {
        return {};
    }
    const int length = WideCharToMultiByte(CP_UTF8, 0, value.data(), static_cast<int>(value.size()), nullptr, 0, nullptr, nullptr);
    std::string output(static_cast<size_t>(length), '\0');
    WideCharToMultiByte(CP_UTF8, 0, value.data(), static_cast<int>(value.size()), output.data(), length, nullptr, nullptr);
    return output;
}

std::string JsonEscape(const std::wstring& value)
{
    std::string output;
    for (const unsigned char byte : Utf8(value))
    {
        switch (byte)
        {
        case '\\': output += "\\\\"; break;
        case '"': output += "\\\""; break;
        case '\b': output += "\\b"; break;
        case '\f': output += "\\f"; break;
        case '\n': output += "\\n"; break;
        case '\r': output += "\\r"; break;
        case '\t': output += "\\t"; break;
        default:
            if (byte < 0x20)
            {
                char escaped[7] = {};
                sprintf_s(escaped, "\\u%04x", byte);
                output += escaped;
            }
            else
            {
                output.push_back(static_cast<char>(byte));
            }
            break;
        }
    }
    return output;
}

std::string ToJson(const Result& result)
{
    return std::string("{\"ok\":") + (result.Ok ? "true" : "false") +
        ",\"restartRequired\":" + (result.RestartRequired ? "true" : "false") +
        ",\"errorCode\":" + std::to_string(result.ErrorCode) +
        ",\"devicePresent\":" + (result.DevicePresent ? "true" : "false") +
        ",\"infName\":\"" + JsonEscape(result.InfName) +
        "\",\"message\":\"" + JsonEscape(result.Message) + "\"}";
}

std::optional<std::wstring> DeviceProperty(
    HDEVINFO devices,
    SP_DEVINFO_DATA& device,
    DWORD property)
{
    DWORD type = 0;
    DWORD bytes = 0;
    SetupDiGetDeviceRegistryPropertyW(devices, &device, property, &type, nullptr, 0, &bytes);
    if (GetLastError() != ERROR_INSUFFICIENT_BUFFER || bytes == 0)
    {
        return std::nullopt;
    }
    std::vector<BYTE> buffer(bytes + sizeof(wchar_t), 0);
    if (!SetupDiGetDeviceRegistryPropertyW(devices, &device, property, &type, buffer.data(), bytes, nullptr))
    {
        return std::nullopt;
    }
    return std::wstring(reinterpret_cast<const wchar_t*>(buffer.data()));
}

bool MultiStringContains(const std::wstring& values, const std::wstring& expected)
{
    const wchar_t* current = values.data();
    const wchar_t* end = values.data() + values.size();
    while (current < end && *current != L'\0')
    {
        if (_wcsicmp(current, expected.c_str()) == 0)
        {
            return true;
        }
        current += wcslen(current) + 1;
    }
    return false;
}

bool MatchesDevice(
    HDEVINFO devices,
    SP_DEVINFO_DATA& device,
    const wchar_t* hardwareId,
    const wchar_t* service)
{
    const auto ids = DeviceProperty(devices, device, SPDRP_HARDWAREID);
    const auto installedService = DeviceProperty(devices, device, SPDRP_SERVICE);
    return ids.has_value() && installedService.has_value() &&
        MultiStringContains(*ids, hardwareId) && _wcsicmp(installedService->c_str(), service) == 0;
}

bool MatchesHardwareId(HDEVINFO devices, SP_DEVINFO_DATA& device, const wchar_t* hardwareId)
{
    const auto ids = DeviceProperty(devices, device, SPDRP_HARDWAREID);
    return ids.has_value() && MultiStringContains(*ids, hardwareId);
}

std::wstring InfProvider(const std::filesystem::path& path)
{
    HINF inf = SetupOpenInfFileW(path.c_str(), nullptr, INF_STYLE_WIN4, nullptr);
    if (inf == INVALID_HANDLE_VALUE)
    {
        return {};
    }
    INFCONTEXT context = {};
    wchar_t provider[LINE_LEN] = {};
    DWORD required = 0;
    const bool read = SetupFindFirstLineW(inf, L"Version", L"Provider", &context) &&
        SetupGetStringFieldW(&context, 1, provider, ARRAYSIZE(provider), &required);
    SetupCloseInfFile(inf);
    return read ? provider : L"";
}

std::wstring FindPulsePublishedInf()
{
    DWORD required = 0;
    SetupGetInfFileListW(nullptr, INF_STYLE_WIN4, nullptr, 0, &required);
    if (required == 0)
    {
        return {};
    }
    std::vector<wchar_t> names(required, L'\0');
    if (!SetupGetInfFileListW(nullptr, INF_STYLE_WIN4, names.data(), required, nullptr))
    {
        return {};
    }

    wchar_t windowsDirectory[MAX_PATH] = {};
    if (GetWindowsDirectoryW(windowsDirectory, ARRAYSIZE(windowsDirectory)) == 0)
    {
        return {};
    }
    for (const wchar_t* name = names.data(); *name != L'\0'; name += wcslen(name) + 1)
    {
        if (_wcsnicmp(name, L"oem", 3) != 0)
        {
            continue;
        }
        const std::filesystem::path path = std::filesystem::path(windowsDirectory) / L"INF" / name;
        DWORD bytes = 0;
        SetupGetInfInformationW(path.c_str(), INFINFO_INF_NAME_IS_ABSOLUTE, nullptr, 0, &bytes);
        if (GetLastError() != ERROR_INSUFFICIENT_BUFFER || bytes == 0)
        {
            continue;
        }
        std::vector<BYTE> information(bytes, 0);
        if (!SetupGetInfInformationW(
                path.c_str(), INFINFO_INF_NAME_IS_ABSOLUTE,
                reinterpret_cast<PSP_INF_INFORMATION>(information.data()), bytes, nullptr))
        {
            continue;
        }
        SP_ORIGINAL_FILE_INFO_W original = {};
        original.cbSize = sizeof(original);
        if (SetupQueryInfOriginalFileInformationW(
                reinterpret_cast<PSP_INF_INFORMATION>(information.data()),
                0,
                nullptr,
                &original) &&
            _wcsicmp(original.OriginalInfName, L"PulseVirtualMic.inf") == 0 &&
            _wcsicmp(original.OriginalInfName, name) != 0 &&
            _wcsicmp(original.OriginalCatalogName, L"PulseVirtualMic.cat") == 0 &&
            _wcsicmp(InfProvider(path).c_str(), L"Pulse") == 0)
        {
            return name;
        }
    }
    return {};
}

void CleanupFailedPulseInstall()
{
    HDEVINFO devices = SetupDiGetClassDevsW(&GUID_DEVCLASS_MEDIA, nullptr, nullptr, 0);
    if (devices != INVALID_HANDLE_VALUE)
    {
        for (DWORD index = 0;; ++index)
        {
            SP_DEVINFO_DATA device = {};
            device.cbSize = sizeof(device);
            if (!SetupDiEnumDeviceInfo(devices, index, &device))
            {
                break;
            }
            if (!MatchesHardwareId(devices, device, PulseHardwareId))
            {
                continue;
            }
            SP_REMOVEDEVICE_PARAMS remove = {};
            remove.ClassInstallHeader.cbSize = sizeof(SP_CLASSINSTALL_HEADER);
            remove.ClassInstallHeader.InstallFunction = DIF_REMOVE;
            remove.Scope = DI_REMOVEDEVICE_GLOBAL;
            SetupDiSetClassInstallParamsW(
                devices, &device, &remove.ClassInstallHeader, sizeof(remove));
            SetupDiCallClassInstaller(DIF_REMOVE, devices, &device);
            break;
        }
        SetupDiDestroyDeviceInfoList(devices);
    }

    const std::wstring publishedInf = FindPulsePublishedInf();
    if (!publishedInf.empty())
    {
        SetupUninstallOEMInfW(publishedInf.c_str(), 0, nullptr);
    }
}

std::wstring DriverInfName(HDEVINFO devices, SP_DEVINFO_DATA& device)
{
    DEVPROPTYPE type = 0;
    DWORD bytes = 0;
    SetupDiGetDevicePropertyW(devices, &device, &DEVPKEY_Device_DriverInfPath, &type, nullptr, 0, &bytes, 0);
    if (GetLastError() != ERROR_INSUFFICIENT_BUFFER || bytes == 0)
    {
        return {};
    }
    std::vector<BYTE> buffer(bytes + sizeof(wchar_t), 0);
    if (!SetupDiGetDevicePropertyW(
            devices, &device, &DEVPKEY_Device_DriverInfPath, &type, buffer.data(), bytes, nullptr, 0))
    {
        return {};
    }
    return reinterpret_cast<const wchar_t*>(buffer.data());
}

bool FindMatchingDevice(
    HDEVINFO devices,
    const wchar_t* hardwareId,
    const wchar_t* service,
    SP_DEVINFO_DATA* match)
{
    for (DWORD index = 0;; ++index)
    {
        SP_DEVINFO_DATA device = {};
        device.cbSize = sizeof(device);
        if (!SetupDiEnumDeviceInfo(devices, index, &device))
        {
            return false;
        }
        if (MatchesDevice(devices, device, hardwareId, service))
        {
            *match = device;
            return true;
        }
    }
}

Result Install(const std::filesystem::path& package)
{
    Result result;
    const std::filesystem::path inf = std::filesystem::absolute(package / L"PulseVirtualMic.inf");
    if (!std::filesystem::is_regular_file(inf) ||
        !std::filesystem::is_regular_file(package / L"PulseVirtualMic.sys") ||
        !std::filesystem::is_regular_file(package / L"PulseVirtualMic.cat"))
    {
        result.ErrorCode = ERROR_FILE_NOT_FOUND;
        result.Message = L"The Pulse driver package must contain PulseVirtualMic.inf, PulseVirtualMic.sys, and PulseVirtualMic.cat.";
        return result;
    }

    HDEVINFO devices = SetupDiGetClassDevsW(&GUID_DEVCLASS_MEDIA, nullptr, nullptr, DIGCF_PRESENT);
    if (devices != INVALID_HANDLE_VALUE)
    {
        SP_DEVINFO_DATA existing = {};
        if (FindMatchingDevice(devices, PulseHardwareId, PulseService, &existing))
        {
            result.DevicePresent = true;
        }
        SetupDiDestroyDeviceInfoList(devices);
    }

    if (!result.DevicePresent)
    {
        devices = SetupDiCreateDeviceInfoList(&GUID_DEVCLASS_MEDIA, nullptr);
        if (devices == INVALID_HANDLE_VALUE)
        {
            result.ErrorCode = GetLastError();
            result.Message = L"Could not create the Pulse root device set: " + WindowsMessage(result.ErrorCode);
            return result;
        }
        SP_DEVINFO_DATA device = {};
        device.cbSize = sizeof(device);
        if (!SetupDiCreateDeviceInfoW(
                devices, L"PulseVirtualMic", &GUID_DEVCLASS_MEDIA, L"Pulse Virtual Microphone", nullptr,
                DICD_GENERATE_ID, &device))
        {
            result.ErrorCode = GetLastError();
            result.Message = L"Could not create the Pulse root devnode: " + WindowsMessage(result.ErrorCode);
            SetupDiDestroyDeviceInfoList(devices);
            return result;
        }

        const wchar_t hardwareIds[] = L"ROOT\\PulseVirtualMic\0";
        if (!SetupDiSetDeviceRegistryPropertyW(
                devices, &device, SPDRP_HARDWAREID,
                reinterpret_cast<const BYTE*>(hardwareIds), sizeof(hardwareIds)) ||
            !SetupDiCallClassInstaller(DIF_REGISTERDEVICE, devices, &device))
        {
            result.ErrorCode = GetLastError();
            result.Message = L"Could not register the Pulse root devnode: " + WindowsMessage(result.ErrorCode);
            SetupDiDestroyDeviceInfoList(devices);
            return result;
        }
        SetupDiDestroyDeviceInfoList(devices);
    }

    BOOL reboot = FALSE;
    if (!UpdateDriverForPlugAndPlayDevicesW(
            nullptr, PulseHardwareId, inf.c_str(), INSTALLFLAG_FORCE, &reboot))
    {
        result.ErrorCode = GetLastError();
        result.Message = L"Could not install PulseVirtualMic.inf: " + WindowsMessage(result.ErrorCode);
        CleanupFailedPulseInstall();
        return result;
    }
    result.Ok = true;
    result.RestartRequired = reboot != FALSE;
    result.DevicePresent = true;
    result.InfName = FindPulsePublishedInf();
    result.Message = L"Pulse virtual microphone installed.";
    return result;
}

Result Remove(const wchar_t* hardwareId, const wchar_t* service, bool removeDriverPackage)
{
    Result result;
    HDEVINFO devices = SetupDiGetClassDevsW(&GUID_DEVCLASS_MEDIA, nullptr, nullptr, 0);
    if (devices == INVALID_HANDLE_VALUE)
    {
        result.ErrorCode = GetLastError();
        result.Message = L"Could not enumerate media devices: " + WindowsMessage(result.ErrorCode);
        return result;
    }

    SP_DEVINFO_DATA device = {};
    if (!FindMatchingDevice(devices, hardwareId, service, &device))
    {
        SetupDiDestroyDeviceInfoList(devices);
        if (removeDriverPackage && _wcsicmp(hardwareId, PulseHardwareId) == 0)
        {
            result.InfName = FindPulsePublishedInf();
            if (!result.InfName.empty() &&
                !SetupUninstallOEMInfW(result.InfName.c_str(), 0, nullptr))
            {
                result.ErrorCode = GetLastError();
                result.Message = L"The Pulse devnode is absent, but its owned Driver Store package could not be removed: " + WindowsMessage(result.ErrorCode);
                return result;
            }
        }
        result.Ok = true;
        result.Message = result.InfName.empty()
            ? L"The matching audio device is not installed."
            : L"The Pulse devnode was absent and its owned Driver Store package was removed.";
        return result;
    }
    result.DevicePresent = true;
    result.InfName = DriverInfName(devices, device);

    SP_REMOVEDEVICE_PARAMS remove = {};
    remove.ClassInstallHeader.cbSize = sizeof(SP_CLASSINSTALL_HEADER);
    remove.ClassInstallHeader.InstallFunction = DIF_REMOVE;
    remove.Scope = DI_REMOVEDEVICE_GLOBAL;
    remove.HwProfile = 0;
    if (!SetupDiSetClassInstallParamsW(
            devices, &device, &remove.ClassInstallHeader, sizeof(remove)) ||
        !SetupDiCallClassInstaller(DIF_REMOVE, devices, &device))
    {
        result.ErrorCode = GetLastError();
        result.Message = L"Could not remove the matching audio devnode: " + WindowsMessage(result.ErrorCode);
        SetupDiDestroyDeviceInfoList(devices);
        return result;
    }

    SP_DEVINSTALL_PARAMS_W parameters = {};
    parameters.cbSize = sizeof(parameters);
    if (SetupDiGetDeviceInstallParamsW(devices, &device, &parameters))
    {
        result.RestartRequired = (parameters.Flags & (DI_NEEDREBOOT | DI_NEEDRESTART)) != 0;
    }
    SetupDiDestroyDeviceInfoList(devices);
    result.DevicePresent = false;

    if (removeDriverPackage && result.InfName.empty() && _wcsicmp(hardwareId, PulseHardwareId) == 0)
    {
        result.InfName = FindPulsePublishedInf();
    }
    if (removeDriverPackage && !result.InfName.empty())
    {
        wchar_t windowsDirectory[MAX_PATH] = {};
        if (GetWindowsDirectoryW(windowsDirectory, ARRAYSIZE(windowsDirectory)) == 0)
        {
            result.ErrorCode = GetLastError();
            result.Message = L"The device was removed, but the Windows directory could not be located: " + WindowsMessage(result.ErrorCode);
            return result;
        }
        const std::filesystem::path infPath = std::filesystem::path(windowsDirectory) / L"INF" / result.InfName;
        BOOL reboot = FALSE;
        if (!DiUninstallDriverW(nullptr, infPath.c_str(), 0, &reboot))
        {
            result.ErrorCode = GetLastError();
            result.Message = L"The device was removed, but its Driver Store package could not be removed: " + WindowsMessage(result.ErrorCode);
            result.RestartRequired = result.RestartRequired || reboot != FALSE;
            return result;
        }
        result.RestartRequired = result.RestartRequired || reboot != FALSE;
    }

    result.Ok = true;
    result.Message = L"The matching audio device and owned driver package were removed.";
    return result;
}

Result Status()
{
    Result result;
    HDEVINFO devices = SetupDiGetClassDevsW(&GUID_DEVCLASS_MEDIA, nullptr, nullptr, DIGCF_PRESENT);
    if (devices == INVALID_HANDLE_VALUE)
    {
        result.ErrorCode = GetLastError();
        result.Message = L"Could not enumerate media devices: " + WindowsMessage(result.ErrorCode);
        return result;
    }
    SP_DEVINFO_DATA device = {};
    result.DevicePresent = FindMatchingDevice(devices, PulseHardwareId, PulseService, &device);
    if (result.DevicePresent)
    {
        result.InfName = DriverInfName(devices, device);
    }
    SetupDiDestroyDeviceInfoList(devices);
    result.Ok = true;
    result.Message = result.DevicePresent ? L"Pulse virtual microphone is present." : L"Pulse virtual microphone is not present.";
    return result;
}

std::optional<std::wstring> Argument(int argc, wchar_t** argv, const wchar_t* name)
{
    for (int index = 2; index + 1 < argc; ++index)
    {
        if (_wcsicmp(argv[index], name) == 0)
        {
            return argv[index + 1];
        }
    }
    return std::nullopt;
}

std::optional<std::wstring> ArgumentOrEnvironment(
    int argc,
    wchar_t** argv,
    const wchar_t* name,
    const wchar_t* environmentName)
{
    if (const auto argument = Argument(argc, argv, name); argument.has_value())
    {
        return argument;
    }
    wchar_t* value = nullptr;
    size_t length = 0;
    if (_wdupenv_s(&value, &length, environmentName) == 0 && value != nullptr && length > 1)
    {
        std::wstring result(value);
        free(value);
        return result;
    }
    free(value);
    return std::nullopt;
}
}

int wmain(int argc, wchar_t** argv)
{
    Result result;
    if (argc < 2)
    {
        result.ErrorCode = ERROR_INVALID_PARAMETER;
        result.Message = L"Usage: PulseDriverInstaller <install|remove|remove-legacy|status> [--package path] [--result path]";
    }
    else if (_wcsicmp(argv[1], L"install") == 0)
    {
        const auto package = ArgumentOrEnvironment(
            argc, argv, L"--package", L"PULSE_DRIVER_PACKAGE");
        if (package.has_value())
        {
            result = Install(*package);
        }
        else
        {
            result.ErrorCode = ERROR_INVALID_PARAMETER;
            result.Message = L"The install command requires --package.";
        }
    }
    else if (_wcsicmp(argv[1], L"remove") == 0)
    {
        result = Remove(PulseHardwareId, PulseService, true);
    }
    else if (_wcsicmp(argv[1], L"remove-legacy") == 0)
    {
        result = Remove(LegacyHardwareId, LegacyService, true);
    }
    else if (_wcsicmp(argv[1], L"status") == 0)
    {
        result = Status();
    }
    else
    {
        result.ErrorCode = ERROR_INVALID_PARAMETER;
        result.Message = L"Unknown command.";
    }

    const std::string json = ToJson(result);
    std::cout << json << '\n';
    if (const auto output = ArgumentOrEnvironment(
            argc, argv, L"--result", L"PULSE_DRIVER_RESULT"); output.has_value())
    {
        std::ofstream file(std::filesystem::path(*output), std::ios::binary | std::ios::trunc);
        file << json;
    }
    return result.Ok ? 0 : 1;
}
