// Focused-field delivery with an unrenderable old clipboard format.
// Private pasteboard and suppressed keyboard events keep user state untouched.
#import <AppKit/AppKit.h>
#import <AVFoundation/AVFoundation.h>
#import <ApplicationServices/ApplicationServices.h>
#import <objc/runtime.h>
static AXUIElementRef fakeRoot, fakeField;
static NSPasteboard *testBoard;
static int posted;
static bool fieldFocused=true;
static CFStringRef fieldRole;
static bool TestTrusted(void) { return true; }
static AXUIElementRef TestRoot(void) { return (AXUIElementRef)CFRetain(fakeRoot); }
static AXError TestTimeout(AXUIElementRef e, float t) { (void)e; (void)t; return kAXErrorSuccess; }
static CFTypeID TestAXType(void) { return CFDictionaryGetTypeID(); }
static AXError TestAttribute(AXUIElementRef e, CFStringRef name, CFTypeRef *out) {
  if (e==fakeRoot && fieldFocused && CFEqual(name,kAXFocusedUIElementAttribute)) { *out=CFRetain(fakeField); return kAXErrorSuccess; }
  if (e==fakeField && CFEqual(name,kAXRoleAttribute)) { *out=CFRetain(fieldRole); return kAXErrorSuccess; }
  if (e==fakeField && CFEqual(name,kAXEnabledAttribute)) { *out=CFRetain(kCFBooleanTrue); return kAXErrorSuccess; }
  if (e==fakeField && CFEqual(name,CFSTR("AXEditable"))) { *out=CFRetain(kCFBooleanTrue); return kAXErrorSuccess; }
  *out=NULL; return kAXErrorNoValue;
}
static AXError TestSettable(AXUIElementRef e, CFStringRef name, Boolean *result) {
  *result=e==fakeField && CFEqual(name,kAXSelectedTextAttribute); return kAXErrorSuccess;
}
static CGEventFlags TestFlags(CGEventSourceStateID source) { (void)source; return 0; }
static void TestPost(CGEventTapLocation location, CGEventRef event) { (void)location; (void)event; posted++; }
#define AXIsProcessTrusted TestTrusted
#define AXUIElementCreateSystemWide TestRoot
#define AXUIElementSetMessagingTimeout TestTimeout
#define AXUIElementGetTypeID TestAXType
#define AXUIElementCopyAttributeValue TestAttribute
#define AXUIElementIsAttributeSettable TestSettable
#define CGEventSourceFlagsState TestFlags
#define CGEventPost TestPost
#include "../../src-tauri/src/dictation/native.m"
#undef AXIsProcessTrusted
#undef AXUIElementCreateSystemWide
#undef AXUIElementSetMessagingTimeout
#undef AXUIElementGetTypeID
#undef AXUIElementCopyAttributeValue
#undef AXUIElementIsAttributeSettable
#undef CGEventSourceFlagsState
#undef CGEventPost
int main(void) { @autoreleasepool {
  fakeRoot=(AXUIElementRef)CFBridgingRetain([NSMutableDictionary dictionary]);
  fakeField=(AXUIElementRef)CFBridgingRetain([NSMutableDictionary dictionary]);
  testBoard=[NSPasteboard pasteboardWithUniqueName];
  Method method=class_getClassMethod([NSPasteboard class],@selector(generalPasteboard));
  IMP original=method_getImplementation(method);
  method_setImplementation(method,imp_implementationWithBlock(^id(id self) { (void)self; return testBoard; }));
  CFStringRef roles[]={kAXTextFieldRole,kAXTextAreaRole,kAXComboBoxRole};
  bool pasted=true;
  for (int i=0;i<3;i++) {
    fieldRole=roles[i]; posted=0;
    [testBoard declareTypes:@[@"app.pulse.test.unavailable"] owner:nil];
    int result=pulse_dictation_final_step("hello from Pulse");
    pasted &= result==1 && posted==4 && [[testBoard stringForType:NSPasteboardTypeString] isEqualToString:@"hello from Pulse"];
  }
  fieldFocused=false; posted=0;
  int noField=pulse_dictation_final_step("clipboard fallback");
  bool copied=pulse_dictation_copy("clipboard fallback") &&
    [[testBoard stringForType:NSPasteboardTypeString] isEqualToString:@"clipboard fallback"];
  method_setImplementation(method,original);
  printf("focused_roles_pasted=%d no_field_step=%d copied=%d\n",pasted,noField,copied);
  [testBoard releaseGlobally]; CFRelease(fakeRoot); CFRelease(fakeField);
  return pasted && noField==-1 && posted==0 && copied ? 0 : 1;
} }
