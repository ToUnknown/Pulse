#import <AppKit/AppKit.h>
#import <AVFoundation/AVFoundation.h>
#import <ApplicationServices/ApplicationServices.h>
#import <QuartzCore/QuartzCore.h>
#import <objc/runtime.h>
#include <IOKit/hidsystem/IOLLEvent.h>
#include <stdbool.h>
#include <stdint.h>

// AX and window operations run on AppKit's main thread. Audio lifecycle runs
// on a dedicated serial queue; tap buffers are copied before returning.
static AVAudioEngine *engine;
static AXUIElementRef deliveryElement, deliveryWindow;
static const int64_t PulseDictationEventTag = 0x50554c5345444943;
static bool deliveryInterrupted;
static bool sameInput(void);
static pid_t deliveryPID;
static bool tapInstalled;
typedef void (*PulseAudio)(uint64_t, const uint8_t *, size_t, float);

// Modifier-only shortcuts cannot use Carbon's global-shortcut registrar.
// NSEvent's two monitors cover both other apps and Pulse without consuming keys.
typedef void (*PulseShortcut)(bool, bool, bool, double);
static id shortcutGlobalMonitor;
static id shortcutLocalMonitor;
static id shortcutSleepObserver;
static NSTimer *shortcutReleaseTimer;
static PulseShortcut shortcutCallback;
static bool shortcutRightDown;

bool pulse_dictation_right_option_down(void) {
    return CGEventSourceKeyState(kCGEventSourceStateCombinedSessionState, 0x3D);
}
static void PulseDictationShortcutEvent(NSEvent *event) {
    if (!shortcutCallback) return;
    CGEventRef cgEvent = event.CGEvent;
    if (cgEvent && CGEventGetIntegerValueField(cgEvent,kCGEventSourceUserData) == PulseDictationEventTag) return;
    if (deliveryElement && event.type == NSEventTypeKeyDown && sameInput()) deliveryInterrupted = true;
    NSEventModifierFlags flags = event.modifierFlags;
    // Device-specific bits distinguish the two Option keys, including when
    // both are held. The ordinary Option flag merges them and is insufficient.
    bool right = shortcutRightDown;
    if (event.type == NSEventTypeFlagsChanged && event.keyCode == 0x3D)
        right = (flags & NX_DEVICERALTKEYMASK) != 0;
    bool chord = event.type == NSEventTypeKeyDown ||
        (flags & (NSEventModifierFlagCommand | NSEventModifierFlagControl |
                  NSEventModifierFlagShift | NSEventModifierFlagFunction |
                  NX_DEVICELALTKEYMASK)) != 0;
    shortcutRightDown = right;
    shortcutCallback(right, chord, false, event.timestamp);
}
void pulse_dictation_unregister_shortcut(void) {
    shortcutCallback = NULL;
    if (shortcutGlobalMonitor) [NSEvent removeMonitor:shortcutGlobalMonitor];
    if (shortcutLocalMonitor) [NSEvent removeMonitor:shortcutLocalMonitor];
    if (shortcutSleepObserver) [NSWorkspace.sharedWorkspace.notificationCenter removeObserver:shortcutSleepObserver];
    [shortcutReleaseTimer invalidate];
    shortcutGlobalMonitor = nil; shortcutLocalMonitor = nil;
    shortcutSleepObserver = nil; shortcutReleaseTimer = nil; shortcutRightDown = false;
}
static void PulseDictationShortcutWatchdog(void) {
    if (!shortcutCallback) return;
    if (!AXIsProcessTrusted()) {
        shortcutRightDown = false; shortcutCallback(false, false, true, 0);
    }
}
bool pulse_dictation_register_shortcut(PulseShortcut callback) {
    pulse_dictation_unregister_shortcut();
    if (!AXIsProcessTrusted()) return false;
    shortcutCallback = callback;
    shortcutRightDown = pulse_dictation_right_option_down();
    NSEventMask mask = NSEventMaskFlagsChanged | NSEventMaskKeyDown;
    shortcutGlobalMonitor = [NSEvent addGlobalMonitorForEventsMatchingMask:mask handler:^(NSEvent *event) {
        PulseDictationShortcutEvent(event);
    }];
    shortcutLocalMonitor = [NSEvent addLocalMonitorForEventsMatchingMask:mask handler:^NSEvent *(NSEvent *event) {
        PulseDictationShortcutEvent(event);
        return event;
    }];
    if (!shortcutGlobalMonitor || !shortcutLocalMonitor) {
        pulse_dictation_unregister_shortcut(); return false;
    }
    shortcutSleepObserver = [NSWorkspace.sharedWorkspace.notificationCenter
        addObserverForName:NSWorkspaceWillSleepNotification object:nil queue:NSOperationQueue.mainQueue
        usingBlock:^(NSNotification *notification) {
            (void)notification;
            shortcutRightDown = false;
            if (shortcutCallback) shortcutCallback(false, false, true, 0);
        }];
    // Trust flagsChanged for release. A separate key-state query may return
    // false while the physical modifier is still held and must not commit an
    // empty recording. The timer only cancels on permission loss; sleep also
    // cancels above, and the recording lifecycle has a maximum duration.
    shortcutReleaseTimer = [NSTimer timerWithTimeInterval:0.1 repeats:YES block:^(NSTimer *timer) {
        (void)timer;
        PulseDictationShortcutWatchdog();
    }];
    [NSRunLoop.mainRunLoop addTimer:shortcutReleaseTimer forMode:NSRunLoopCommonModes];
    return true;
}

