// Windows platform bridge. Shared Rust owns recording, protocol, UI and delivery
// state. No microphone, target field, or clipboard is touched while idle.
#define NOMINMAX
#include <windows.h>
#include <uiautomation.h>
#include <mmdeviceapi.h>
#include <audioclient.h>
#include <shellapi.h>
#include <wrl/client.h>
#include <atomic>
#include <cmath>
#include <cstdint>
#include <future>
#include <mutex>
#include <string>
#include <thread>
#include <vector>
using Microsoft::WRL::ComPtr;
using Shortcut = void(*)(bool,bool,bool,double);
using Audio = void(*)(uint64_t,const uint8_t*,size_t,float);
static constexpr ULONG_PTR EventTag = 0x50554c53;
static std::atomic<Shortcut> shortcutCallback{nullptr};
static std::thread hookThread;
static DWORD hookThreadId;
static HHOOK keyboardHook;
static bool rightDown, passRightAlt;
static std::atomic<bool> deliveryInterrupted{false}, deliveryActive{false};

static LRESULT CALLBACK keyboard(int code, WPARAM message, LPARAM data) {
    if (code == HC_ACTION) {
        auto *key = reinterpret_cast<KBDLLHOOKSTRUCT*>(data);
        if (!(key->flags & LLKHF_INJECTED)) {
            bool down = message == WM_KEYDOWN || message == WM_SYSKEYDOWN;
            bool right = key->vkCode == VK_RMENU || (key->vkCode == VK_MENU && (key->flags & LLKHF_EXTENDED));
            if (down && deliveryActive) deliveryInterrupted = true;
            if (right) {
                if (down && !rightDown) passRightAlt=(GetAsyncKeyState(VK_CONTROL)&0x8000)!=0;
                rightDown=down;
            }
            // AltGr injects a synthetic left Ctrl; consult physical hook events,
            // not GetAsyncKeyState(VK_CONTROL), when interpreting Right Alt.
            bool chord = down && !right;
            if (auto callback = shortcutCallback.load()) callback(rightDown,chord,false,GetTickCount64()/1000.0);
            // Reserve bare Right Alt so releasing it cannot focus an app's menu
            // and lose the text field. Preserve the Ctrl+Alt sequence of AltGr.
            if (right && !passRightAlt) return 1;
        }
    }
    return CallNextHookEx(keyboardHook,code,message,data);
}
extern "C" bool pulse_dictation_right_option_down() { return (GetAsyncKeyState(VK_RMENU)&0x8000)!=0; }
extern "C" void pulse_dictation_unregister_shortcut() {
    shortcutCallback = nullptr;
    if (hookThread.joinable()) { PostThreadMessageW(hookThreadId,WM_QUIT,0,0); hookThread.join(); }
}
extern "C" bool pulse_dictation_register_shortcut(Shortcut callback) {
    pulse_dictation_unregister_shortcut();
    std::promise<bool> ready; auto started=ready.get_future();
    shortcutCallback=callback;
    hookThread=std::thread([p=std::move(ready)]() mutable {
        hookThreadId=GetCurrentThreadId();
        MSG message; PeekMessageW(&message,nullptr,WM_USER,WM_USER,PM_NOREMOVE);
        rightDown=pulse_dictation_right_option_down();
        passRightAlt=rightDown; // Do not consume an unmatched release when enabling.
        keyboardHook=SetWindowsHookExW(WH_KEYBOARD_LL,keyboard,GetModuleHandleW(nullptr),0);
        p.set_value(keyboardHook!=nullptr);
        if (keyboardHook) {
            while (GetMessageW(&message,nullptr,0,0)>0) { TranslateMessage(&message); DispatchMessageW(&message); }
            UnhookWindowsHookEx(keyboardHook); keyboardHook=nullptr;
        }
    });
    bool ok=started.get();
    if (!ok) pulse_dictation_unregister_shortcut();
    return ok;
}
extern "C" bool pulse_dictation_ax_allowed() { return true; }
extern "C" bool pulse_dictation_mic_allowed() {
    wchar_t consent[32]{}; DWORD size=sizeof(consent);
    auto result=RegGetValueW(HKEY_CURRENT_USER,L"Software\\Microsoft\\Windows\\CurrentVersion\\CapabilityAccessManager\\ConsentStore\\microphone",L"Value",RRF_RT_REG_SZ,nullptr,consent,&size);
    return result!=ERROR_SUCCESS || wcscmp(consent,L"Deny")!=0;
}
extern "C" void pulse_dictation_request_access(bool microphone) {
    if (microphone) ShellExecuteW(nullptr,L"open",L"ms-settings:privacy-microphone",nullptr,nullptr,SW_SHOWNORMAL);
}

