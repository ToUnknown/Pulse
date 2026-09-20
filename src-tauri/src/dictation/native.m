#import <AppKit/AppKit.h>
#import <AVFoundation/AVFoundation.h>
#import <ApplicationServices/ApplicationServices.h>
#import <QuartzCore/QuartzCore.h>
#import <objc/message.h>
#import <objc/runtime.h>
#include <IOKit/hidsystem/IOLLEvent.h>
#include <dlfcn.h>
#include <stdbool.h>
#include <stdint.h>

// AX and window operations run on AppKit's main thread. Audio lifecycle runs
// on a dedicated serial queue; tap buffers are copied before returning.
static AVAudioEngine *engine;
static const int64_t PulseDictationEventTag = 0x50554c5345444943;
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
    // Prefer the active app's current focus over a stale system-wide element.
    pid_t pid=NSWorkspace.sharedWorkspace.frontmostApplication.processIdentifier;
    if (pid<=0) return NULL;
    for (int attempt=0; attempt<2; attempt++) {
        AXUIElementRef root=attempt ? AXUIElementCreateSystemWide() : AXUIElementCreateApplication(pid);
        AXUIElementSetMessagingTimeout(root,0.25);
        CFTypeRef value=attribute(root,kAXFocusedUIElementAttribute);
        CFRelease(root);
        pid_t owner=0;
        if (value && CFGetTypeID(value)==AXUIElementGetTypeID() &&
            AXUIElementGetPid((AXUIElementRef)value,&owner)==kAXErrorSuccess && owner==pid &&
            NSWorkspace.sharedWorkspace.frontmostApplication.processIdentifier==pid) return (AXUIElementRef)value;
        if (value) CFRelease(value);
    }
    return NULL;
}
static bool editable(AXUIElementRef element) {
    CFTypeRef role = attribute(element, kAXRoleAttribute);
    CFTypeRef subrole = attribute(element, kAXSubroleAttribute);
    CFTypeRef enabled = attribute(element, kAXEnabledAttribute);
    CFTypeRef readOnly = attribute(element, CFSTR("AXEditable"));
    Boolean selectedSettable = false;
    AXUIElementIsAttributeSettable(element, kAXSelectedTextAttribute, &selectedSettable);
    bool textRole = role && (CFEqual(role, kAXTextFieldRole) || CFEqual(role, kAXTextAreaRole) || CFEqual(role, kAXComboBoxRole));
    bool valid = (textRole || selectedSettable || (readOnly && CFEqual(readOnly, kCFBooleanTrue)))
        && !(subrole && CFEqual(subrole, kAXSecureTextFieldSubrole))
        && !(enabled && CFEqual(enabled, kCFBooleanFalse))
        && !(readOnly && CFEqual(readOnly, kCFBooleanFalse));
    if (role) CFRelease(role);
    if (subrole) CFRelease(subrole);
    if (enabled) CFRelease(enabled);
    if (readOnly) CFRelease(readOnly);
    return valid;
}
static bool modifiersDown(void) {
    CGEventFlags flags=CGEventSourceFlagsState(kCGEventSourceStateCombinedSessionState);
    return (flags & (kCGEventFlagMaskAlternate | kCGEventFlagMaskCommand |
                     kCGEventFlagMaskControl | kCGEventFlagMaskShift)) != 0;
}
// 0 = wait for physical modifiers; 1 = submitted once; -1 = copy instead.
int pulse_dictation_final_step(const char *utf8) {
    if (modifiersDown()) return 0;
    NSString *text=[NSString stringWithUTF8String:utf8];
    if (!text.length) return -1;
    pid_t pid=NSWorkspace.sharedWorkspace.frontmostApplication.processIdentifier;
    AXUIElementRef element=focusedElement();
    if (!element) return -1;
    AXUIElementSetMessagingTimeout(element,0.08);
    bool input=editable(element);
    CFRelease(element);
    if (pid!=NSWorkspace.sharedWorkspace.frontmostApplication.processIdentifier) return 0;
    if (!input || pid<=0) return -1;
    CGEventSourceRef source=CGEventSourceCreate(kCGEventSourceStatePrivate);
    CGEventRef down=source ? CGEventCreateKeyboardEvent(source,0,true) : NULL;
    CGEventRef up=source ? CGEventCreateKeyboardEvent(source,0,false) : NULL;
    if (!down || !up) {
        if (down) CFRelease(down);
        if (up) CFRelease(up);
        if (source) CFRelease(source);
        return -1;
    }
    CGEventSetFlags(down,0); CGEventSetFlags(up,0);
    CGEventSetIntegerValueField(down,kCGEventSourceUserData,PulseDictationEventTag);
    CGEventSetIntegerValueField(up,kCGEventSourceUserData,PulseDictationEventTag);
    int result=0;
    if (!modifiersDown() && pid==NSWorkspace.sharedWorkspace.frontmostApplication.processIdentifier) {
        // Small UTF-16 packets avoid Quartz's long-string truncation. Submit
        // immediately, without paced typing, and never split a surrogate pair.
        for (NSUInteger offset=0; offset<text.length;) {
            UniChar characters[20];
            NSUInteger length=MIN((NSUInteger)20,text.length-offset);
            // Keep line separators inside a packet rather than at its start.
            while (length>1 && offset+length<text.length &&
                   [[NSCharacterSet newlineCharacterSet] characterIsMember:[text characterAtIndex:offset+length]]) length--;
            [text getCharacters:characters range:NSMakeRange(offset,length)];
            if (offset+length<text.length && (characters[length-1]&0xfc00)==0xd800) length--;
            CGEventKeyboardSetUnicodeString(down,length,characters);
            CGEventKeyboardSetUnicodeString(up,length,characters);
            CGEventPost(kCGHIDEventTap,down); CGEventPost(kCGHIDEventTap,up);
            offset+=length;
        }
        result=1;
    }
    CFRelease(down); CFRelease(up); CFRelease(source);
    return result;
}
// The input path never accesses the clipboard. Only fallback writes here.
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
// AppKit materials always bring their own color recipe. The outer backdrop
// needs only a Gaussian filter, so use the compositor directly. These private
// capabilities are optional: an unsupported OS gets no outer effect, not tint.
static bool PulseEnableDesktopBackdrop(NSWindow *window) {
    static char configuredKey;
    NSNumber *configured=objc_getAssociatedObject(window,&configuredKey);
    if (configured) return configured.boolValue;
    bool enabled=false;
    id oldFlatten=nil, oldHosting=nil;
    @try {
        oldFlatten=[window valueForKey:@"shouldAutoFlattenLayerTree"];
        oldHosting=[window valueForKey:@"canHostLayersInWindowServer"];
        [window setValue:@NO forKey:@"shouldAutoFlattenLayerTree"];
        [window setValue:@NO forKey:@"canHostLayersInWindowServer"];
        [window setValue:@YES forKey:@"canHostLayersInWindowServer"];
        // A nonzero alpha keeps the transparent window's backing surface live.
        window.opaque=NO;
        window.backgroundColor=[NSColor colorWithWhite:0 alpha:0.001];
        enabled=true;
    } @catch (NSException *exception) {
        (void)exception;
        @try {
            if (oldFlatten) [window setValue:oldFlatten forKey:@"shouldAutoFlattenLayerTree"];
            if (oldHosting) [window setValue:oldHosting forKey:@"canHostLayersInWindowServer"];
        } @catch (NSException *restoreException) { (void)restoreException; }
    }
    objc_setAssociatedObject(window,&configuredKey,@(enabled),OBJC_ASSOCIATION_RETAIN_NONATOMIC);
    return enabled;
}
static void PulseKeepBackdropLive(NSWindow *window) {
    // Keep WindowServer from flattening the blur during Spaces transitions.
    // Window resize may reset this tag; do not send an IPC on every UI frame.
    typedef int32_t (*ConnectionID)(void);
    typedef int32_t (*SetTags)(int32_t,int32_t,const uint32_t *,int32_t);
    static ConnectionID connection;
    static SetTags setTags;
    static char taggedSizeKey;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        connection=(ConnectionID)dlsym(RTLD_DEFAULT,"CGSMainConnectionID");
        setTags=(SetTags)dlsym(RTLD_DEFAULT,"CGSSetWindowTags");
    });
    if (connection && setTags && window.windowNumber>0) {
        NSValue *taggedSize=objc_getAssociatedObject(window,&taggedSizeKey);
        NSSize size=window.frame.size;
        if (taggedSize && NSEqualSizes(taggedSize.sizeValue,size)) return;
        uint32_t tags[2]={0,1u<<16};
        if (setTags(connection(),(int32_t)window.windowNumber,tags,64)==0)
            objc_setAssociatedObject(window,&taggedSizeKey,[NSValue valueWithSize:size],OBJC_ASSOCIATION_RETAIN_NONATOMIC);
    }
}
static NSView *PulseCreateUntintedBackdrop(NSWindow *window) {
    @try {
        Class backdropClass=NSClassFromString(@"CABackdropLayer");
        Class filterClass=NSClassFromString(@"CAFilter");
        SEL factory=NSSelectorFromString(@"filterWithType:");
        if (!backdropClass || ![backdropClass isSubclassOfClass:CALayer.class] ||
            ![filterClass respondsToSelector:factory]) return nil;
        id filter=((id (*)(id,SEL,id))objc_msgSend)(filterClass,factory,@"gaussianBlur");
        if (!filter) return nil;
        [filter setValue:@18 forKey:@"inputRadius"];
        [filter setValue:@YES forKey:@"inputNormalizeEdges"];
        CALayer *backdrop=[backdropClass layer];
        if (!backdrop) return nil;
        [backdrop setValue:@YES forKey:@"windowServerAware"];
        [backdrop setValue:@YES forKey:@"allowsGroupBlending"];
        [backdrop setValue:@YES forKey:@"allowsGroupOpacity"];
        [backdrop setValue:@YES forKey:@"ignoresOffscreenGroups"];
        [backdrop setValue:@NO forKey:@"allowsInPlaceFiltering"];
        [backdrop setValue:@YES forKey:@"disablesOccludedBackdropBlurs"];
        [backdrop setValue:@0.25 forKey:@"scale"];
        [backdrop setValue:@24 forKey:@"bleedAmount"];
        [backdrop setValue:[NSString stringWithFormat:@"PulseDictation-%ld",(long)window.windowNumber] forKey:@"groupName"];
        backdrop.backgroundColor=NULL;
        backdrop.filters=@[filter];
        if (!PulseEnableDesktopBackdrop(window)) return nil;
        NSView *view=[[NSView alloc] initWithFrame:NSZeroRect];
        view.wantsLayer=YES;
        view.layer.backgroundColor=NULL;
        view.layer.sublayers=@[backdrop];
        CALayer *mask=[CALayer layer];
        mask.contents=(__bridge id)PulseBackdropMask();
        mask.contentsCenter=CGRectMake(38.0/78,38.0/78,2.0/78,2.0/78);
        view.layer.mask=mask;
        return view;
    } @catch (NSException *exception) {
        (void)exception;
        return nil;
    }
}
static void PulseDictationBackdrop(NSWindow *window, const void *key,
                                   double x, double y, double width, double height,
                                   double opacity, bool darkMode) {
    (void)darkMode;
    NSView *host = PulseDictationMaterialHost(window);
    if (!host) return;
    id saved=objc_getAssociatedObject(window,key);
    if (saved==NSNull.null) return;
    NSView *halo=saved;
    if (!halo) {
        halo=PulseCreateUntintedBackdrop(window);
        if (!halo) {
            objc_setAssociatedObject(window,key,NSNull.null,OBJC_ASSOCIATION_RETAIN_NONATOMIC);
            return;
        }
        [host addSubview:halo positioned:NSWindowBelow relativeTo:nil];
        objc_setAssociatedObject(window,key,halo,OBJC_ASSOCIATION_RETAIN_NONATOMIC);
    }
    halo.hidden = opacity<=0 || width<=0 || height<=0 || NSWorkspace.sharedWorkspace.accessibilityDisplayShouldReduceTransparency;
    if (halo.hidden) return;
    PulseKeepBackdropLive(window);
    NSRect block = NSMakeRect(x,host.isFlipped ? y : NSHeight(host.bounds)-y-height,width,height);
    [CATransaction begin];
    [CATransaction setDisableActions:YES];
    halo.frame = NSInsetRect(block,-PulseBackdropPadding,-PulseBackdropPadding);
    halo.alphaValue = opacity;
    halo.layer.mask.frame = halo.bounds;
    halo.layer.sublayers.firstObject.frame = halo.bounds;
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