bool pulse_dictation_mic_allowed(void) {
    if (@available(macOS 10.14, *)) return [AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeAudio] == AVAuthorizationStatusAuthorized;
    return true; // macOS 10.13 predates microphone privacy authorization.
}
bool pulse_dictation_ax_allowed(void) { return AXIsProcessTrusted(); }
void pulse_dictation_request_access(bool microphone) {
    if (microphone) {
        if (@available(macOS 10.14, *)) {
        if ([AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeAudio] == AVAuthorizationStatusNotDetermined)
            [AVCaptureDevice requestAccessForMediaType:AVMediaTypeAudio completionHandler:^(BOOL granted) { (void)granted; }];
        else if (!pulse_dictation_mic_allowed())
            [NSWorkspace.sharedWorkspace openURL:[NSURL URLWithString:@"x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"]];
        }
    } else {
        NSDictionary *options = @{(__bridge NSString *)kAXTrustedCheckOptionPrompt: @YES};
        AXIsProcessTrustedWithOptions((__bridge CFDictionaryRef)options);
    }
}
static dispatch_queue_t PulseDictationAudioQueue(void) {
    static dispatch_queue_t queue;
    static dispatch_once_t once;
    dispatch_once(&once, ^{ queue = dispatch_queue_create("app.pulse.dictation.audio", DISPATCH_QUEUE_SERIAL); });
    return queue;
}
static void PulseDictationStopAudio(void) {
    if (tapInstalled) { [engine.inputNode removeTapOnBus:0]; tapInstalled = false; }
    [engine stop];
    engine = nil;
}
static bool PulseDictationStartAudio(uint64_t identifier, PulseAudio callback) {
    if (!pulse_dictation_mic_allowed()) return false;
    @try {
        engine = [[AVAudioEngine alloc] init];
        AVAudioInputNode *input = engine.inputNode;
        AVAudioFormat *format = [input outputFormatForBus:0];
        if (format.sampleRate <= 0 || format.channelCount == 0) { engine = nil; return false; }
        AVAudioFormat *output = [[AVAudioFormat alloc] initWithCommonFormat:AVAudioPCMFormatInt16 sampleRate:24000 channels:1 interleaved:YES];
        AVAudioConverter *converter = [[AVAudioConverter alloc] initFromFormat:format toFormat:output];
        if (!converter) { engine = nil; return false; }
        [input installTapOnBus:0 bufferSize:2048 format:format block:^(AVAudioPCMBuffer *buffer, AVAudioTime *when) {
            (void)when;
            @autoreleasepool {
                AVAudioFrameCount capacity = (AVAudioFrameCount)ceil(buffer.frameLength * 24000.0 / format.sampleRate) + 64;
                AVAudioPCMBuffer *pcm = [[AVAudioPCMBuffer alloc] initWithPCMFormat:output frameCapacity:capacity];
                __block BOOL supplied = NO;
                NSError *error = nil;
                AVAudioConverterOutputStatus status = [converter convertToBuffer:pcm error:&error withInputFromBlock:^AVAudioBuffer *(AVAudioPacketCount packets, AVAudioConverterInputStatus *inputStatus) {
                    (void)packets;
                    if (supplied) { *inputStatus = AVAudioConverterInputStatus_NoDataNow; return nil; }
                    supplied = YES;
                    *inputStatus = AVAudioConverterInputStatus_HaveData;
                    return buffer;
                }];
                if (status == AVAudioConverterOutputStatus_Error || error) { callback(identifier, NULL, 0, -1); return; }
                int16_t *samples = pcm.int16ChannelData[0];
                double power = 0;
                for (AVAudioFrameCount i = 0; i < pcm.frameLength; i++) { double value = samples[i] / 32768.0; power += value * value; }
                if (pcm.frameLength) callback(identifier, (uint8_t *)samples, pcm.frameLength * 2, sqrt(power / pcm.frameLength));
            }
        }];
        tapInstalled = true;
        NSError *error = nil;
        [engine prepare];
        if (![engine startAndReturnError:&error]) { PulseDictationStopAudio(); return false; }
        return true;
    } @catch (NSException *exception) { PulseDictationStopAudio(); return false; }
}
// Enqueue immediately from key-down, so release/cancel cannot overtake startup.
// The completion callback owns context and is invoked exactly once.
void pulse_dictation_start_async(uint64_t identifier, PulseAudio callback,
                                void (*ready)(void *, bool), void *context) {
    dispatch_async(PulseDictationAudioQueue(), ^{
        @autoreleasepool { ready(context, PulseDictationStartAudio(identifier, callback)); }
    });
}
void pulse_dictation_stop(void) {
    // Flush startup and the tap before Rust closes its PCM sender. No audio
    // lifecycle code may synchronously wait for AppKit from this queue.
    dispatch_sync(PulseDictationAudioQueue(), ^{ PulseDictationStopAudio(); });
}
static CFTypeRef attribute(AXUIElementRef element, CFStringRef name) {
    CFTypeRef result = NULL;
    AXUIElementCopyAttributeValue(element, name, &result);
    return result;
}
static AXUIElementRef focusedElement(void) {
    if (!AXIsProcessTrusted()) return NULL;
    AXUIElementRef system = AXUIElementCreateSystemWide();
    AXUIElementSetMessagingTimeout(system, 0.25);
    CFTypeRef value = attribute(system, kAXFocusedUIElementAttribute);
    CFRelease(system);
    if (value && CFGetTypeID(value) == AXUIElementGetTypeID()) return (AXUIElementRef)value;
    if (value) CFRelease(value);
    // Embedded browser editors can be absent from system-wide focus while the
    // active application still exposes the precise focused element. Query only
    // that app; never search other windows or follow an unrelated editor.
    pid_t pid=NSWorkspace.sharedWorkspace.frontmostApplication.processIdentifier;
    if (pid<=0) return NULL;
    AXUIElementRef app=AXUIElementCreateApplication(pid);
    AXUIElementSetMessagingTimeout(app,0.25);
    value=attribute(app,kAXFocusedUIElementAttribute);
    CFRelease(app);
    if (value && CFGetTypeID(value)==AXUIElementGetTypeID() &&
        NSWorkspace.sharedWorkspace.frontmostApplication.processIdentifier==pid) return (AXUIElementRef)value;
    if (value) CFRelease(value);
    return NULL;
}
static bool editable(AXUIElementRef element) {
    CFTypeRef role = attribute(element, kAXRoleAttribute);
    CFTypeRef subrole = attribute(element, kAXSubroleAttribute);
    CFTypeRef enabled = attribute(element, kAXEnabledAttribute);
    CFTypeRef readOnly = attribute(element, CFSTR("AXEditable"));
    Boolean selectedSettable = false, valueSettable = false;
    AXUIElementIsAttributeSettable(element, kAXSelectedTextAttribute, &selectedSettable);
    AXUIElementIsAttributeSettable(element, kAXValueAttribute, &valueSettable);
    bool textRole = role && (CFEqual(role, kAXTextFieldRole) || CFEqual(role, kAXTextAreaRole) || CFEqual(role, kAXComboBoxRole));
    bool valid = (selectedSettable || (textRole && valueSettable) || (textRole && readOnly && CFEqual(readOnly, kCFBooleanTrue)))
        && !(subrole && CFEqual(subrole, kAXSecureTextFieldSubrole))
        && !(enabled && CFEqual(enabled, kCFBooleanFalse))
        && !(readOnly && CFEqual(readOnly, kCFBooleanFalse));
    if (role) CFRelease(role);
    if (subrole) CFRelease(subrole);
    if (enabled) CFRelease(enabled);
    if (readOnly) CFRelease(readOnly);
    return valid;
}
// One capability-based path for all apps: replace the current selection when
// supported, otherwise use normal Unicode input. Never set an entire field or
// borrow the pasteboard. Read back the result before reporting delivery.
static NSString *insertionBase, *insertedText, *expectedValue, *pendingText, *pendingValue;
static NSRange originalSelection, expectedSelection, pendingSelection;
static CFTimeInterval pendingSince, unreadableSince;
static bool insertionUsable, insertionStarted;
static NSString *placeholderCandidate;
static bool textLength(AXUIElementRef element, CFIndex *length) {
    CFTypeRef value = attribute(element,kAXNumberOfCharactersAttribute);
    bool ok = value && CFGetTypeID(value)==CFNumberGetTypeID() &&
        CFNumberGetValue(value,kCFNumberCFIndexType,length) && *length>=0;
    if (value) CFRelease(value);
    return ok;
}
static bool readSelection(AXUIElementRef element, NSRange *range) {
    CFTypeRef value = attribute(element, kAXSelectedTextRangeAttribute);
    CFRange selected;
    bool ok = value && CFGetTypeID(value) == AXValueGetTypeID() &&
        AXValueGetValue(value, kAXValueCFRangeType, &selected) && selected.location >= 0 && selected.length >= 0;
    if (ok) {
        *range = NSMakeRange((NSUInteger)selected.location, (NSUInteger)selected.length);
        if (!range->length) {
            CFTypeRef raw=attribute(element,kAXValueAttribute);
            // Some empty contenteditable fields expose a synthetic paragraph
            // position at 1 despite AXValue and AXNumberOfCharacters being empty.
            // Normalize only this verified empty state, never nonempty ranges.
            if (range->location==1 && raw && CFGetTypeID(raw)==CFStringGetTypeID() && CFStringGetLength(raw)==0) {
                CFTypeRef count=attribute(element,kAXNumberOfCharactersAttribute);
                long length=-1;
                if (count && CFGetTypeID(count)==CFNumberGetTypeID() &&
                    CFNumberGetValue(count,kCFNumberLongType,&length) && length==0) range->location=0;
                if (count) CFRelease(count);
            }
            if (raw) CFRelease(raw);
        }
    }
    if (value) CFRelease(value);
    return ok;
}
static NSString *readText(AXUIElementRef element) {
    CFTypeRef value = NULL;
    AXError error = AXUIElementCopyAttributeValue(element,kAXValueAttribute,&value);
    if (value && CFGetTypeID(value) == CFStringGetTypeID()) {
        return CFBridgingRelease(value);
    }
    if (value) CFRelease(value);
    // Empty native composers (including Messages) omit AXValue. Other editors
    // expose their document only through the text-range API. Missing is not
    // empty: require an explicit character count, never a timed-out AX query.
    CFIndex length;
    if ((error==kAXErrorNoValue || error==kAXErrorAttributeUnsupported) && textLength(element,&length)) {
        if (length==0) return @"";
        CFRange all = CFRangeMake(0,length);
        AXValueRef range = AXValueCreate(kAXValueCFRangeType,&all);
        value = NULL;
        AXError read = AXUIElementCopyParameterizedAttributeValue(element,kAXStringForRangeParameterizedAttribute,range,&value);
        CFRelease(range);
        if (read==kAXErrorSuccess && value && CFGetTypeID(value)==CFStringGetTypeID() && CFStringGetLength(value)==length)
            return CFBridgingRelease(value);
        if (value) CFRelease(value);
    }
    return nil;
}
static bool textInput(AXUIElementRef element) {
    CFTypeRef role = attribute(element, kAXRoleAttribute);
    bool result = role && (CFEqual(role,kAXTextFieldRole) || CFEqual(role,kAXTextAreaRole) || CFEqual(role,kAXComboBoxRole));
    if (role) CFRelease(role);
    return result;
}
void pulse_dictation_clear_target(void) {
    if (deliveryElement) CFRelease(deliveryElement);
    if (deliveryWindow) CFRelease(deliveryWindow);
    deliveryWindow=NULL;
    deliveryElement = NULL; deliveryPID = 0;
    insertionBase = insertedText = expectedValue = pendingText = pendingValue = nil;
    insertionUsable = false; insertionStarted = false; deliveryInterrupted = false;
    placeholderCandidate=nil; unreadableSince=0;
}
// 0 = no input, 1 = safely readable/replaceable input, 2 = unsupported input.
int pulse_dictation_delivery_begin(void) {
    pulse_dictation_clear_target();
    AXUIElementRef element = focusedElement();
    if (!element) return 0;
    bool isInput = textInput(element) || editable(element);
    if (!isInput) { CFRelease(element); return 0; }
    deliveryElement = element;
    AXUIElementSetMessagingTimeout(element, 0.08);
    if (AXUIElementGetPid(element, &deliveryPID)!=kAXErrorSuccess || deliveryPID<=0) return 2;
    CFTypeRef window=attribute(element,kAXWindowAttribute);
    if (window && CFGetTypeID(window)==AXUIElementGetTypeID()) deliveryWindow=(AXUIElementRef)window;
    else if (window) CFRelease(window);
    if (deliveryWindow) AXUIElementSetMessagingTimeout(deliveryWindow,0.08);
    if (!editable(element)) return 2;
    insertionBase = readText(element);
    insertionUsable = insertionBase && readSelection(element,&originalSelection) &&
        originalSelection.location <= insertionBase.length && originalSelection.length <= insertionBase.length-originalSelection.location;
    if (!insertionUsable) return 2;
    // Web placeholders can be included in AXValue AND AX caret offsets.
    // Keep the original content until a real edit proves it was display-only:
    // the entire resulting document must equal precisely our inserted text.
    if (insertionBase.length && !originalSelection.length) placeholderCandidate=insertionBase;
    expectedValue = insertionBase; insertedText = @""; expectedSelection = originalSelection;
    return 1;
}
// Identity is pinned for final delivery. Capability reads may temporarily
// fail while an editor updates; check them before mutations, not as identity.
static bool sameInput(void) {
    AXUIElementRef focused = focusedElement();
    bool owner = NSWorkspace.sharedWorkspace.frontmostApplication.processIdentifier == deliveryPID;
    bool same = owner && focused && deliveryElement && CFEqual(focused,deliveryElement);
    if (same && deliveryWindow) {
        // Some apps keep reporting their last focused control after a different
        // window becomes key. Verify window ownership as well as element/PID.
        AXUIElementRef app=AXUIElementCreateApplication(deliveryPID);
        AXUIElementSetMessagingTimeout(app,0.08);
        CFTypeRef window=attribute(app,kAXFocusedWindowAttribute);
        if (window) { same=CFEqual(window,deliveryWindow); CFRelease(window); }
        CFRelease(app);
    }
    if (focused) CFRelease(focused);
    return same;
}
static bool typeUnicode(NSString *text) {
    if (!text.length) return false;
    CGEventSourceRef source = CGEventSourceCreate(kCGEventSourceStatePrivate);
    CGKeyCode key = 0;
    CGEventRef down = source ? CGEventCreateKeyboardEvent(source,key,true) : NULL;
    CGEventRef up = source ? CGEventCreateKeyboardEvent(source,key,false) : NULL;
    if (down && up) {
        if (text.length) {
            NSMutableData *characters=[NSMutableData dataWithLength:text.length*sizeof(UniChar)];
            [text getCharacters:characters.mutableBytes range:NSMakeRange(0,text.length)];
            CGEventKeyboardSetUnicodeString(down,text.length,characters.bytes);
            CGEventKeyboardSetUnicodeString(up,text.length,characters.bytes);
        }
        CGEventSetFlags(down,0); CGEventSetFlags(up,0);
        CGEventSetIntegerValueField(down,kCGEventSourceUserData,PulseDictationEventTag);
        CGEventSetIntegerValueField(up,kCGEventSourceUserData,PulseDictationEventTag);
        // Constrain delivery to the pinned process even if another app becomes
        // active between the last AX focus check and event dispatch.
        CGEventPostToPid(deliveryPID,down); CGEventPostToPid(deliveryPID,up);
    }
    bool sent = down && up;
    if (down) CFRelease(down);
    if (up) CFRelease(up);
    if (source) CFRelease(source);
    return sent;
}
static bool replaceSelection(NSString *text) {
    Boolean selectionWritable=false;
    if (AXUIElementIsAttributeSettable(deliveryElement,kAXSelectedTextAttribute,&selectionWritable)==kAXErrorSuccess && selectionWritable) {
        AXError result=AXUIElementSetAttributeValue(deliveryElement,kAXSelectedTextAttribute,(__bridge CFStringRef)text);
        // Success, timeout, and other ambiguous outcomes all require readback.
        // Never retry a write that may already have reached the editor.
        if (result!=kAXErrorAttributeUnsupported && result!=kAXErrorNotImplemented) return true;
        NSRange selection;
        if (deliveryInterrupted || !sameInput() || !editable(deliveryElement) ||
            ![readText(deliveryElement) isEqualToString:expectedValue] ||
            !readSelection(deliveryElement,&selection) || !NSEqualRanges(selection,expectedSelection)) return false;
    }
    return typeUnicode(text);
}
// AX value and selection are independent, asynchronous observations. A Unicode
// event can also be applied in several input events by the receiving editor.
// While waiting, never resend, adopt partial content, or write to another field.
// The exact expected document acknowledges delivery. Caret reporting may lag
// or disappear after a successful edit and must not trigger a clipboard copy.
static int awaitInsertionReadback(void) {
    CFTimeInterval now=CFAbsoluteTimeGetCurrent();
    if (!unreadableSince) unreadableSince=now;
    CFTimeInterval since=pendingValue ? pendingSince : unreadableSince;
    if (now-since < 0.8) return 0;
    insertionUsable=false;
    return -1;
}
// Called only after recording finishes: -1 = clipboard fallback, 0 = pending, 1 = inserted.
int pulse_dictation_final_step(const char *utf8) {
    if (!insertionUsable || deliveryInterrupted) { insertionUsable=false; return -1; }
    if (!sameInput()) { insertionUsable=false; return -1; }
    NSString *actual = readText(deliveryElement);
    if (!actual) return awaitInsertionReadback();
    if (pendingValue) {
        if (!insertionStarted && placeholderCandidate && ![actual isEqualToString:insertionBase] && [actual isEqualToString:pendingText]) {
            // The editor removed its placeholder, not user content. Rebase the
            // owned range to the verified empty document without typing twice.
            insertionBase=@""; expectedValue=@""; pendingValue=pendingText;
            originalSelection=NSMakeRange(0,0); pendingSelection=NSMakeRange(pendingText.length,0);
        }
        if ([actual isEqualToString:pendingValue]) {
            insertedText=pendingText; expectedValue=pendingValue; expectedSelection=pendingSelection; insertionStarted=true;
            pendingText=pendingValue=nil;
        } else return awaitInsertionReadback();
    }
    NSString *desired = [NSString stringWithUTF8String:utf8];
    if (!desired) { insertionUsable=false; return -1; }
    if (![actual isEqualToString:expectedValue]) { insertionUsable=false; return -1; }
    if ([desired isEqualToString:insertedText]) { unreadableSince=0; return 1; }
    if (insertionStarted) { insertionUsable=false; return -1; }
    NSRange selection;
    if (!readSelection(deliveryElement,&selection)) return awaitInsertionReadback();
    if (!NSEqualRanges(selection,expectedSelection)) { insertionUsable=false; return -1; }
    if (!editable(deliveryElement)) return awaitInsertionReadback();
    // Revalidate content and focus immediately before posting input.
    if (deliveryInterrupted) { insertionUsable=false; return -1; }
    if (!sameInput()) { insertionUsable=false; return -1; }
    if (![readText(deliveryElement) isEqualToString:expectedValue]) return awaitInsertionReadback();
    // Replace only the selected range with the complete final transcript.
    if (!replaceSelection(desired)) { insertionUsable=false; return -1; }
    pendingText=desired;
    pendingValue=[insertionBase stringByReplacingCharactersInRange:originalSelection withString:desired];
    pendingSelection=NSMakeRange(selection.location+desired.length,0);
    pendingSince=CFAbsoluteTimeGetCurrent();
    unreadableSince=0;
    return 0;
}
// Final clipboard delivery when the selected input is not verified and synced.

