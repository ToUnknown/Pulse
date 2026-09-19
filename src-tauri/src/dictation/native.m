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
// Delivery never uses the pasteboard when an input was selected. Unicode key
// events exercise web editors' normal input path after recording finishes.
// Every asynchronous write is read back before reporting delivery.
static NSString *insertionBase, *insertedText, *expectedValue, *pendingText, *pendingValue;
static NSRange originalSelection, expectedSelection, pendingSelection;
static CFTimeInterval pendingSince, unreadableSince;
static bool insertionUsable, insertionStarted;
static NSString *placeholderCandidate;
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
    CFTypeRef value = attribute(element, kAXValueAttribute);
    if (value && CFGetTypeID(value) == CFStringGetTypeID()) {
        return CFBridgingRelease(value);
    }
    if (value) CFRelease(value);
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
// AX value and selection are independent, asynchronous observations. A Unicode
// event can also be applied in several input events by the receiving editor.
// While waiting, never resend, adopt partial content, or write to another field.
// Only the exact expected value AND caret can acknowledge our transaction.
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
    NSRange selection;
    if (!actual || !readSelection(deliveryElement,&selection)) return awaitInsertionReadback();
    if (pendingValue) {
        if (!insertionStarted && placeholderCandidate && [actual isEqualToString:pendingText] &&
            NSEqualRanges(selection,NSMakeRange(pendingText.length,0))) {
            // The editor removed its placeholder, not user content. Rebase the
            // owned range to the verified empty document without typing twice.
            insertionBase=@""; expectedValue=@""; pendingValue=pendingText;
            originalSelection=NSMakeRange(0,0); pendingSelection=selection;
        }
        if ([actual isEqualToString:pendingValue] && NSEqualRanges(selection,pendingSelection)) {
            insertedText=pendingText; expectedValue=pendingValue; expectedSelection=pendingSelection; insertionStarted=true;
            pendingText=pendingValue=nil;
        } else return awaitInsertionReadback();
    }
    if (![actual isEqualToString:expectedValue] || !NSEqualRanges(selection,expectedSelection)) { insertionUsable=false; return -1; }
    NSString *desired = [NSString stringWithUTF8String:utf8];
    if (!desired) { insertionUsable=false; return -1; }
    if ([desired isEqualToString:insertedText]) { unreadableSince=0; return 1; }
    if (insertionStarted) { insertionUsable=false; return -1; }
    if (!editable(deliveryElement)) return awaitInsertionReadback();
    // Revalidate content and focus immediately before posting input.
    if (deliveryInterrupted) { insertionUsable=false; return -1; }
    if (!sameInput()) { insertionUsable=false; return -1; }
    if (![readText(deliveryElement) isEqualToString:expectedValue]) return awaitInsertionReadback();
    // Send the complete final transcript once, without paced typing batches.
    if (!typeUnicode(desired)) { insertionUsable=false; return -1; }
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
    *bottom = MAX(20, NSMinY(chosen.visibleFrame) - NSMinY(chosen.frame) + 16);
}

// Back the bordered transcript box with the native desktop material.
void pulse_dictation_transcript_blur(void *pointer, double x, double y, double width, double height, double opacity, bool darkMode) {
    NSWindow *window=(__bridge NSWindow *)pointer;
    NSView *host=window.contentView;
    if (!host) return;
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
        [host addSubview:blur positioned:NSWindowBelow relativeTo:nil];
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
    NSView *host=window.contentView;
    if (!host) return false;
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
        [host addSubview:glass positioned:NSWindowBelow relativeTo:nil];
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
