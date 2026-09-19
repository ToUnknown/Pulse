#import <AppKit/AppKit.h>
#import <AVFoundation/AVFoundation.h>
#import <ApplicationServices/ApplicationServices.h>
#include <IOKit/hidsystem/IOLLEvent.h>
#include <stdbool.h>
#include <stdint.h>

// All lifecycle and AX operations run on AppKit's main thread. Only the tap
// callback runs on the audio thread; its buffers are copied before returning.
static AVAudioEngine *engine;
static AXUIElementRef deliveryElement;
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
// events exercise web editors' normal input path; AX only reads and selects the
// exact range owned by this dictation. Every asynchronous write is read back.
static NSString *liveBase, *liveApplied, *liveExpected, *pendingText, *pendingValue;
static NSRange liveOriginal, liveSelection, pendingSelection;
static CFTimeInterval pendingSince;
static bool liveUsable, liveStarted;
static NSString *livePlaceholderCandidate;
static bool livePlaceholderConfirmed;
static bool liveSelecting;
static NSRange requestedSelection;
static CFTimeInterval selectionSince;
static bool emptyDisplayValue(NSString *raw) {
    bool expectingEmpty = pendingValue ? pendingValue.length == 0 : liveExpected.length == 0;
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
    deliveryElement = NULL; deliveryPID = 0;
    liveBase = liveApplied = liveExpected = pendingText = pendingValue = nil;
    liveUsable = false; liveStarted = false; liveInterrupted = false;
    livePlaceholderCandidate=nil; livePlaceholderConfirmed=false; liveSelecting=false;
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
    AXUIElementGetPid(element, &deliveryPID);
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
// Global top-left coordinates. Prefer the input's upper edge, not the caret.
bool pulse_dictation_live_bounds(double *x, double *y, double *width, double *height) {
    CGRect rect;
    if (!deliveryElement || !elementBounds(deliveryElement,&rect) || rect.size.width <= 0 || rect.size.height <= 0) return false;
    *x=rect.origin.x; *y=rect.origin.y; *width=rect.size.width; *height=rect.size.height;
    return true;
}
static bool sameInput(void) {
    AXUIElementRef focused = focusedElement();
    bool same = focused && deliveryElement && CFEqual(focused,deliveryElement) &&
        NSWorkspace.sharedWorkspace.frontmostApplication.processIdentifier == deliveryPID && editable(focused);
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
        CGEventPost(kCGHIDEventTap,down); CGEventPost(kCGHIDEventTap,up);
    }
    bool sent = down && up;
    if (down) CFRelease(down);
    if (up) CFRelease(up);
    if (source) CFRelease(source);
    return sent;
}
// -1 = unsafe to continue, 0 = pending, 1 = synced, 2 = original input unfocused.
int pulse_dictation_live_step(const char *utf8) {
    if (!liveUsable || liveInterrupted) { liveUsable=false; return -1; }
    if (!sameInput()) return 2; // Pin the session; never follow focus to a new input.
    NSString *actual = readText(deliveryElement);
    NSRange selection;
    if (!actual || !readSelection(deliveryElement,&selection)) { liveUsable=false; return -1; }
    if (liveSelecting) {
        if (![actual isEqualToString:liveExpected]) { liveUsable=false; return -1; }
        if (NSEqualRanges(selection,requestedSelection)) { liveSelection=selection; liveSelecting=false; }
        else if (NSEqualRanges(selection,liveSelection) && CFAbsoluteTimeGetCurrent()-selectionSince < 0.8) return 0;
        else { liveUsable=false; return -1; }
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
        } else if (([actual isEqualToString:liveExpected] || [actual isEqualToString:pendingValue]) &&
                   (NSEqualRanges(selection,liveSelection) || NSEqualRanges(selection,pendingSelection)) &&
                   CFAbsoluteTimeGetCurrent()-pendingSince < 0.8) {
            return 0;
        } else { liveUsable=false; return -1; }
    }
    if (![actual isEqualToString:liveExpected] || !NSEqualRanges(selection,liveSelection)) { liveUsable=false; return -1; }
    NSString *desired = [NSString stringWithUTF8String:utf8];
    if (!desired) { liveUsable=false; return -1; }
    if ([desired isEqualToString:liveApplied]) return 1;
    NSUInteger common = 0, limit = MIN(desired.length,liveApplied.length);
    while (common < limit && [desired characterAtIndex:common] == [liveApplied characterAtIndex:common]) common++;
    // Do not split a surrogate pair, combining sequence, or emoji family.
    if (common < liveApplied.length) common = MIN(common,[liveApplied rangeOfComposedCharacterSequenceAtIndex:common].location);
    if (common < desired.length) common = MIN(common,[desired rangeOfComposedCharacterSequenceAtIndex:common].location);
    NSUInteger end = MIN(desired.length,common+12);
    if (end < desired.length) end = NSMaxRange([desired rangeOfComposedCharacterSequenceAtIndex:end-1]);
    NSString *next = [desired substringToIndex:end];
    NSRange replace = liveStarted ? NSMakeRange(liveOriginal.location+common,liveApplied.length-common) : liveOriginal;
    if (!NSEqualRanges(replace,selection)) {
        if (!selectOwnedRange(replace)) { liveUsable=false; return -1; }
        requestedSelection=replace; selectionSince=CFAbsoluteTimeGetCurrent(); liveSelecting=true;
        // AX writes may be acknowledged before the editor updates its range.
        // Read it back on the next tick; never type into an unconfirmed range.
        return 0;
    }
    // Revalidate content and focus after selecting, before posting input.
    if (liveInterrupted || !sameInput() || ![readText(deliveryElement) isEqualToString:liveExpected]) { liveUsable=false; return -1; }
    NSString *fragment = [next substringFromIndex:common];
    if (!typeUnicode(fragment)) { liveUsable=false; return -1; }
    pendingText=next;
    pendingValue=[liveBase stringByReplacingCharactersInRange:liveOriginal withString:next];
    pendingSelection=NSMakeRange(liveOriginal.location+next.length,0);
    pendingSince=CFAbsoluteTimeGetCurrent();
    return 0;
}
// Called only for a session that started with no input selected.
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
