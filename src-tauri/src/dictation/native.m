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
static CFTypeRef deliveryRange;
static pid_t deliveryPID;
static bool tapInstalled;
typedef void (*PulseAudio)(uint64_t, const uint8_t *, size_t, float);

// Modifier-only shortcuts cannot use Carbon's global-shortcut registrar.
// NSEvent's two monitors cover both other apps and Pulse without consuming keys.
typedef void (*PulseShortcut)(bool, bool, bool);
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
    NSEventModifierFlags flags = event.modifierFlags;
    // Device-specific bits distinguish the two Option keys, including when
    // both are held. The ordinary Option flag merges them and is insufficient.
    bool right = (flags & NX_DEVICERALTKEYMASK) != 0;
    bool chord = event.type == NSEventTypeKeyDown ||
        (flags & (NSEventModifierFlagCommand | NSEventModifierFlagControl |
                  NSEventModifierFlagShift | NSEventModifierFlagFunction |
                  NX_DEVICELALTKEYMASK)) != 0;
    shortcutRightDown = right;
    shortcutCallback(right, chord, false);
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
            if (shortcutCallback) shortcutCallback(false, false, true);
        }];
    // Recover a missed release (for example across an app switch), and stop
    // recording if Accessibility is revoked while the key is held.
    shortcutReleaseTimer = [NSTimer timerWithTimeInterval:0.1 repeats:YES block:^(NSTimer *timer) {
        (void)timer;
        if (!shortcutCallback || !shortcutRightDown) return;
        if (!AXIsProcessTrusted()) {
            shortcutRightDown = false; shortcutCallback(false, false, true);
        } else if (!pulse_dictation_right_option_down()) {
            shortcutRightDown = false; shortcutCallback(false, false, false);
        }
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
void pulse_dictation_clear_target(void) {
    if (deliveryElement) CFRelease(deliveryElement);
    if (deliveryRange) CFRelease(deliveryRange);
    deliveryElement = NULL; deliveryRange = NULL; deliveryPID = 0;
}
// CGRect output is in global top-left desktop points (not backing pixels).
bool pulse_dictation_target(double *x, double *y, double *width, double *height) {
    pulse_dictation_clear_target();
    AXUIElementRef element = focusedElement();
    if (!element) return false;
    AXUIElementSetMessagingTimeout(element, 0.25);
    if (!editable(element)) { CFRelease(element); return false; }
    deliveryElement = element;
    AXUIElementGetPid(element, &deliveryPID);
    deliveryRange = attribute(element, kAXSelectedTextRangeAttribute);
    CGRect rect = CGRectZero;
    CFTypeRef bounds = NULL;
    bool hasBounds = deliveryRange && AXUIElementCopyParameterizedAttributeValue(element, kAXBoundsForRangeParameterizedAttribute, deliveryRange, &bounds) == kAXErrorSuccess
        && bounds && CFGetTypeID(bounds) == AXValueGetTypeID() && AXValueGetValue(bounds, kAXValueCGRectType, &rect);
    if (bounds) CFRelease(bounds);
    if (!hasBounds || rect.size.height <= 0) hasBounds = elementBounds(element, &rect);
    if (hasBounds) { *x = rect.origin.x; *y = rect.origin.y; *width = MAX(2, rect.size.width); *height = MAX(18, rect.size.height); }
    return hasBounds;
}
// 1 = AX insertion, 2 = clipboard, 3 = paste dispatched with clipboard backup.
int pulse_dictation_deliver(const char *utf8) {
    NSString *text = [NSString stringWithUTF8String:utf8];
    if (!text.length) return 0;
    AXUIElementRef focused = focusedElement();
    CFTypeRef range = focused ? attribute(focused, kAXSelectedTextRangeAttribute) : NULL;
    bool same = focused && deliveryElement && CFEqual(focused, deliveryElement)
        && NSWorkspace.sharedWorkspace.frontmostApplication.processIdentifier == deliveryPID
        && (!deliveryRange || (range && CFEqual(range, deliveryRange))) && editable(focused);
    if (range) CFRelease(range);
    if (focused) CFRelease(focused);
    if (same && AXUIElementSetAttributeValue(deliveryElement, kAXSelectedTextAttribute, (__bridge CFStringRef)text) == kAXErrorSuccess) {
        pulse_dictation_clear_target(); return 1;
    }
    NSPasteboard *board = NSPasteboard.generalPasteboard;
    [board clearContents];
    if (![board setString:text forType:NSPasteboardTypeString]) { pulse_dictation_clear_target(); return 0; }
    if (same) {
        CGEventRef down = CGEventCreateKeyboardEvent(NULL, 9, true);
        CGEventRef up = CGEventCreateKeyboardEvent(NULL, 9, false);
        bool sent = down && up;
        if (sent) {
            CGEventSetFlags(down, kCGEventFlagMaskCommand); CGEventSetFlags(up, kCGEventFlagMaskCommand);
            CGEventPostToPid(deliveryPID, down); CGEventPostToPid(deliveryPID, up);
        }
        if (down) CFRelease(down);
        if (up) CFRelease(up);
        pulse_dictation_clear_target(); return sent ? 3 : 2;
    }
    pulse_dictation_clear_target(); return 2;
}
// Cover the target's screen without becoming key. A full transparent canvas
// lets the transcript travel to the caret without moving a focused window.
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