bool pulse_dictation_copy(const char *utf8) {
    NSString *text=[NSString stringWithUTF8String:utf8];
    if (!text.length) return false;
    NSPasteboard *board=NSPasteboard.generalPasteboard;
    [board clearContents];
    return [board setString:text forType:NSPasteboardTypeString];
}

// A click-through overlay stays above normal windows and follows Spaces.
// It never takes keyboard focus or tracks an input field.
void pulse_dictation_position(void *pointer, double *bottom) {
    NSWindow *window = (__bridge NSWindow *)pointer;
    NSScreen *chosen = nil;
    for (NSScreen *screen in NSScreen.screens) if (NSPointInRect(NSEvent.mouseLocation, screen.frame)) chosen = screen;
    if (!chosen) chosen = NSScreen.mainScreen;
    window.level = NSStatusWindowLevel;
    window.collectionBehavior = NSWindowCollectionBehaviorCanJoinAllSpaces | NSWindowCollectionBehaviorFullScreenAuxiliary | NSWindowCollectionBehaviorIgnoresCycle;
    window.ignoresMouseEvents = YES; window.hidesOnDeactivate = NO; window.hasShadow = NO;
    window.animationBehavior = NSWindowAnimationBehaviorNone;
    [window setFrame:chosen.frame display:YES animate:NO];
    // Leave room for the 24pt feather even when the Dock is hidden.
    *bottom = MAX(28, NSMinY(chosen.visibleFrame) - NSMinY(chosen.frame) + 16);
}

