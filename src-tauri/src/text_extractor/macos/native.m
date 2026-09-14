#import "native.h"
#import <AppKit/AppKit.h>
#import <CoreText/CoreText.h>
#import <ScreenCaptureKit/ScreenCaptureKit.h>
#import <Vision/Vision.h>
#import <unistd.h>

static bool PulseError(char **error, NSString *message) {
    if (error) *error = strdup(message.UTF8String ?: "The native operation failed.");
    return false;
}
void pulse_native_free(void *pointer) { free(pointer); }

bool pulse_capture_supported(void) {
    if (@available(macOS 14.0, *)) return true;
    return false;
}
bool pulse_capture_allowed(void) {
    if (@available(macOS 14.0, *)) return CGPreflightScreenCaptureAccess();
    return false;
}
bool pulse_request_capture_access(void) {
    // Called on the main thread only in response to the Settings permission button.
    if (@available(macOS 14.0, *)) {
        if (CGPreflightScreenCaptureAccess() || CGRequestScreenCaptureAccess()) return true;
        [[NSWorkspace sharedWorkspace] openURL:[NSURL URLWithString:
            @"x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"]];
    }
    return false;
}

static PulseMonitor PulseScreen(NSScreen *screen) {
    NSRect frame = screen.frame;
    CGFloat primaryTop = NSMaxY(NSScreen.screens.firstObject.frame);
    CGFloat scale = screen.backingScaleFactor;
    return (PulseMonitor){ frame.origin.x, primaryTop - NSMaxY(frame), scale,
        (uint32_t)llround(frame.size.width * scale),
        (uint32_t)llround(frame.size.height * scale),
        [screen.deviceDescription[@"NSScreenNumber"] unsignedIntValue] };
}
static NSScreen *PulseFindScreen(uint32_t displayID) {
    for (NSScreen *screen in NSScreen.screens) {
        if ([screen.deviceDescription[@"NSScreenNumber"] unsignedIntValue] == displayID) return screen;
    }
    return nil;
}
bool pulse_monitor_at_pointer(PulseMonitor *monitor, char **error) {
    NSPoint pointer = NSEvent.mouseLocation;
    for (NSScreen *screen in NSScreen.screens) {
        if (NSPointInRect(pointer, screen.frame)) {
            *monitor = PulseScreen(screen);
            if (!monitor->width || !monitor->height ||
                (uint64_t)monitor->width * monitor->height > 40000000) {
                return PulseError(error, @"This screen is too large to capture. Reduce its resolution and try again.");
            }
            return true;
        }
    }
    return PulseError(error, @"Could not locate the screen under the pointer.");
}
void pulse_configure_overlay(void *pointer) {
    NSWindow *window = (__bridge NSWindow *)pointer;
    window.animationBehavior = NSWindowAnimationBehaviorNone;
    window.level = NSScreenSaverWindowLevel;
    window.collectionBehavior = NSWindowCollectionBehaviorCanJoinAllSpaces |
        NSWindowCollectionBehaviorFullScreenAuxiliary | NSWindowCollectionBehaviorStationary |
        NSWindowCollectionBehaviorIgnoresCycle;
    window.hidesOnDeactivate = NO;
    window.hasShadow = NO;
}
bool pulse_position_overlay(void *pointer, PulseMonitor monitor, bool notice, char **error) {
    NSScreen *screen = PulseFindScreen(monitor.display_id);
    if (!screen) return PulseError(error, @"This screen disconnected. Start a new selection.");
    PulseMonitor current = PulseScreen(screen);
    if (current.width != monitor.width || current.height != monitor.height ||
        current.scale != monitor.scale || current.x != monitor.x || current.y != monitor.y) {
        return PulseError(error, @"This screen changed. Start a new selection.");
    }
    NSWindow *window = (__bridge NSWindow *)pointer;
    NSRect frame = screen.frame;
    if (notice) {
        // Keep the notice below the menu bar/notch, on the screen that was captured.
        frame = NSMakeRect(NSMidX(frame) - 120, NSMaxY(screen.visibleFrame) - 88 - 8, 240, 88);
    }
    [window setFrame:frame display:YES animate:NO];
    return true;
}

