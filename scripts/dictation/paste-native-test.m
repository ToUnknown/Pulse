// Verify one copy-and-paste attempt without changing the user's clipboard or typing.
#import <AppKit/AppKit.h>
#import <AVFoundation/AVFoundation.h>
#import <ApplicationServices/ApplicationServices.h>
#import <objc/runtime.h>
@interface TestPasteboard : NSObject
@property (nonatomic, copy) NSString *text;
@end
@implementation TestPasteboard
- (NSInteger)clearContents { self.text=nil; return 1; }
- (BOOL)setString:(NSString *)text forType:(NSPasteboardType)type { (void)type; self.text=text; return YES; }
- (NSString *)stringForType:(NSPasteboardType)type { (void)type; return self.text; }
@end
static TestPasteboard *testBoard;
static int posted;
static CGKeyCode pasteKey;
static CGEventFlags flags;
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
    Method method=class_getClassMethod([NSPasteboard class],@selector(generalPasteboard));
    IMP original=method_getImplementation(method);
    method_setImplementation(method,imp_implementationWithBlock(^id(id self) { (void)self; return testBoard; }));
    flags=kCGEventFlagMaskAlternate;
    int waiting=pulse_dictation_final_step("hello from Pulse");
    bool untouched=posted==0 && testBoard.text==nil;
    flags=0;
    int sent=pulse_dictation_final_step("hello from Pulse");
    bool copied=[testBoard.text isEqualToString:@"hello from Pulse"];
    bool onePaste=posted==4 && pasteKey==PulseDictationPasteKey();
    method_setImplementation(method,original);
    printf("waited=%d copied=%d one_paste=%d\n",waiting==0 && untouched,copied,onePaste && sent==1);
    return waiting==0 && untouched && copied && onePaste && sent==1 ? 0 : 1;
} }