// Keep every backdrop below every native surface, and all native material
// below WKWebView. Otherwise two overlapping halos can frost the glass rim.
static NSView *PulseDictationMaterialHost(NSWindow *window) {
    NSView *content = window.contentView;
    if (!content) return nil;
    static char materialKey;
    NSView *host = objc_getAssociatedObject(window,&materialKey);
    if (!host) {
        host = [[NSView alloc] initWithFrame:content.bounds];
        host.autoresizingMask = NSViewWidthSizable | NSViewHeightSizable;
        [content addSubview:host positioned:NSWindowBelow relativeTo:nil];
        objc_setAssociatedObject(window,&materialKey,host,OBJC_ASSOCIATION_RETAIN_NONATOMIC);
    }
    return host;
}

// A reusable nine-slice alpha mask feathers a 24pt halo around each block.
// Stretch only the center: resizing text must not stretch the blur's edge width.
static const CGFloat PulseBackdropPadding = 24;
static CGImageRef PulseBackdropMask(void) {
    static CGImageRef image;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        enum { size = 78 };
        uint8_t pixels[size * size * 4];
        for (size_t y=0; y<size; y++) for (size_t x=0; x<size; x++) {
            // Signed distance from a 30pt rounded rect with a 14pt radius.
            CGFloat dx = MAX(fabs(x + 0.5 - size/2.0) - 1, 0);
            CGFloat dy = MAX(fabs(y + 0.5 - size/2.0) - 1, 0);
            CGFloat distance = MAX(hypot(dx,dy) - 14, 0);
            CGFloat t = MAX(0, 1 - distance/PulseBackdropPadding);
            uint8_t alpha = (uint8_t)lrint(255 * t*t*(3-2*t));
            for (size_t c=0; c<4; c++) pixels[(y*size+x)*4+c] = alpha;
        }
        CGColorSpaceRef color = CGColorSpaceCreateDeviceRGB();
        CGContextRef context = CGBitmapContextCreate(pixels,size,size,8,size*4,color,(CGBitmapInfo)kCGImageAlphaPremultipliedLast);
        image = CGBitmapContextCreateImage(context);
        CGContextRelease(context);
        CGColorSpaceRelease(color);
    });
    return image;
}
static void PulseDictationBackdrop(NSWindow *window, const void *key,
                                   double x, double y, double width, double height,
                                   double opacity, bool darkMode) {
    NSView *host = PulseDictationMaterialHost(window);
    if (!host) return;
    NSVisualEffectView *halo = objc_getAssociatedObject(window,key);
    if (!halo) {
        halo = [[NSVisualEffectView alloc] initWithFrame:NSZeroRect];
        halo.material = NSVisualEffectMaterialPopover;
        halo.blendingMode = NSVisualEffectBlendingModeBehindWindow;
        halo.state = NSVisualEffectStateActive;
        halo.wantsLayer = YES;
        CALayer *mask = [CALayer layer];
        mask.contents = (__bridge id)PulseBackdropMask();
        mask.contentsCenter = CGRectMake(38.0/78,38.0/78,2.0/78,2.0/78);
        halo.layer.mask = mask;
        [host addSubview:halo positioned:NSWindowBelow relativeTo:nil];
        objc_setAssociatedObject(window,key,halo,OBJC_ASSOCIATION_RETAIN_NONATOMIC);
    }
    halo.hidden = opacity<=0 || width<=0 || height<=0 || NSWorkspace.sharedWorkspace.accessibilityDisplayShouldReduceTransparency;
    if (halo.hidden) return;
    // This quiet backing follows the desktop appearance; the UI keeps its
    // contrasting inverse surface above it. No separate animation or delay.
    halo.appearance = [NSAppearance appearanceNamed:darkMode ? NSAppearanceNameDarkAqua : NSAppearanceNameAqua];
    NSRect block = NSMakeRect(x,host.isFlipped ? y : NSHeight(host.bounds)-y-height,width,height);
    [CATransaction begin];
    [CATransaction setDisableActions:YES];
    halo.frame = NSInsetRect(block,-PulseBackdropPadding,-PulseBackdropPadding);
    halo.alphaValue = opacity * 0.8;
    halo.layer.mask.frame = halo.bounds;
    [CATransaction commit];
}

