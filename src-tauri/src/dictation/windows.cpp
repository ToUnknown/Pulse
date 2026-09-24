// Windows platform bridge. Shared Rust owns recording, protocol, UI and delivery
// state. No microphone, target field, or clipboard is touched while idle.
#define NOMINMAX
#include <windows.h>
#include <uiautomation.h>
#include <mmdeviceapi.h>
#include <ole2.h>
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
static std::atomic<Shortcut> shortcutCallback{nullptr};
static std::thread hookThread;
static DWORD hookThreadId;
static HHOOK keyboardHook;
static bool rightDown, passRightAlt;

static LRESULT CALLBACK keyboard(int code, WPARAM message, LPARAM data) {
    if (code == HC_ACTION) {
        auto *key = reinterpret_cast<KBDLLHOOKSTRUCT*>(data);
        if (!(key->flags & LLKHF_INJECTED)) {
            bool down = message == WM_KEYDOWN || message == WM_SYSKEYDOWN;
            bool right = key->vkCode == VK_RMENU || (key->vkCode == VK_MENU && (key->flags & LLKHF_EXTENDED));
            if (right) {
                if (down && !rightDown) passRightAlt=(GetAsyncKeyState(VK_CONTROL)&0x8000)!=0;
                rightDown=down;
            }
            // Forward AltGr/Ctrl+Alt to the app without presenting it as a
            // dictation press, including while hands-free recording is active.
            bool chord = down && !right;
            if (auto callback = shortcutCallback.load()) callback(rightDown && !passRightAlt,chord,false,GetTickCount64()/1000.0);
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
static HWND overlayWindow;
static std::wstring utf16(const char *text) {
    int n=MultiByteToWideChar(CP_UTF8,MB_ERR_INVALID_CHARS,text,-1,nullptr,0);
    if (!n) return {};
    std::wstring result(n,L'\0'); MultiByteToWideChar(CP_UTF8,MB_ERR_INVALID_CHARS,text,-1,result.data(),n);
    result.resize(n-1); return result;
}
static bool initAutomation() {
    if (automation) return true;
    HRESULT init=CoInitializeEx(nullptr,COINIT_APARTMENTTHREADED);
    if (FAILED(init) && init!=RPC_E_CHANGED_MODE) return false;
    HRESULT hr=CoCreateInstance(__uuidof(CUIAutomation8),nullptr,CLSCTX_INPROC_SERVER,IID_PPV_ARGS(&automation));
    if (SUCCEEDED(hr) && automation) {
        ComPtr<IUIAutomation2> limits;
        if (SUCCEEDED(automation.As(&limits)) && limits) { limits->put_ConnectionTimeout(250); limits->put_TransactionTimeout(250); }
    }
    return SUCCEEDED(hr) && automation;
}
enum class TextInput { Unknown, Rejected, Editable };
static TextInput focusedTextInput() {
    ComPtr<IUIAutomationElement> element;
    if (!initAutomation() || FAILED(automation->GetFocusedElement(&element)) || !element) return TextInput::Unknown;
    BOOL password=FALSE, enabled=TRUE;
    if ((SUCCEEDED(element->get_CurrentIsPassword(&password)) && password) ||
        (SUCCEEDED(element->get_CurrentIsEnabled(&enabled)) && !enabled)) return TextInput::Rejected;
    ComPtr<IUIAutomationValuePattern> value;
    BOOL readOnly=TRUE;
    if (SUCCEEDED(element->GetCurrentPatternAs(UIA_ValuePatternId,IID_PPV_ARGS(&value))) && value &&
        SUCCEEDED(value->get_CurrentIsReadOnly(&readOnly))) return readOnly ? TextInput::Rejected : TextInput::Editable;
    ComPtr<IUIAutomationTextPattern> text; ComPtr<IUIAutomationTextRange> document;
    if (SUCCEEDED(element->GetCurrentPatternAs(UIA_TextPatternId,IID_PPV_ARGS(&text))) && text &&
        SUCCEEDED(text->get_DocumentRange(&document)) && document) {
        VARIANT attribute; VariantInit(&attribute);
        bool known=SUCCEEDED(document->GetAttributeValue(UIA_IsReadOnlyAttributeId,&attribute)) && attribute.vt==VT_BOOL;
        bool writable=known && attribute.boolVal==VARIANT_FALSE;
        VariantClear(&attribute);
        if (known) return writable ? TextInput::Editable : TextInput::Rejected;
    }
    CONTROLTYPEID type=0;
    return SUCCEEDED(element->get_CurrentControlType(&type)) && type==UIA_EditControlTypeId
        ? TextInput::Editable : TextInput::Unknown;
}
static bool modifiersDown() {
    return ((GetAsyncKeyState(VK_MENU)|GetAsyncKeyState(VK_CONTROL)|GetAsyncKeyState(VK_SHIFT)|
        GetAsyncKeyState(VK_LWIN)|GetAsyncKeyState(VK_RWIN))&0x8000)!=0;
}
enum class ClipboardWrite { Failed, Changed, Written };
static ClipboardWrite writeClipboardText(const char *utf8, const DWORD *expectedSequence, DWORD *writtenSequence);
static ComPtr<IDataObject> previousClipboard;
static DWORD temporarySequence;
static bool pastePending;
static void restoreClipboard() {
    if (previousClipboard && temporarySequence && GetClipboardSequenceNumber()==temporarySequence) {
        // If the user copied something newer, their clipboard always wins.
        OleSetClipboard(previousClipboard.Get());
    }
    previousClipboard.Reset();
    temporarySequence=0;
    pastePending=false;
    OleUninitialize();
}
static VOID CALLBACK finishPaste(HWND window, UINT message, UINT_PTR timer, DWORD time) {
    (void)window; (void)message; (void)time;
    KillTimer(nullptr,timer);
    restoreClipboard();
}
// 0 = wait for physical modifiers; 1 = paste submitted; -1 = copy instead.
extern "C" int pulse_dictation_final_step(const char *utf8) {
    if (modifiersDown() || pastePending) return 0;
    if (!utf8 || !*utf8) return -1;
    HWND foreground=GetForegroundWindow();
    if (!foreground) return -1;
    TextInput inputKind=focusedTextInput();
    GUITHREADINFO info{sizeof(info)};
    DWORD thread=GetWindowThreadProcessId(foreground,nullptr);
    bool haveInfo=thread && GetGUIThreadInfo(thread,&info);
    if (GetForegroundWindow()!=foreground) return 0;
    if (inputKind==TextInput::Rejected) return -1;
    if (haveInfo && (info.flags&(GUI_INMENUMODE|GUI_INMOVESIZE))) return -1;
    bool caret=haveInfo && info.hwndFocus && info.hwndCaret &&
        (info.hwndFocus==info.hwndCaret || IsChild(info.hwndFocus,info.hwndCaret));
    if (inputKind!=TextInput::Editable && !caret) return -1;

    HRESULT init=OleInitialize(nullptr);
    if (FAILED(init)) return -1;
    DWORD before=GetClipboardSequenceNumber();
    if (!before) { OleUninitialize(); return -1; }
    ComPtr<IDataObject> previous;
    // An empty or unavailable clipboard is not a reason to skip a focused
    // editor. Paste once, but leave the transcript if nothing can be restored.
    if (FAILED(OleGetClipboard(previous.GetAddressOf()))) previous.Reset();
    // The data object and sequence must describe the same clipboard contents.
    if (GetClipboardSequenceNumber()!=before) { OleUninitialize(); return 0; }
    if (modifiersDown() || GetForegroundWindow()!=foreground) { OleUninitialize(); return 0; }
    DWORD temporary=0;
    ClipboardWrite written=writeClipboardText(utf8,&before,&temporary);
    if (written==ClipboardWrite::Changed) { OleUninitialize(); return 0; }
    if (written==ClipboardWrite::Failed) {
        if (previous && temporary && GetClipboardSequenceNumber()==temporary) OleSetClipboard(previous.Get());
        OleUninitialize();
        return -1;
    }
    if (modifiersDown() || GetForegroundWindow()!=foreground) {
        if (previous && GetClipboardSequenceNumber()==temporary) OleSetClipboard(previous.Get());
        OleUninitialize();
        return 0;
    }

    INPUT keys[4]{};
    keys[0].type=keys[1].type=keys[2].type=keys[3].type=INPUT_KEYBOARD;
    keys[0].ki.wVk=keys[3].ki.wVk=VK_CONTROL;
    keys[1].ki.wVk=keys[2].ki.wVk='V';
    keys[2].ki.dwFlags=keys[3].ki.dwFlags=KEYEVENTF_KEYUP;
    UINT sent=SendInput(4,keys,sizeof(INPUT));
    if (!sent) {
        if (previous && GetClipboardSequenceNumber()==temporary) OleSetClipboard(previous.Get());
        OleUninitialize();
        return -1;
    }
    if (sent<4) {
        // A prefix may have pressed Ctrl (and V) without releasing them. The
        // paste may already have fired, so never submit the transcript again.
        INPUT releases[2]{};
        UINT count=0;
        if (sent==2) {
            releases[count].type=INPUT_KEYBOARD;
            releases[count].ki.wVk='V';
            releases[count++].ki.dwFlags=KEYEVENTF_KEYUP;
        }
        releases[count].type=INPUT_KEYBOARD;
        releases[count].ki.wVk=VK_CONTROL;
        releases[count++].ki.dwFlags=KEYEVENTF_KEYUP;
        for (UINT released=0, attempts=0; released<count && attempts<3; attempts++) {
            released+=SendInput(count-released,releases+released,sizeof(INPUT));
        }
    }
    previousClipboard=previous;
    temporarySequence=temporary;
    pastePending=true;
    if (!SetTimer(nullptr,0,250,finishPaste)) {
        // Timers normally run on this UI thread. Keep restoration safe if one
        // cannot be installed, at the cost of a brief finalization pause.
        Sleep(250);
        restoreClipboard();
    }
    return 1;
}
static ClipboardWrite writeClipboardText(const char *utf8, const DWORD *expectedSequence, DWORD *writtenSequence) {
    std::wstring text=utf16(utf8); if(text.empty()) return ClipboardWrite::Failed;
    HGLOBAL memory=GlobalAlloc(GMEM_MOVEABLE,(text.size()+1)*sizeof(wchar_t));
    if (!memory) return ClipboardWrite::Failed;
    void *data=GlobalLock(memory); if(!data) { GlobalFree(memory); return ClipboardWrite::Failed; }
    memcpy(data,text.c_str(),(text.size()+1)*sizeof(wchar_t)); GlobalUnlock(memory);
    ClipboardWrite result=ClipboardWrite::Failed;
    if (IsWindow(overlayWindow)) for (int attempt=0;attempt<5;attempt++) {
        if (OpenClipboard(overlayWindow)) {
            // OpenClipboard excludes other writers until the transaction ends.
            if (expectedSequence && GetClipboardSequenceNumber()!=*expectedSequence) {
                result=ClipboardWrite::Changed;
            } else if (EmptyClipboard()) {
                if (SetClipboardData(CF_UNICODETEXT,memory)) result=ClipboardWrite::Written;
                if (writtenSequence) *writtenSequence=GetClipboardSequenceNumber();
            }
            CloseClipboard(); break;
        }
        Sleep(10);
    }
    if (result!=ClipboardWrite::Written) GlobalFree(memory);
    return result;
}
extern "C" bool pulse_dictation_copy(const char *utf8) {
    return writeClipboardText(utf8,nullptr,nullptr)==ClipboardWrite::Written;
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