bool pulse_capture_screen(PulseMonitor monitor, uint8_t **pixels, char **error) {
    @autoreleasepool {
        if (@available(macOS 14.0, *)) {
            if (!pulse_capture_allowed()) return PulseError(error, @"Allow Screen Recording for Pulse in System Settings, then try again.");
            // ScreenCaptureKit callbacks outlive timeouts safely: ARC owns the result
            // holder and semaphore; no callback writes to a caller's stack or buffer.
            dispatch_semaphore_t ready = dispatch_semaphore_create(0);
            __block NSBitmapImageRep *captured = nil;
            __block NSString *failure = nil;
            [SCShareableContent getShareableContentExcludingDesktopWindows:NO onScreenWindowsOnly:YES
                completionHandler:^(SCShareableContent *content, NSError *contentError) {
                if (contentError || !content) {
                    failure = @"Could not read screen content. Check Pulse's Screen Recording permission.";
                    dispatch_semaphore_signal(ready);
                    return;
                }
                SCDisplay *display = nil;
                for (SCDisplay *candidate in content.displays) {
                    if (candidate.displayID == monitor.display_id) { display = candidate; break; }
                }
                if (!display || fabs(display.frame.size.width * monitor.scale - monitor.width) > 1 ||
                    fabs(display.frame.size.height * monitor.scale - monitor.height) > 1 ||
                    fabs(display.frame.origin.x - monitor.x) > 1 || fabs(display.frame.origin.y - monitor.y) > 1) {
                    failure = @"This screen changed. Start a new selection.";
                    dispatch_semaphore_signal(ready);
                    return;
                }
                NSMutableArray<SCWindow *> *excluded = [NSMutableArray array];
                for (SCWindow *window in content.windows) {
                    if (window.owningApplication.processID == getpid() &&
                        ([window.title isEqualToString:@"Pulse Text Extractor"] ||
                         [window.title isEqualToString:@"Pulse Quick Copy Notice"])) {
                        [excluded addObject:window];
                    }
                }
                SCContentFilter *filter = [[SCContentFilter alloc] initWithDisplay:display excludingWindows:excluded];
                if (@available(macOS 14.2, *)) filter.includeMenuBar = YES;
                SCStreamConfiguration *config = [SCStreamConfiguration new];
                config.width = monitor.width;
                config.height = monitor.height;
                config.showsCursor = NO;
                config.colorSpaceName = kCGColorSpaceSRGB;
                // SDR output is deliberate: the PNG/webview path is SDR and must not
                // interpret linear HDR values as ordinary 8-bit screenshot colors.
                if (@available(macOS 15.0, *)) config.captureDynamicRange = SCCaptureDynamicRangeSDR;
                [SCScreenshotManager captureImageWithFilter:filter configuration:config
                    completionHandler:^(CGImageRef image, NSError *captureError) {
                    if (image && !captureError) captured = [[NSBitmapImageRep alloc] initWithCGImage:image];
                    else failure = @"macOS could not capture this screen. Check Screen Recording permission and try again.";
                    dispatch_semaphore_signal(ready);
                }];
            }];
            if (dispatch_semaphore_wait(ready, dispatch_time(DISPATCH_TIME_NOW, 10 * NSEC_PER_SEC))) {
                return PulseError(error, @"Screen capture timed out. Try again.");
            }
            if (!captured) return PulseError(error, failure ?: @"Could not capture this screen.");
            CGImageRef image = captured.CGImage;
            if (CGImageGetWidth(image) != monitor.width || CGImageGetHeight(image) != monitor.height) {
                return PulseError(error, @"The screen resolution changed. Start a new selection.");
            }
            size_t length = (size_t)monitor.width * monitor.height * 4;
            uint8_t *buffer = calloc(1, length);
            CGColorSpaceRef space = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
            CGContextRef context = buffer ? CGBitmapContextCreate(buffer, monitor.width, monitor.height,
                8, (size_t)monitor.width * 4, space, kCGBitmapByteOrder32Little | kCGImageAlphaPremultipliedFirst) : NULL;
            CGColorSpaceRelease(space);
            if (!context) { free(buffer); return PulseError(error, @"Could not allocate the screen capture."); }
            CGContextDrawImage(context, CGRectMake(0, 0, monitor.width, monitor.height), image);
            CGContextRelease(context);
            *pixels = buffer;
            return true;
        }
        return PulseError(error, @"Text Extractor requires macOS 14 or later.");
    }
}