static std::mutex audioMutex;
static std::thread audioThread;
static HANDLE audioStop;
extern "C" void pulse_dictation_stop() {
    std::lock_guard<std::mutex> lock(audioMutex);
    if (audioThread.joinable()) { SetEvent(audioStop); audioThread.join(); }
    if (audioStop) { CloseHandle(audioStop); audioStop=nullptr; }
}
extern "C" void pulse_dictation_start_async(uint64_t id, Audio callback, void(*ready)(void*,bool), void *context) {
    pulse_dictation_stop();
    std::lock_guard<std::mutex> lock(audioMutex);
    audioStop=CreateEventW(nullptr,TRUE,FALSE,nullptr);
    if (!audioStop) { ready(context,false); return; }
    audioThread=std::thread([=,stop=audioStop] {
        HRESULT com=CoInitializeEx(nullptr,COINIT_MULTITHREADED);
        HANDLE samples=CreateEventW(nullptr,FALSE,FALSE,nullptr);
        {
            ComPtr<IMMDeviceEnumerator> devices; ComPtr<IMMDevice> device;
            ComPtr<IAudioClient> client; ComPtr<IAudioCaptureClient> capture;
            WAVEFORMATEX format{WAVE_FORMAT_PCM,1,24000,48000,2,16,0};
            HRESULT hr=CoCreateInstance(__uuidof(MMDeviceEnumerator),nullptr,CLSCTX_ALL,IID_PPV_ARGS(&devices));
            if (SUCCEEDED(hr)) hr=devices->GetDefaultAudioEndpoint(eCapture,eConsole,&device);
            if (SUCCEEDED(hr)) hr=device->Activate(__uuidof(IAudioClient),CLSCTX_ALL,nullptr,reinterpret_cast<void**>(client.GetAddressOf()));
            // WASAPI converts the default device's mix format to model-ready PCM.
            if (SUCCEEDED(hr)) hr=client->Initialize(AUDCLNT_SHAREMODE_SHARED,AUDCLNT_STREAMFLAGS_EVENTCALLBACK|AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM|AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,1000000,0,&format,nullptr);
            if (SUCCEEDED(hr)) hr=samples ? client->SetEventHandle(samples) : E_FAIL;
            if (SUCCEEDED(hr)) hr=client->GetService(IID_PPV_ARGS(&capture));
            if (SUCCEEDED(hr)) hr=client->Start();
            ready(context,SUCCEEDED(hr));
            if (SUCCEEDED(hr)) {
                HANDLE events[]{stop,samples};
                bool stopping=false, failed=false;
                while (!stopping && !failed) {
                    DWORD wake=WaitForMultipleObjects(2,events,FALSE,1000);
                    stopping=wake==WAIT_OBJECT_0;
                    if (wake==WAIT_FAILED) { failed=true; break; }
                    UINT32 packets=0;
                    while (SUCCEEDED(hr=capture->GetNextPacketSize(&packets)) && packets) {
                        BYTE *data=nullptr; UINT32 frames=0; DWORD flags=0;
                        hr=capture->GetBuffer(&data,&frames,&flags,nullptr,nullptr);
                        if (FAILED(hr)) break;
                        std::vector<int16_t> pcm(frames,0);
                        if (!(flags&AUDCLNT_BUFFERFLAGS_SILENT)) memcpy(pcm.data(),data,frames*sizeof(int16_t));
                        capture->ReleaseBuffer(frames);
                        double power=0; for (int16_t sample:pcm) { double v=sample/32768.0; power+=v*v; }
                        if (frames) callback(id,reinterpret_cast<const uint8_t*>(pcm.data()),pcm.size()*2,static_cast<float>(sqrt(power/frames)));
                    }
                    failed=FAILED(hr);
                }
                client->Stop();
                if (failed) callback(id,nullptr,0,-1);
            }
        }
        if (samples) CloseHandle(samples);
        if (SUCCEEDED(com)) CoUninitialize();
    });
}

