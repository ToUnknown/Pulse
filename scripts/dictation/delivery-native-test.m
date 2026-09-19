// Exercise real delivery against a web editor that accepts AX writes but does
// not update its document. No real clipboard writes or keyboard events.
#import <AppKit/AppKit.h>
#import <AVFoundation/AVFoundation.h>
#import <ApplicationServices/ApplicationServices.h>
static AXUIElementRef fakeElement;
static bool focused = true;
static unsigned axWrites, systemEvents, pidEvents;
@interface TestPasteboard : NSObject
+ (instancetype)generalPasteboard;
- (NSInteger)clearContents;
- (BOOL)setString:(NSString *)text forType:(NSString *)type;
@end
@implementation TestPasteboard
+ (instancetype)generalPasteboard { static TestPasteboard *board; if (!board) board = [TestPasteboard new]; return board; }
- (NSInteger)clearContents { return 0; }
- (BOOL)setString:(NSString *)text forType:(NSString *)type { return text.length > 0; }
@end
static bool trusted(void) { return true; }
static AXError copyAttribute(AXUIElementRef element, CFStringRef name, CFTypeRef *out) {
    (void)element; *out = NULL;
    if (CFEqual(name,kAXFocusedUIElementAttribute) && focused) *out = CFRetain(fakeElement);
    else if (CFEqual(name,kAXRoleAttribute)) *out = CFRetain(kAXTextAreaRole);
    else if (CFEqual(name,CFSTR("AXEditable"))) *out = CFRetain(kCFBooleanTrue);
    return *out ? kAXErrorSuccess : kAXErrorNoValue;
}
static AXError settable(AXUIElementRef e, CFStringRef n, Boolean *out) { *out = true; return kAXErrorSuccess; }
static AXError noOpWrite(AXUIElementRef e, CFStringRef n, CFTypeRef v) { axWrites++; return kAXErrorSuccess; }
static void systemPost(CGEventTapLocation tap, CGEventRef event) {
    if (tap != kCGHIDEventTap || CGEventGetIntegerValueField(event,kCGKeyboardEventKeycode) != 9 || CGEventGetFlags(event) != kCGEventFlagMaskCommand) abort();
    systemEvents++;
}
static void pidPost(pid_t pid, CGEventRef event) { pidEvents++; }
#define AXIsProcessTrusted trusted
#define AXUIElementCopyAttributeValue copyAttribute
#define AXUIElementIsAttributeSettable settable
#define AXUIElementSetAttributeValue noOpWrite
#define CGEventPost systemPost
#define CGEventPostToPid pidPost
#define NSPasteboard TestPasteboard
#include "../../src-tauri/src/dictation/native.m"
#include <stdio.h>
int main(void) {
    @autoreleasepool {
        fakeElement = AXUIElementCreateApplication(getpid());
        deliveryElement = (AXUIElementRef)CFRetain(fakeElement);
        deliveryPID = NSWorkspace.sharedWorkspace.frontmostApplication.processIdentifier;
        int result = pulse_dictation_deliver("Regression transcript");
        if (result != 3 || systemEvents != 2 || axWrites || pidEvents) {
            fprintf(stderr,"FAIL: editor received no standard paste (result=%d, AX writes=%u, system events=%u, PID events=%u)\n",result,axWrites,systemEvents,pidEvents); return 1;
        }
        focused = false;
        deliveryElement = (AXUIElementRef)CFRetain(fakeElement);
        result = pulse_dictation_deliver("Clipboard fallback");
        if (result != 2 || systemEvents != 2) { fprintf(stderr,"FAIL: changed focus must only copy\n"); return 1; }
        CFRelease(fakeElement);
        puts("PASS: web editor uses standard paste; changed focus only copies");
    }
}
