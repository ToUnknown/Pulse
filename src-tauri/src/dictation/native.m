#import <AppKit/AppKit.h>
#import <AVFoundation/AVFoundation.h>
#import <ApplicationServices/ApplicationServices.h>
#import <QuartzCore/QuartzCore.h>
#import <objc/runtime.h>
#include <IOKit/hidsystem/IOLLEvent.h>
#include <stdbool.h>
#include <stdint.h>

// All lifecycle and AX operations run on AppKit's main thread. Only the tap
// callback runs on the audio thread; its buffers are copied before returning.
static AVAudioEngine *engine;
static AXUIElementRef deliveryElement, deliveryWindow, deliveryParent;
static NSString *deliveryIdentity, *deliveryIdentityAttribute;
static const int64_t PulseDictationEventTag = 0x50554c5345444943;
static bool liveInterrupted;
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
    if (deliveryElement && event.type == NSEventTypeKeyDown && sameInput()) liveInterrupted = true;
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
void pulse_dictation_stop(void) {
    if (tapInstalled) { [engine.inputNode removeTapOnBus:0]; tapInstalled = false; }
    [engine stop];
    engine = nil;
}
bool pulse_dictation_start(uint64_t identifier, PulseAudio callback) {
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
        if (![engine startAndReturnError:&error]) { pulse_dictation_stop(); return false; }
        return true;
    } @catch (NSException *exception) { pulse_dictation_stop(); return false; }
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
static bool elementBounds(AXUIElementRef element, CGRect *rect) {
    CFTypeRef position = attribute(element, kAXPositionAttribute);
    CFTypeRef size = attribute(element, kAXSizeAttribute);
    bool valid = position && size && CFGetTypeID(position) == AXValueGetTypeID() && CFGetTypeID(size) == AXValueGetTypeID()
        && AXValueGetValue(position, kAXValueCGPointType, &rect->origin) && AXValueGetValue(size, kAXValueCGSizeType, &rect->size);
    if (position) CFRelease(position);
    if (size) CFRelease(size);
    return valid;
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
// events exercise web editors' normal input path; AX restores only the owned
// range during a handoff. Every asynchronous write is read back.
static NSString *liveBase, *liveApplied, *liveExpected, *pendingText, *pendingValue;
static NSRange liveOriginal, liveSelection, pendingSelection;
static CFTimeInterval pendingSince, unreadableSince, recoverySince, lastRecoveryAttempt;
static bool liveRecovering;
static int withdrawal;
static CFTimeInterval withdrawalSince;
static NSString *withdrawalFrom;
static NSRange withdrawalRange;
static bool liveUsable, liveStarted;
static NSString *livePlaceholderCandidate;
static bool livePlaceholderConfirmed;
static bool liveSelecting;
static NSRange requestedSelection;
static CFTimeInterval selectionSince;
static bool emptyDisplayValue(NSString *raw) {
    bool expectingEmpty = withdrawal ? liveBase.length == 0 : pendingValue ? pendingValue.length == 0 : liveExpected.length == 0;
    if (liveBase.length || !(liveStarted || livePlaceholderConfirmed) || !expectingEmpty) return false;
    return (livePlaceholderConfirmed && [raw isEqualToString:livePlaceholderCandidate]) ||
        [raw stringByTrimmingCharactersInSet:NSCharacterSet.newlineCharacterSet].length == 0;
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
            if (raw && CFGetTypeID(raw)==CFStringGetTypeID() && range->location <= [(__bridge NSString *)raw length] && emptyDisplayValue((__bridge NSString *)raw)) *range=NSMakeRange(0,0);
            if (raw) CFRelease(raw);
        }
    }
    if (value) CFRelease(value);
    return ok;
}
static NSString *readText(AXUIElementRef element) {
    CFTypeRef value = attribute(element, kAXValueAttribute);
    if (value && CFGetTypeID(value) == CFStringGetTypeID()) {
        NSString *text = CFBridgingRelease(value);
        NSRange selected;
        if (emptyDisplayValue(text) &&
            readSelection(element,&selected) && NSEqualRanges(selected,NSMakeRange(0,0))) return @"";
        return text;
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
    if (deliveryParent) CFRelease(deliveryParent);
    deliveryParent=NULL; deliveryIdentity=deliveryIdentityAttribute=nil;
    withdrawal=0; withdrawalFrom=nil; withdrawalSince=0;
    deliveryWindow=NULL; liveRecovering=false; recoverySince=lastRecoveryAttempt=0;
    deliveryElement = NULL; deliveryPID = 0;
    liveBase = liveApplied = liveExpected = pendingText = pendingValue = nil;
    liveUsable = false; liveStarted = false; liveInterrupted = false;
    livePlaceholderCandidate=nil; livePlaceholderConfirmed=false; liveSelecting=false; unreadableSince=0;
}
// 0 = no input, 1 = safely readable/replaceable input, 2 = unsupported input.
int pulse_dictation_live_begin(void) {
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
    CFTypeRef parent=attribute(element,kAXParentAttribute);
    if (parent && CFGetTypeID(parent)==AXUIElementGetTypeID()) deliveryParent=(AXUIElementRef)parent;
    else if (parent) CFRelease(parent);
    for (NSString *name in @[@"AXIdentifier", @"AXDOMIdentifier"]) {
        CFTypeRef value=attribute(element,(__bridge CFStringRef)name);
        if (value && CFGetTypeID(value)==CFStringGetTypeID() && CFStringGetLength(value)>0) {
            deliveryIdentity=CFBridgingRelease(value); deliveryIdentityAttribute=name; break;
        }
        if (value) CFRelease(value);
    }
    if (!editable(element)) return 2;
    Boolean rangeSettable = false;
    AXUIElementIsAttributeSettable(element,kAXSelectedTextRangeAttribute,&rangeSettable);
    liveBase = readText(element);
    liveUsable = rangeSettable && liveBase && readSelection(element,&liveOriginal) &&
        liveOriginal.location <= liveBase.length && liveOriginal.length <= liveBase.length-liveOriginal.location;
    if (!liveUsable) return 2;
    // Web placeholders can be included in AXValue AND AX caret offsets.
    // Keep the original content until a real edit proves it was display-only:
    // the entire resulting document must equal precisely our inserted text.
    if (liveBase.length && !liveOriginal.length) livePlaceholderCandidate=liveBase;
    liveExpected = liveBase; liveApplied = @""; liveSelection = liveOriginal;
    return 1;
}
// Track the caret, including wrapping/growing editors, instead of centering
// on the entire field. Older AX implementations fall back to the field edge.
static bool boundsForRange(NSRange range, CGRect *rect) {
    CFRange r=CFRangeMake(range.location,range.length);
    AXValueRef value=AXValueCreate(kAXValueCFRangeType,&r);
    CFTypeRef result=NULL;
    AXUIElementCopyParameterizedAttributeValue(deliveryElement,kAXBoundsForRangeParameterizedAttribute,value,&result);
    CFRelease(value);
    bool ok=result && CFGetTypeID(result)==AXValueGetTypeID() && AXValueGetValue(result,kAXValueCGRectType,rect) &&
        isfinite(rect->origin.x) && isfinite(rect->origin.y) && isfinite(rect->size.width) && isfinite(rect->size.height) &&
        rect->size.height>0 && rect->size.width>=0;
    if (result) CFRelease(result);
    return ok;
}
bool pulse_dictation_live_bounds(double *x, double *y, double *width, double *height) {
    if (!deliveryElement) return false;
    CGRect rect;
    NSRange caret;
    if (readSelection(deliveryElement,&caret)) {
        NSUInteger end=caret.location;
        // Corrections may temporarily select an earlier word. Keep the pill at
        // the current dictation end rather than bouncing back to that selection.
        if (liveStarted) {
            NSString *actual=readText(deliveryElement);
            NSString *applied=pendingValue && [actual isEqualToString:pendingValue] ? pendingText : liveApplied;
            NSUInteger ownedEnd=liveOriginal.location+applied.length;
            if (ownedEnd<=actual.length) end=ownedEnd;
        }
        if (boundsForRange(NSMakeRange(end,0),&rect)) {
            *x=rect.origin.x; *y=rect.origin.y; *width=MAX(1,rect.size.width); *height=rect.size.height; return true;
        }
        NSString *text=readText(deliveryElement);
        if (end>0 && end<=text.length && boundsForRange([text rangeOfComposedCharacterSequenceAtIndex:end-1],&rect)) {
            *x=CGRectGetMaxX(rect); *y=rect.origin.y; *width=1; *height=rect.size.height; return true;
        }
    }
    if (!elementBounds(deliveryElement,&rect) || rect.size.width<=0 || rect.size.height<=0) return false;
    *x=rect.origin.x; *y=rect.origin.y; *width=rect.size.width; *height=rect.size.height;
    return true;
}
// Recreated controls may get a new AX object. Rebind only when the old one is
// invalid and a stable identifier, parent, window and process all still match.
// Similar text or screen coordinates alone never authorize a replacement.
static bool rebindOriginal(AXUIElementRef candidate) {
    if (!candidate || !deliveryIdentity || !deliveryParent || !deliveryWindow) return false;
    CFTypeRef oldRole=NULL;
    AXError oldStatus=AXUIElementCopyAttributeValue(deliveryElement,kAXRoleAttribute,&oldRole);
    if (oldRole) CFRelease(oldRole);
    if (oldStatus!=kAXErrorInvalidUIElement) return false;
    pid_t pid=0;
    if (AXUIElementGetPid(candidate,&pid)!=kAXErrorSuccess || pid!=deliveryPID || !editable(candidate)) return false;
    CFTypeRef identity=attribute(candidate,(__bridge CFStringRef)deliveryIdentityAttribute);
    CFTypeRef parent=attribute(candidate,kAXParentAttribute), window=attribute(candidate,kAXWindowAttribute);
    bool match=identity && CFGetTypeID(identity)==CFStringGetTypeID() && [(__bridge NSString *)identity isEqualToString:deliveryIdentity] &&
        parent && CFEqual(parent,deliveryParent) && window && CFEqual(window,deliveryWindow);
    if (identity) CFRelease(identity); if (parent) CFRelease(parent); if (window) CFRelease(window);
    if (!match) return false;
    CFRelease(deliveryElement); deliveryElement=(AXUIElementRef)CFRetain(candidate);
    AXUIElementSetMessagingTimeout(deliveryElement,0.08);
    liveRecovering=true; recoverySince=lastRecoveryAttempt=0;
    return true;
}
// Identity is pinned until the next verified handoff. Capability reads may temporarily
// fail while an editor updates; check them before mutations, not as identity.
static bool sameInput(void) {
    AXUIElementRef focused = focusedElement();
    bool owner = NSWorkspace.sharedWorkspace.frontmostApplication.processIdentifier == deliveryPID;
    bool same = owner && focused && deliveryElement && (CFEqual(focused,deliveryElement) || rebindOriginal(focused));
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
static bool selectOwnedRange(NSRange selection) {
    CFRange range = CFRangeMake(selection.location,selection.length);
    AXValueRef value = AXValueCreate(kAXValueCFRangeType,&range);
    AXError result = AXUIElementSetAttributeValue(deliveryElement,kAXSelectedTextRangeAttribute,value);
    CFRelease(value);
    return result == kAXErrorSuccess;
}
static bool typeUnicode(NSString *text) {
    CGEventSourceRef source = CGEventSourceCreate(kCGEventSourceStatePrivate);
    // Empty replacement deletes only our explicitly selected suffix.
    CGKeyCode key = text.length ? 0 : 0x33;
    CGEventRef down = source ? CGEventCreateKeyboardEvent(source,key,true) : NULL;
    CGEventRef up = source ? CGEventCreateKeyboardEvent(source,key,false) : NULL;
    if (down && up) {
        if (text.length) {
            UniChar characters[text.length];
            [text getCharacters:characters range:NSMakeRange(0,text.length)];
            CGEventKeyboardSetUnicodeString(down,text.length,characters);
            CGEventKeyboardSetUnicodeString(up,text.length,characters);
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
// Keep unchanged text in place, including text after a corrected word. Diff a
// bounded window of composed characters so long dictations cannot block AX.
static NSUInteger editChunkEnd(NSString *text, NSUInteger start, NSUInteger end) {
    NSUInteger stop=MIN(end,start+12);
    if (stop>start && stop<end) stop=NSMaxRange([text rangeOfComposedCharacterSequenceAtIndex:stop-1]);
    return stop;
}
static void nextLiveEdit(NSString *old, NSString *desired, NSRange *replace, NSString **fragment) {
    NSUInteger common=0, limit=MIN(old.length,desired.length);
    while (common<limit && [old characterAtIndex:common]==[desired characterAtIndex:common]) common++;
    if (common<old.length) common=MIN(common,[old rangeOfComposedCharacterSequenceAtIndex:common].location);
    if (common<desired.length) common=MIN(common,[desired rangeOfComposedCharacterSequenceAtIndex:common].location);
    NSUInteger oldEnd=old.length, newEnd=desired.length;
    while (oldEnd>common && newEnd>common) {
        NSRange a=[old rangeOfComposedCharacterSequenceAtIndex:oldEnd-1];
        NSRange b=[desired rangeOfComposedCharacterSequenceAtIndex:newEnd-1];
        if (a.location<common || b.location<common || ![[old substringWithRange:a] isEqualToString:[desired substringWithRange:b]]) break;
        oldEnd=a.location; newEnd=b.location;
    }
    if (oldEnd>common && newEnd>common) {
        // Find the first changed run, preserving common words between edits
        // (for example capitalization at the start and punctuation at the end).
        enum { Window=48 };
        NSMutableArray<NSString *> *a=[NSMutableArray new], *b=[NSMutableArray new];
        NSUInteger aOffsets[Window+1]={common}, bOffsets[Window+1]={common};
        while (a.count<Window && aOffsets[a.count]<oldEnd) {
            NSRange r=[old rangeOfComposedCharacterSequenceAtIndex:aOffsets[a.count]];
            [a addObject:[old substringWithRange:r]]; aOffsets[a.count]=NSMaxRange(r);
        }
        while (b.count<Window && bOffsets[b.count]<newEnd) {
            NSRange r=[desired rangeOfComposedCharacterSequenceAtIndex:bOffsets[b.count]];
            [b addObject:[desired substringWithRange:r]]; bOffsets[b.count]=NSMaxRange(r);
        }
        unsigned char matches[Window+1][Window+1]={0};
        for (NSInteger i=a.count-1;i>=0;i--) for (NSInteger j=b.count-1;j>=0;j--)
            matches[i][j]=[a[i] isEqualToString:b[j]] ? 1+matches[i+1][j+1] : MAX(matches[i+1][j],matches[i][j+1]);
        if (matches[0][0]) {
            NSUInteger i=0,j=0;
            while (i<a.count && j<b.count && ![a[i] isEqualToString:b[j]]) {
                if (matches[i+1][j]>=matches[i][j+1]) i++; else j++;
            }
            oldEnd=aOffsets[i]; newEnd=bOffsets[j];
        }
        // Large rewrites progress locally without first clearing the sentence.
        oldEnd=editChunkEnd(old,common,oldEnd);
    }
    newEnd=editChunkEnd(desired,common,newEnd);
    *replace=NSMakeRange(common,oldEnd-common);
    *fragment=[desired substringWithRange:NSMakeRange(common,newEnd-common)];
}
static bool requestLiveSelection(NSRange selection) {
    if (!selectOwnedRange(selection)) return false;
    requestedSelection=selection; selectionSince=CFAbsoluteTimeGetCurrent(); liveSelecting=true;
    return true;
}
// AX value and selection are independent, asynchronous observations. A Unicode
// event can also be applied in several input events by the receiving editor.
// While waiting, never resend, adopt partial content, or write to another field.
// Only the exact expected value AND caret can acknowledge our transaction.
static int awaitLiveReadback(void) {
    CFTimeInterval now=CFAbsoluteTimeGetCurrent();
    if (!unreadableSince) unreadableSince=now;
    CFTimeInterval since=pendingValue ? pendingSince : liveSelecting ? selectionSince : unreadableSince;
    if (now-since < 0.8) return 0;
    liveUsable=false;
    return -1;
}
// Withdraw only our verified edit when handing off to preview. Accessibility
// writes address the pinned control directly; never send deletion keystrokes to
// whichever app the user clicked. Restore any original selection we replaced.
static int withdrawLiveText(void) {
    if (!liveStarted && !pendingValue && !withdrawal) return 1;
    CFTimeInterval now=CFAbsoluteTimeGetCurrent();
    if (!withdrawalSince) withdrawalSince=now;
    if (now-withdrawalSince>=0.8) return -1;
    NSString *actual=readText(deliveryElement);
    if (!actual) return 0;
    if (!withdrawal) {
        if (pendingValue) {
            if (!liveStarted && livePlaceholderCandidate && [actual isEqualToString:pendingText]) {
                livePlaceholderConfirmed=true; liveBase=@""; liveOriginal=NSMakeRange(0,0); pendingValue=pendingText;
            }
            if (![actual isEqualToString:pendingValue]) {
                return 0; // Let partially applied Unicode input settle before withdrawing.
            }
            liveApplied=pendingText; liveExpected=pendingValue; liveStarted=true;
            pendingText=pendingValue=nil;
        }
        if (![actual isEqualToString:liveExpected]) return -1;
        withdrawalFrom=actual;
        withdrawalRange=NSMakeRange(liveOriginal.location,liveApplied.length);
        if (withdrawalRange.location>actual.length || withdrawalRange.length>actual.length-withdrawalRange.location) return -1;
        withdrawal=1; liveSelecting=false;
        if (!selectOwnedRange(withdrawalRange)) return -1;
        return 0;
    }
    if (withdrawal==2) {
        if ([actual isEqualToString:liveBase]) {
            liveApplied=@""; liveExpected=liveBase; liveSelection=liveOriginal;
            liveStarted=false; pendingText=pendingValue=nil; liveSelecting=false;
            withdrawal=0; withdrawalFrom=nil; withdrawalSince=0; unreadableSince=0;
            return 1;
        }
        return [actual isEqualToString:withdrawalFrom] ? 0 : -1;
    }
    if (![actual isEqualToString:withdrawalFrom]) return -1;
    NSRange selected;
    if (!readSelection(deliveryElement,&selected) || !NSEqualRanges(selected,withdrawalRange)) return 0;
    Boolean writable=false;
    AXUIElementIsAttributeSettable(deliveryElement,kAXSelectedTextAttribute,&writable);
    if (!writable) return -1;
    NSString *original=[liveBase substringWithRange:liveOriginal];
    if (![readText(deliveryElement) isEqualToString:withdrawalFrom]) return -1;
    AXError result=AXUIElementSetAttributeValue(deliveryElement,kAXSelectedTextAttribute,(__bridge CFStringRef)original);
    if (result!=kAXErrorSuccess) return -1;
    withdrawal=2;
    return 0; // Success means read-back on a subsequent tick, never just posting.
}
// Focus belongs to the user. Hand off to bottom preview without raising
// windows or moving focus. The outer transaction handles destination changes.
static int ensureLiveTarget(void) {
    bool focused=sameInput();
    if (!focused || withdrawal || withdrawalSince) {
        liveRecovering=true; recoverySince=lastRecoveryAttempt=0;
        int result=withdrawLiveText();
        if (result<0) { liveUsable=false; return -1; }
        if (!result) return 3; // Preview is visible while the old input is cleaned.
        if (!focused) return 2;
    }
    if (!liveRecovering) return 1;
    CFTimeInterval now=CFAbsoluteTimeGetCurrent();
    if (!recoverySince) { recoverySince=now; if(pendingValue)pendingSince=now; if(liveSelecting)selectionSince=now; }
    if (now-recoverySince>=0.8) { liveUsable=false; return -1; }
    NSString *actual=readText(deliveryElement);
    NSString *expected=pendingValue ?: liveExpected;
    if (pendingValue && !liveStarted && livePlaceholderCandidate && [actual isEqualToString:pendingText]) {
        livePlaceholderConfirmed=true; liveBase=@""; liveExpected=@""; pendingValue=pendingText;
        liveOriginal=NSMakeRange(0,0); pendingSelection=NSMakeRange(pendingText.length,0); expected=pendingValue;
    }
    if (!actual) return 0;
    if (![actual isEqualToString:expected]) {
        if (pendingValue) return awaitLiveReadback();
        liveUsable=false; return -1;
    }
    NSRange selection;
    if (!readSelection(deliveryElement,&selection) || !editable(deliveryElement)) return 0;
    NSRange owned=pendingValue ? pendingSelection : liveSelecting ? requestedSelection : liveSelection;
    if (!NSEqualRanges(selection,owned)) {
        if (now-lastRecoveryAttempt>=0.15) { lastRecoveryAttempt=now; selectOwnedRange(owned); }
        return 0;
    }
    liveRecovering=false; recoverySince=lastRecoveryAttempt=0;
    return 1;
}
// -1 = unsafe, 0 = pending, 1 = synced, 2 = preview, 3 = preview with withdrawal pending.
static int writeLiveText(const char *utf8) {
    if (!liveUsable || liveInterrupted) { liveUsable=false; return -1; }
    int target=ensureLiveTarget();
    if (target!=1) return target;
    NSString *actual = readText(deliveryElement);
    NSRange selection;
    if (!actual || !readSelection(deliveryElement,&selection)) return awaitLiveReadback();
    if (liveSelecting) {
        if (![actual isEqualToString:liveExpected]) return awaitLiveReadback();
        if (NSEqualRanges(selection,requestedSelection)) { liveSelection=selection; liveSelecting=false; }
        else return awaitLiveReadback();
    }
    if (pendingValue) {
        if (!liveStarted && livePlaceholderCandidate && [actual isEqualToString:pendingText] &&
            NSEqualRanges(selection,NSMakeRange(pendingText.length,0))) {
            // The editor removed its placeholder, not user content. Rebase the
            // owned range to the verified empty document without typing twice.
            livePlaceholderConfirmed=true; liveBase=@""; liveExpected=@""; pendingValue=pendingText;
            liveOriginal=NSMakeRange(0,0); pendingSelection=selection;
        }
        if ([actual isEqualToString:pendingValue] && NSEqualRanges(selection,pendingSelection)) {
            liveApplied=pendingText; liveExpected=pendingValue; liveSelection=pendingSelection; liveStarted=true;
            pendingText=pendingValue=nil;
        } else return awaitLiveReadback();
    }
    if (![actual isEqualToString:liveExpected] || !NSEqualRanges(selection,liveSelection)) { liveUsable=false; return -1; }
    NSString *desired = [NSString stringWithUTF8String:utf8];
    if (!desired) { liveUsable=false; return -1; }
    if ([desired isEqualToString:liveApplied]) {
        NSRange end=NSMakeRange(liveOriginal.location+liveApplied.length,0);
        if (!liveStarted || NSEqualRanges(selection,end)) { unreadableSince=0; return 1; }
        if (!editable(deliveryElement)) return awaitLiveReadback();
        if (!requestLiveSelection(end)) { liveUsable=false; return -1; }
        return 0;
    }
    NSRange change;
    NSString *fragment;
    nextLiveEdit(liveApplied,desired,&change,&fragment);
    NSString *next=[liveApplied stringByReplacingCharactersInRange:change withString:fragment];
    NSRange replace=liveStarted ? NSMakeRange(liveOriginal.location+change.location,change.length) : liveOriginal;
    if (!editable(deliveryElement)) return awaitLiveReadback();
    if (!NSEqualRanges(replace,selection)) {
        if (!requestLiveSelection(replace)) { liveUsable=false; return -1; }
        // AX writes may be acknowledged before the editor updates its range.
        // Read it back on the next tick; never type into an unconfirmed range.
        return 0;
    }
    // Revalidate content and focus after selecting, before posting input.
    if (liveInterrupted) { liveUsable=false; return -1; }
    if (!sameInput()) { liveRecovering=true; recoverySince=lastRecoveryAttempt=0; return 3; }
    if (![readText(deliveryElement) isEqualToString:liveExpected]) return awaitLiveReadback();
    if (!typeUnicode(fragment)) { liveUsable=false; return -1; }
    pendingText=next;
    pendingValue=[liveBase stringByReplacingCharactersInRange:liveOriginal withString:next];
    pendingSelection=NSMakeRange(replace.location+fragment.length,0);
    pendingSince=CFAbsoluteTimeGetCurrent();
    unreadableSince=0;
    return 0;
}
// A handoff is a transaction: withdraw the old owned edit, then capture the
// currently selected destination. The complete transcript remains in Rust and
// is replayed into that destination's own original selection exactly once.
int pulse_dictation_live_step(const char *utf8) {
    if (deliveryElement) {
        bool current=sameInput();
        // A failed withdrawal must not recapture the same field and duplicate
        // text that may still be present. Only a distinct selection can retry.
        if (current && !liveUsable) return 2;
        if (current && !withdrawal && !withdrawalSince) {
            return writeLiveText(utf8);
        }
        if (liveUsable && !liveInterrupted) {
            int result=withdrawLiveText();
            if (!result) return 3;
            if (result<0) { liveUsable=false; return -1; }
        }
        pulse_dictation_clear_target();
    }
    int mode=pulse_dictation_live_begin();
    if (mode!=1) return 2;
    if (!sameInput()) { pulse_dictation_clear_target(); return 2; }
    return writeLiveText(utf8);
}
// Final clipboard delivery when the selected input is not verified and synced.

bool pulse_dictation_copy(const char *utf8) {
    NSString *text=[NSString stringWithUTF8String:utf8];
    if (!text.length) return false;
    NSPasteboard *board=NSPasteboard.generalPasteboard;
    [board clearContents];
    return [board setString:text forType:NSPasteboardTypeString];
}

// Cover the target screen without becoming key so the pill can sit above
// the input while the original app keeps keyboard focus.
void pulse_dictation_position(void *pointer, double *originX, double *originY, double *bottom) {
    NSWindow *window = (__bridge NSWindow *)pointer;
    NSScreen *chosen = nil;
    CGFloat primaryTop = NSMaxY(NSScreen.screens.firstObject.frame);
    AXUIElementRef element = focusedElement();
    CGRect rect;
    if (element && elementBounds(element, &rect)) {
        NSPoint center = NSMakePoint(CGRectGetMidX(rect), primaryTop - CGRectGetMidY(rect));
        for (NSScreen *screen in NSScreen.screens) if (NSPointInRect(center, screen.frame)) chosen = screen;
    }
    if (element) CFRelease(element);
    if (!chosen) for (NSScreen *screen in NSScreen.screens) if (NSPointInRect(NSEvent.mouseLocation, screen.frame)) chosen = screen;
    if (!chosen) chosen = NSScreen.mainScreen;
    window.level = NSStatusWindowLevel;
    window.collectionBehavior = NSWindowCollectionBehaviorCanJoinAllSpaces | NSWindowCollectionBehaviorFullScreenAuxiliary | NSWindowCollectionBehaviorIgnoresCycle;
    window.ignoresMouseEvents = YES; window.hidesOnDeactivate = NO; window.hasShadow = NO;
    window.animationBehavior = NSWindowAnimationBehaviorNone;
    [window setFrame:chosen.frame display:YES animate:NO];
    *originX = NSMinX(chosen.frame); *originY = primaryTop - NSMaxY(chosen.frame);
    *bottom = MAX(20, NSMinY(chosen.visibleFrame) - NSMinY(chosen.frame) + 16);
}

// Native material sits under the transparent WKWebView. DOM-measured geometry
// keeps the glass aligned with the bars through placement and pill morphs.
bool pulse_dictation_glass(void *pointer, double x, double y, double width, double height, double opacity) {
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
        // Use a light material in both appearances so charcoal bars stay legible.
        glass.appearance=[NSAppearance appearanceNamed:NSAppearanceNameAqua];
        [host addSubview:glass positioned:NSWindowBelow relativeTo:nil];
        objc_setAssociatedObject(window,&glassKey,glass,OBJC_ASSOCIATION_RETAIN_NONATOMIC);
    }
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