// Back the bordered transcript box with the native desktop material.
void pulse_dictation_transcript_blur(void *pointer, double x, double y, double width, double height, double opacity, bool darkMode) {
    NSWindow *window=(__bridge NSWindow *)pointer;
    NSView *host=PulseDictationMaterialHost(window);
    if (!host) return;
    static char backdropKey;
    PulseDictationBackdrop(window,&backdropKey,x,y,width,height,opacity,darkMode);
    static char blurKey;
    NSVisualEffectView *blur=objc_getAssociatedObject(window,&blurKey);
    if (!blur) {
        blur=[[NSVisualEffectView alloc] initWithFrame:NSZeroRect];
        blur.material=NSVisualEffectMaterialPopover;
        blur.blendingMode=NSVisualEffectBlendingModeBehindWindow;
        blur.state=NSVisualEffectStateActive;
        blur.wantsLayer=YES;
        blur.layer.cornerCurve=kCACornerCurveContinuous;
        blur.layer.masksToBounds=YES;
        [host addSubview:blur positioned:NSWindowAbove relativeTo:nil];
        objc_setAssociatedObject(window,&blurKey,blur,OBJC_ASSOCIATION_RETAIN_NONATOMIC);
    }
    blur.appearance=[NSAppearance appearanceNamed:darkMode ? NSAppearanceNameAqua : NSAppearanceNameDarkAqua];
    blur.hidden=opacity<=0 || width<=0 || height<=0;
    if (blur.hidden) return;
    [CATransaction begin]; [CATransaction setDisableActions:YES];
    blur.frame=NSMakeRect(x,host.isFlipped ? y : NSHeight(host.bounds)-y-height,width,height);
    blur.alphaValue=opacity;
    blur.layer.cornerRadius=14;
    [CATransaction commit];
}