static ComPtr<IUIAutomation> automation;
static ComPtr<IUIAutomationElement> target;
static HWND targetWindow, overlayWindow;
static std::wstring baseText, expectedText;
static size_t selectionStart, selectionLength;
static bool pending;
static ULONGLONG pendingSince;
static std::wstring utf16(const char *text) {
    int n=MultiByteToWideChar(CP_UTF8,MB_ERR_INVALID_CHARS,text,-1,nullptr,0);
    if (!n) return {};
    std::wstring result(n,L'\0'); MultiByteToWideChar(CP_UTF8,MB_ERR_INVALID_CHARS,text,-1,result.data(),n);
    result.resize(n-1); return result;
}
static std::wstring bstr(BSTR value) { return value ? std::wstring(value,SysStringLen(value)) : std::wstring(); }
static bool initAutomation() {
    if (automation) return true;
    // Tauri initializes COM for WebView2 on this thread. Do not change its apartment.
    HRESULT init=CoInitializeEx(nullptr,COINIT_APARTMENTTHREADED);
    if (FAILED(init) && init!=RPC_E_CHANGED_MODE) return false;
    HRESULT hr=CoCreateInstance(__uuidof(CUIAutomation8),nullptr,CLSCTX_INPROC_SERVER,IID_PPV_ARGS(&automation));
    if (SUCCEEDED(hr)) {
        ComPtr<IUIAutomation2> limits;
        if (SUCCEEDED(automation.As(&limits))) { limits->put_ConnectionTimeout(500); limits->put_TransactionTimeout(500); }
    }
    return SUCCEEDED(hr);
}
static bool editable(IUIAutomationElement *element) {
    BOOL password=TRUE, enabled=FALSE, focus=FALSE;
    if (FAILED(element->get_CurrentIsPassword(&password)) || password ||
        FAILED(element->get_CurrentIsEnabled(&enabled)) || !enabled ||
        FAILED(element->get_CurrentHasKeyboardFocus(&focus)) || !focus) return false;
    ComPtr<IUIAutomationValuePattern> value;
    BOOL readOnly=TRUE;
    if (SUCCEEDED(element->GetCurrentPatternAs(UIA_ValuePatternId,IID_PPV_ARGS(&value))) &&
        SUCCEEDED(value->get_CurrentIsReadOnly(&readOnly))) return !readOnly;
    ComPtr<IUIAutomationTextPattern> text; ComPtr<IUIAutomationTextRange> document;
    if (FAILED(element->GetCurrentPatternAs(UIA_TextPatternId,IID_PPV_ARGS(&text))) || FAILED(text->get_DocumentRange(&document))) return false;
    VARIANT attribute; VariantInit(&attribute);
    bool writable=SUCCEEDED(document->GetAttributeValue(UIA_IsReadOnlyAttributeId,&attribute)) && attribute.vt==VT_BOOL && attribute.boolVal==VARIANT_FALSE;
    VariantClear(&attribute); return writable;
}
static bool snapshot(IUIAutomationElement *element, std::wstring &value, size_t *start=nullptr, size_t *length=nullptr) {
    ComPtr<IUIAutomationTextPattern> text; ComPtr<IUIAutomationTextRange> document;
    if (FAILED(element->GetCurrentPatternAs(UIA_TextPatternId,IID_PPV_ARGS(&text))) || FAILED(text->get_DocumentRange(&document))) {
        // Classic single-line Edit controls may expose ValuePattern only.
        // Read their existing selection; never select-all or replace the field.
        ComPtr<IUIAutomationValuePattern> field;
        BSTR raw=nullptr;
        if (FAILED(element->GetCurrentPatternAs(UIA_ValuePatternId,IID_PPV_ARGS(&field))) || FAILED(field->get_CurrentValue(&raw))) return false;
        value=bstr(raw); SysFreeString(raw);
        if (!start) return true;
        UIA_HWND native=nullptr;
        if (FAILED(element->get_CurrentNativeWindowHandle(&native)) || !native || value.size()>65535) return false;
        HWND handle=reinterpret_cast<HWND>(native);
        wchar_t name[32]{};
        if (!GetClassNameW(handle,name,32) || _wcsicmp(name,L"Edit")!=0) return false;
        DWORD_PTR selected=0;
        if (!SendMessageTimeoutW(handle,EM_GETSEL,0,0,SMTO_ABORTIFHUNG|SMTO_BLOCK,100,&selected) || selected==0xffffffff) return false;
        *start=LOWORD(selected); size_t end=HIWORD(selected);
        if (end<*start || end>value.size()) return false;
        *length=end-*start; return true;
    }
    BSTR raw=nullptr;
    if (FAILED(document->GetText(-1,&raw))) return false;
    value=bstr(raw); SysFreeString(raw);
    if (!start) return true;
    ComPtr<IUIAutomationTextRangeArray> ranges; ComPtr<IUIAutomationTextRange> selected, prefix;
    int count=0;
    if (FAILED(text->GetSelection(&ranges)) || FAILED(ranges->get_Length(&count)) || count!=1 ||
        FAILED(ranges->GetElement(0,&selected)) || FAILED(document->Clone(&prefix)) ||
        FAILED(prefix->MoveEndpointByRange(TextPatternRangeEndpoint_End,selected.Get(),TextPatternRangeEndpoint_Start))) return false;
    if (FAILED(prefix->GetText(-1,&raw))) return false;
    *start=SysStringLen(raw); SysFreeString(raw);
    if (FAILED(selected->GetText(-1,&raw))) return false;
    *length=SysStringLen(raw); SysFreeString(raw);
    return *start<=value.size() && *length<=value.size()-*start;
}
static std::wstring normalizeLines(const std::wstring &value) {
    std::wstring result;
    for (size_t i=0;i<value.size();i++) {
        if (value[i]==L'\r') { result+=L'\n'; if(i+1<value.size() && value[i+1]==L'\n') i++; }
        else result+=value[i];
    }
    return result;
}
static bool sameInput() {
    if (!target || GetForegroundWindow()!=targetWindow) return false;
    ComPtr<IUIAutomationElement> focused; BOOL same=FALSE;
    return SUCCEEDED(automation->GetFocusedElement(&focused)) && focused &&
        SUCCEEDED(automation->CompareElements(target.Get(),focused.Get(),&same)) && same;
}
extern "C" void pulse_dictation_clear_target() {
    deliveryActive=false; target.Reset(); targetWindow=nullptr;
    baseText.clear(); expectedText.clear(); pending=false; deliveryInterrupted=false;
}
extern "C" int pulse_dictation_delivery_begin() {
    pulse_dictation_clear_target();
    if (!initAutomation()) return 0;
    targetWindow=GetForegroundWindow();
    if (!targetWindow || FAILED(automation->GetFocusedElement(&target)) || !target) return 0;
    if (!editable(target.Get()) || !snapshot(target.Get(),baseText,&selectionStart,&selectionLength) || !sameInput()) {
        pulse_dictation_clear_target(); return 2;
    }
    deliveryActive=true; return 1;
}
extern "C" int pulse_dictation_final_step(const char *utf8) {
    if (!target || deliveryInterrupted || !sameInput()) return -1;
    std::wstring actual;
    if (pending) {
        if (snapshot(target.Get(),actual) && normalizeLines(actual)==normalizeLines(expectedText)) return 1;
        return GetTickCount64()-pendingSince<1000 ? 0 : -1;
    }
    size_t start=0,length=0;
    if (!editable(target.Get()) || !snapshot(target.Get(),actual,&start,&length) ||
        actual!=baseText || start!=selectionStart || length!=selectionLength) return -1;
    std::wstring text=utf16(utf8);
    if (text.empty()) return -1;
    // Do not release/repress the user's physical modifiers. Wait briefly for
    // the dictation key to rise instead of generating Alt/Ctrl shortcuts.
    if ((GetAsyncKeyState(VK_MENU)|GetAsyncKeyState(VK_CONTROL)|GetAsyncKeyState(VK_SHIFT)|GetAsyncKeyState(VK_LWIN)|GetAsyncKeyState(VK_RWIN))&0x8000) return 0;
    if (deliveryInterrupted || !sameInput()) return -1;
    expectedText=baseText.substr(0,start)+text+baseText.substr(start+length);
    std::vector<INPUT> input(text.size()*2);
    for (size_t i=0;i<text.size();i++) {
        input[2*i].type=input[2*i+1].type=INPUT_KEYBOARD;
        input[2*i].ki.wScan=input[2*i+1].ki.wScan=text[i];
        input[2*i].ki.dwFlags=KEYEVENTF_UNICODE;
        input[2*i+1].ki.dwFlags=KEYEVENTF_UNICODE|KEYEVENTF_KEYUP;
        input[2*i].ki.dwExtraInfo=input[2*i+1].ki.dwExtraInfo=EventTag;
    }
    // Native Edit/RichEdit controls offer an atomic, undoable selection edit.
    // Web/custom controls use the same verified transaction with Unicode input.
    pending=true; pendingSince=GetTickCount64();
    UIA_HWND native=nullptr;
    if (SUCCEEDED(target->get_CurrentNativeWindowHandle(&native)) && native) {
        HWND handle=reinterpret_cast<HWND>(native);
        wchar_t name[64]{}; GetClassNameW(handle,name,64);
        if (_wcsicmp(name,L"Edit")==0 || _wcsnicmp(name,L"RichEdit",8)==0) {
            DWORD_PTR result=0;
            // A timeout is ambiguous, so only read back; never replay a write.
            SendMessageTimeoutW(handle,EM_REPLACESEL,TRUE,reinterpret_cast<LPARAM>(text.c_str()),SMTO_ABORTIFHUNG|SMTO_BLOCK,200,&result);
            return 0;
        }
    }
    // One batch, no paced typing, no clipboard writes, and no retry after a
    // partial/ambiguous dispatch. Windows enforces elevated-app boundaries.
    UINT sent=SendInput(static_cast<UINT>(input.size()),input.data(),sizeof(INPUT));
    return sent ? 0 : -1;
}
extern "C" bool pulse_dictation_copy(const char *utf8) {
    std::wstring text=utf16(utf8); if(text.empty()) return false;
    HGLOBAL memory=GlobalAlloc(GMEM_MOVEABLE,(text.size()+1)*sizeof(wchar_t));
    if (!memory) return false;
    void *data=GlobalLock(memory); if(!data) { GlobalFree(memory); return false; }
    memcpy(data,text.c_str(),(text.size()+1)*sizeof(wchar_t)); GlobalUnlock(memory);
    bool copied=false;
    if (IsWindow(overlayWindow)) for (int attempt=0;attempt<5;attempt++) {
        if (OpenClipboard(overlayWindow)) {
            if (EmptyClipboard()) copied=SetClipboardData(CF_UNICODETEXT,memory)!=nullptr;
            CloseClipboard(); break;
        }
        Sleep(10);
    }
    if (!copied) GlobalFree(memory);
    return copied;
}
extern "C" void pulse_dictation_position(void *pointer,double *bottom) {
    HWND window=static_cast<HWND>(pointer);
    overlayWindow=window;
    MONITORINFO monitor{sizeof(monitor)};
    HMONITOR screen=MonitorFromWindow(GetForegroundWindow(),MONITOR_DEFAULTTOPRIMARY);
    if (!GetMonitorInfoW(screen,&monitor)) return;
    LONG_PTR style=GetWindowLongPtrW(window,GWL_EXSTYLE);
    SetWindowLongPtrW(window,GWL_EXSTYLE,style|WS_EX_NOACTIVATE|WS_EX_TOOLWINDOW|WS_EX_TRANSPARENT);
    SetWindowPos(window,HWND_TOPMOST,monitor.rcMonitor.left,monitor.rcMonitor.top,
        monitor.rcMonitor.right-monitor.rcMonitor.left,monitor.rcMonitor.bottom-monitor.rcMonitor.top,SWP_NOACTIVATE);
    double scale=GetDpiForWindow(window)/96.0;
    *bottom=(monitor.rcMonitor.bottom-monitor.rcWork.bottom)/scale+28;
}
// Apple optical glass is platform-specific. Keep the shared CSS material and
// feathered backdrop on Windows rather than applying blur to the full screen.
extern "C" void pulse_dictation_transcript_blur(void*,double,double,double,double,double,bool) {}
extern "C" bool pulse_dictation_glass(void*,double,double,double,double,double,bool) { return false; }
