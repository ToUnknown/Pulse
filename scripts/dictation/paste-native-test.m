// Verify one copy-and-paste attempt without changing the user's clipboard or typing.
#import <AppKit/AppKit.h>
#import <AVFoundation/AVFoundation.h>
#import <ApplicationServices/ApplicationServices.h>
#import <objc/runtime.h>
static CGEventFlags flags;
static pid_t activePID=1;
static bool changeFlagsOnCopy, changeAppOnCopy;
@interface TestFrontmostApp : NSObject
- (pid_t)processIdentifier;
@end
@implementation TestFrontmostApp
- (pid_t)processIdentifier { return activePID; }
@end
@interface TestPasteboard : NSObject
@property (nonatomic, copy) NSString *text;
@end
@implementation TestPasteboard
- (NSInteger)clearContents { self.text=nil; return 1; }
- (BOOL)setString:(NSString *)text forType:(NSPasteboardType)type {
    (void)type; self.text=text;
    if (changeFlagsOnCopy) flags=kCGEventFlagMaskShift;
    if (changeAppOnCopy) activePID=2;
    return YES;
}
- (NSString *)stringForType:(NSPasteboardType)type { (void)type; return self.text; }
@end
static TestPasteboard *testBoard;
static int posted;
static CGKeyCode pasteKey;
static CGEventFlags TestFlags(CGEventSourceStateID source) { (void)source; return flags; }
static void TestPost(CGEventTapLocation location, CGEventRef event) {
    (void)location;
    if (posted==1) pasteKey=CGEventGetIntegerValueField(event,kCGKeyboardEventKeycode);
    posted++;
}
#define CGEventSourceFlagsState TestFlags
#define CGEventPost TestPost
#include "../../src-tauri/src/dictation/native.m"
#undef CGEventSourceFlagsState
#undef CGEventPost
int main(void) { @autoreleasepool {
    testBoard=[TestPasteboard new];
    TestFrontmostApp *frontmostApp=[TestFrontmostApp new];
    Method method=class_getClassMethod([NSPasteboard class],@selector(generalPasteboard));
    IMP original=method_getImplementation(method);
    method_setImplementation(method,imp_implementationWithBlock(^id(id self) { (void)self; return testBoard; }));
    Method frontMethod=class_getInstanceMethod([NSWorkspace class],@selector(frontmostApplication));
    IMP originalFront=method_getImplementation(frontMethod);
    method_setImplementation(frontMethod,imp_implementationWithBlock(^id(id self) { (void)self; return frontmostApp; }));
    flags=kCGEventFlagMaskAlternate;
    int waiting=pulse_dictation_final_step("hello from Pulse");
    bool untouched=posted==0 && testBoard.text==nil;
    flags=0;
    int sent=pulse_dictation_final_step("hello from Pulse");
    bool copied=[testBoard.text isEqualToString:@"hello from Pulse"];
    bool onePaste=posted==4 && pasteKey==PulseDictationPasteKey();
    posted=0; changeFlagsOnCopy=true;
    int changedModifier=pulse_dictation_final_step("modifiers changed");
    bool modifierSkipped=changedModifier==1 && posted==0 && [testBoard.text isEqualToString:@"modifiers changed"];
    changeFlagsOnCopy=false; flags=0;
    posted=0; activePID=1; changeAppOnCopy=true;
    int changedApp=pulse_dictation_final_step("window changed");
    bool appSkipped=changedApp==1 && posted==0 && [testBoard.text isEqualToString:@"window changed"];
    method_setImplementation(frontMethod,originalFront);
    method_setImplementation(method,original);
    printf("waited=%d copied=%d one_paste=%d modifier_skipped=%d app_skipped=%d\n",waiting==0 && untouched,copied,onePaste && sent==1,modifierSkipped,appSkipped);
    return waiting==0 && untouched && copied && onePaste && sent==1 && modifierSkipped && appSkipped ? 0 : 1;
} }