// Native material sits under the transparent WKWebView. DOM-measured geometry
// keeps the glass aligned with the bars through placement and pill morphs.
bool pulse_dictation_glass(void *pointer, double x, double y, double width, double height, double opacity, bool darkMode) {
    NSWindow *window=(__bridge NSWindow *)pointer;
    NSView *host=PulseDictationMaterialHost(window);
    if (!host) return false;
    static char backdropKey;
    PulseDictationBackdrop(window,&backdropKey,x,y,width,height,opacity,darkMode);
    static char glassKey;
    NSView *glass=objc_getAssociatedObject(window,&glassKey);
    if (!glass) {
#if __MAC_OS_X_VERSION_MAX_ALLOWED >= 260000
        if (@available(macOS 26.0, *)) {
            NSGlassEffectView *effect=[[NSGlassEffectView alloc] initWithFrame:NSZeroRect];
            // Clear is the optical Liquid Glass material: AppKit refracts the
            // real backdrop at the capsule rim rather than frosting its center.
            effect.style=NSGlassEffectViewStyleClear;
            glass=effect;
        }
#endif
        if (!glass) {
            NSVisualEffectView *effect=[[NSVisualEffectView alloc] initWithFrame:NSZeroRect];
            effect.material=NSVisualEffectMaterialPopover;
            effect.blendingMode=NSVisualEffectBlendingModeBehindWindow;
            effect.state=NSVisualEffectStateActive;
            effect.wantsLayer=YES;
            effect.layer.masksToBounds=YES;
            glass=effect;
        }
        [host addSubview:glass positioned:NSWindowAbove relativeTo:nil];
        objc_setAssociatedObject(window,&glassKey,glass,OBJC_ASSOCIATION_RETAIN_NONATOMIC);
    }
    // Use the inverse appearance, matching the CSS surface and waveform colors.
    glass.appearance=[NSAppearance appearanceNamed:darkMode ? NSAppearanceNameAqua : NSAppearanceNameDarkAqua];
#if __MAC_OS_X_VERSION_MAX_ALLOWED >= 260000
    if (@available(macOS 26.0, *)) {
        if ([glass isKindOfClass:NSGlassEffectView.class])
            ((NSGlassEffectView *)glass).tintColor=[NSColor colorWithWhite:darkMode ? 1.0 : 0.0 alpha:0.8];
    }
#endif
    glass.hidden=opacity<=0 || width<=0 || height<=0;
    if (glass.hidden) return true;
    CGFloat nativeY=host.isFlipped ? y : NSHeight(host.bounds)-y-height;
    // The webview supplies the presentation bounds on each animation frame.
    // Resize the actual glass lens (including its rim), never scale a snapshot
    // of it or add a second implicit animation that trails the waveform.
    [CATransaction begin];
    [CATransaction setDisableActions:YES];
    glass.frame=NSMakeRect(x,nativeY,width,height);
    glass.alphaValue=opacity;
#if __MAC_OS_X_VERSION_MAX_ALLOWED >= 260000
    if (@available(macOS 26.0, *)) {
        if ([glass isKindOfClass:NSGlassEffectView.class]) ((NSGlassEffectView *)glass).cornerRadius=height/2;
    }
#endif
    if ([glass isKindOfClass:NSVisualEffectView.class]) glass.layer.cornerRadius=height/2;
    [CATransaction commit];
    return true;
}