char *pulse_recognize_text(const uint8_t *rgba, uint32_t width, uint32_t height, char **error) {
    @autoreleasepool {
        if (@available(macOS 14.0, *)) {
            CGDataProviderRef provider = CGDataProviderCreateWithData(NULL, rgba, (size_t)width * height * 4, NULL);
            CGColorSpaceRef space = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
            CGImageRef image = CGImageCreate(width, height, 8, 32, (size_t)width * 4, space,
                kCGBitmapByteOrder32Big | kCGImageAlphaLast, provider, NULL, false, kCGRenderingIntentDefault);
            CGColorSpaceRelease(space);
            CGDataProviderRelease(provider);
            if (!image) { PulseError(error, @"Could not prepare this selection for text recognition."); return NULL; }
            VNRecognizeTextRequest *request = [VNRecognizeTextRequest new];
            request.revision = VNRecognizeTextRequestRevision3;
            request.recognitionLevel = VNRequestTextRecognitionLevelAccurate;
            request.minimumTextHeight = 0;
            request.automaticallyDetectsLanguage = YES;
            // Copy exactly what was selected, including code and identifiers.
            request.usesLanguageCorrection = NO;
            NSError *languageError = nil;
            NSArray<NSString *> *supported = [request supportedRecognitionLanguagesAndReturnError:&languageError];
            NSMutableOrderedSet<NSString *> *languages = [NSMutableOrderedSet orderedSet];
            for (NSString *preferred in NSLocale.preferredLanguages) {
                for (NSString *candidate in supported) {
                    if ([[candidate componentsSeparatedByString:@"-"].firstObject isEqualToString:
                        [preferred componentsSeparatedByString:@"-"].firstObject]) [languages addObject:candidate];
                }
            }
            for (NSString *candidate in @[@"uk-UA", @"ru-RU", @"en-US"]) {
                if ([supported containsObject:candidate]) [languages addObject:candidate];
            }
            if (languages.count) request.recognitionLanguages = languages.array;
            VNImageRequestHandler *handler = [[VNImageRequestHandler alloc] initWithCGImage:image options:@{}];
            NSError *recognitionError = nil;
            BOOL success = [handler performRequests:@[request] error:&recognitionError];
            CGImageRelease(image);
            if (!success) { PulseError(error, @"Apple Vision could not read this selection. Try again."); return NULL; }
            NSMutableArray *lines = [NSMutableArray array];
            for (VNRecognizedTextObservation *observation in request.results) {
                VNRecognizedText *text = [observation topCandidates:1].firstObject;
                if (!text.string.length) continue;
                CGRect box = observation.boundingBox;
                [lines addObject:@{ @"text":text.string, @"x":@(box.origin.x),
                    @"y":@(1 - CGRectGetMaxY(box)), @"width":@(box.size.width), @"height":@(box.size.height) }];
            }
            NSData *json = [NSJSONSerialization dataWithJSONObject:lines options:0 error:nil];
            NSString *result = [[NSString alloc] initWithData:json encoding:NSUTF8StringEncoding];
            if (!result) { PulseError(error, @"Could not read Apple Vision's result."); return NULL; }
            return strdup(result.UTF8String);
        }
        PulseError(error, @"Text Extractor requires macOS 14 or later.");
        return NULL;
    }
}

bool pulse_prepare_text_recognition(char **error) {
    @autoreleasepool {
        // Synthetic text exercises the same multilingual recognition path as a
        // capture, without screen access. A blank image only warms the detector.
        const uint32_t width = 700, height = 160;
        uint8_t *pixels = calloc((size_t)width * height, 4);
        if (!pixels) return PulseError(error, @"Could not prepare on-device OCR. Try again.");
        CGColorSpaceRef space = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
        CGContextRef context = CGBitmapContextCreate(pixels, width, height, 8, width * 4,
            space, kCGBitmapByteOrder32Big | kCGImageAlphaPremultipliedLast);
        CGColorSpaceRelease(space);
        if (!context) {
            free(pixels);
            return PulseError(error, @"Could not prepare on-device OCR. Try again.");
        }
        CGContextSetRGBFillColor(context, 1, 1, 1, 1);
        CGContextFillRect(context, CGRectMake(0, 0, width, height));
        CTFontRef font = CTFontCreateWithName(CFSTR("Helvetica"), 28, NULL);
        CGFloat y = 100;
        for (NSString *text in @[@"Pulse reads words with spaces.", @"Український текст для перевірки."]) {
            NSAttributedString *string = [[NSAttributedString alloc] initWithString:text
                attributes:@{(__bridge NSString *)kCTFontAttributeName: (__bridge id)font}];
            CTLineRef line = CTLineCreateWithAttributedString((__bridge CFAttributedStringRef)string);
            CGContextSetTextPosition(context, 25, y);
            CTLineDraw(line, context);
            CFRelease(line);
            y -= 50;
        }
        CFRelease(font);
        CGContextRelease(context);
        char *result = pulse_recognize_text(pixels, width, height, error);
        free(pixels);
        if (!result) return false;
        NSData *data = [NSData dataWithBytes:result length:strlen(result)];
        free(result);
        NSArray *lines = [NSJSONSerialization JSONObjectWithData:data options:0 error:nil];
        if (![lines isKindOfClass:NSArray.class] || !lines.count) {
            return PulseError(error, @"Could not prepare on-device OCR. Try again.");
        }
        return true;
    }
}
