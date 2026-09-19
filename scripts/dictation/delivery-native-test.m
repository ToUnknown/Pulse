// Actual native live writer, with AX, keyboard posting and pasteboard mocked.
// No real input or clipboard is changed. Run on macOS.
#import <AppKit/AppKit.h>
#import <AVFoundation/AVFoundation.h>
#import <ApplicationServices/ApplicationServices.h>
static AXUIElementRef fakeElement, selectedElement, fakeSystem, fakeWindow, focusedWindow, fakeParent;
static bool stableIdentity, wrongIdentity, wrongParent, caretBounds;
static bool acceptFocus=true, targetGone=false;
static pid_t foregroundPID=1000, secondPID=1000;
static unsigned focusRequests, raiseRequests;
static bool missingSystemFocus, missingText, missingSelection, missingEditability;
static NSUInteger emptyCaretOffset;
static long characterCountOverride=-1;
static void (*observePosted)(CGEventRef);
static unsigned shortcutActions;
static NSString *editor, *secondEditor;
static AXUIElementRef secondElement;
static NSRange secondCaret;
static NSMutableArray<NSString *> *editFrames;
static bool exposesPlaceholder, phantomPlaceholderCaret, emptyParagraph;
static NSRange caret;
static bool focused=true, supportsRange=true, acceptTyping=true, acceptSelection=true;
static unsigned keyboardEvents, clipboardWrites, withdrawalWrites;
static bool acceptWithdrawal=true;
@interface TestPasteboard : NSObject
+ (instancetype)generalPasteboard;
- (NSInteger)clearContents;
- (BOOL)setString:(NSString *)text forType:(NSString *)type;
@end
@implementation TestPasteboard
+ (instancetype)generalPasteboard { static TestPasteboard *board; if (!board) board=[TestPasteboard new]; return board; }
- (NSInteger)clearContents { clipboardWrites++; return 0; }
- (BOOL)setString:(NSString *)text forType:(NSString *)type { clipboardWrites++; return text.length>0; }
@end
// Focus recovery is mocked too: the harness never activates a real app.
@interface TestRunningApplication : NSObject
@property pid_t processIdentifier;
@property BOOL terminated;
+ (instancetype)runningApplicationWithProcessIdentifier:(pid_t)pid;
- (BOOL)activateWithOptions:(NSApplicationActivationOptions)options;
@end
@implementation TestRunningApplication
+ (instancetype)runningApplicationWithProcessIdentifier:(pid_t)pid { TestRunningApplication *app=[self new]; app.processIdentifier=pid; app.terminated=targetGone; return app; }
- (BOOL)activateWithOptions:(NSApplicationActivationOptions)options { if(acceptFocus)foregroundPID=self.processIdentifier; return acceptFocus; }
@end
@interface TestWorkspace : NSObject
@property(readonly) TestRunningApplication *frontmostApplication;
@property(readonly) NSNotificationCenter *notificationCenter;
+ (instancetype)sharedWorkspace;
- (BOOL)openURL:(NSURL *)url;
@end
@implementation TestWorkspace
+ (instancetype)sharedWorkspace { static TestWorkspace *w; if(!w)w=[self new]; return w; }
- (TestRunningApplication *)frontmostApplication { return [TestRunningApplication runningApplicationWithProcessIdentifier:foregroundPID]; }
- (BOOL)openURL:(NSURL *)url { abort(); }
- (NSNotificationCenter *)notificationCenter { return NSNotificationCenter.defaultCenter; }
@end
static AXUIElementRef createSystem(void) { return (AXUIElementRef)CFRetain(fakeSystem); }
static bool trusted(void) { return true; }
static AXError getPid(AXUIElementRef element,pid_t *pid) { *pid=element==secondElement ? secondPID : 1000; return kAXErrorSuccess; }
static AXError copyAttribute(AXUIElementRef element,CFStringRef name,CFTypeRef *out) {
    *out=NULL;
    NSString *fieldText=element==secondElement ? secondEditor : editor;
    NSRange fieldCaret=element==secondElement ? secondCaret : caret;
    if (targetGone && CFEqual(element,fakeElement)) return kAXErrorInvalidUIElement;
    if (CFEqual(name,kAXIdentifierAttribute) && stableIdentity) { *out=CFRetain(wrongIdentity && !CFEqual(element,fakeElement) ? CFSTR("other") : CFSTR("original-editor")); return kAXErrorSuccess; }
    if (CFEqual(name,kAXParentAttribute)) { *out=CFRetain(wrongParent && !CFEqual(element,fakeElement) ? fakeWindow : fakeParent); return kAXErrorSuccess; }
    if (CFEqual(name,kAXFocusedWindowAttribute)) { *out=CFRetain(focusedWindow); return kAXErrorSuccess; }
    if (CFEqual(name,kAXWindowAttribute)) { *out=CFRetain(fakeWindow); return kAXErrorSuccess; }
    if ((missingText && CFEqual(name,kAXValueAttribute)) || (missingSelection && CFEqual(name,kAXSelectedTextRangeAttribute))) return kAXErrorCannotComplete;
    if (missingEditability && (CFEqual(name,kAXRoleAttribute) || CFEqual(name,CFSTR("AXEditable")))) return kAXErrorCannotComplete;
    if (CFEqual(name,kAXFocusedUIElementAttribute) && focused && !(missingSystemFocus && CFEqual(element,fakeSystem))) *out=CFRetain(selectedElement);
    else if (CFEqual(name,kAXRoleAttribute)) *out=CFRetain(kAXTextAreaRole);
    else if (CFEqual(name,CFSTR("AXEditable"))) *out=CFRetain(kCFBooleanTrue);
    else if (CFEqual(name,kAXValueAttribute)) *out=CFBridgingRetain(!fieldText.length && emptyParagraph ? @"\n" : exposesPlaceholder && !fieldText.length ? @"\nType here" : fieldText);
    else if (CFEqual(name,kAXNumberOfCharactersAttribute)) { long count=characterCountOverride>=0 ? characterCountOverride : fieldText.length; *out=CFNumberCreate(NULL,kCFNumberLongType,&count); }
    else if (CFEqual(name,kAXDescriptionAttribute) && exposesPlaceholder) *out=CFRetain(CFSTR("Type here"));
    else if (CFEqual(name,kAXSelectedTextRangeAttribute)) { CFRange r=CFRangeMake(!fieldText.length && emptyCaretOffset ? emptyCaretOffset : phantomPlaceholderCaret && !fieldText.length ? 10 : fieldCaret.location,fieldCaret.length); *out=AXValueCreate(kAXValueCFRangeType,&r); }
    return *out?kAXErrorSuccess:kAXErrorNoValue;
}
static AXError settable(AXUIElementRef e,CFStringRef name,Boolean *out) { *out=!missingEditability && (CFEqual(name,kAXSelectedTextRangeAttribute)?supportsRange:true); return kAXErrorSuccess; }
static AXError writeAttribute(AXUIElementRef e,CFStringRef name,CFTypeRef value) {
    // Final delivery never changes focus, selection, or a whole field through AX.
    abort();
}
static void post(pid_t pid,CGEventRef event) {
    if (pid!=(selectedElement==secondElement ? secondPID : 1000) || CGEventGetFlags(event)!=0 || !CGEventGetIntegerValueField(event,kCGEventSourceUserData)) abort();
    keyboardEvents++;
    if (observePosted) observePosted(event);
    if (!acceptTyping || CGEventGetType(event)!=kCGEventKeyDown) return;
    UniChar buffer[128]; UniCharCount length=0;
    CGEventKeyboardGetUnicodeString(event,128,&length,buffer);
    if (CGEventGetIntegerValueField(event,kCGKeyboardEventKeycode)==0x33) length=0;
    NSString *insert=length?[[NSString alloc] initWithCharacters:buffer length:length]:@"";
    if(selectedElement==secondElement){secondEditor=[secondEditor stringByReplacingCharactersInRange:secondCaret withString:insert];secondCaret=NSMakeRange(secondCaret.location+insert.length,0);return;}
    editor=[editor stringByReplacingCharactersInRange:caret withString:insert];
    caret=NSMakeRange(caret.location+insert.length,0);
    [editFrames addObject:editor];
}
static AXError parameterized(AXUIElementRef e,CFStringRef name,CFTypeRef value,CFTypeRef *out) {
    *out=NULL;
    if (!caretBounds || !CFEqual(name,kAXBoundsForRangeParameterizedAttribute))return kAXErrorNoValue;
    CFRange r; if(!AXValueGetValue(value,kAXValueCFRangeType,&r))return kAXErrorIllegalArgument;
    CGRect rect=CGRectMake(100+(r.location%20)*7,200+(r.location/20)*18,r.length*7,18);
    *out=AXValueCreate(kAXValueCGRectType,&rect);return kAXErrorSuccess;
}
#define AXUIElementCopyParameterizedAttributeValue parameterized
static AXError performAction(AXUIElementRef e,CFStringRef action) { if(!CFEqual(e,fakeWindow) || !CFEqual(action,kAXRaiseAction))abort(); raiseRequests++; if(acceptFocus)focusedWindow=fakeWindow; return kAXErrorSuccess; }
static CFAbsoluteTime testTime=1000;
static CFAbsoluteTime currentTime(void) { testTime+=0.02; return testTime; }
#define CFAbsoluteTimeGetCurrent currentTime
#define AXUIElementPerformAction performAction
#define NSRunningApplication TestRunningApplication
#define NSWorkspace TestWorkspace
#define AXUIElementCreateSystemWide createSystem
#define AXIsProcessTrusted trusted
#define AXUIElementGetPid getPid
#define AXUIElementCopyAttributeValue copyAttribute
#define AXUIElementIsAttributeSettable settable
#define AXUIElementSetAttributeValue writeAttribute
#define CGEventPostToPid post
#define NSPasteboard TestPasteboard
#include "../../src-tauri/src/dictation/native.m"
#include <stdio.h>
static void expect(bool ok,const char *message) { if (!ok) { fprintf(stderr,"FAIL: %s\n",message); exit(1); } }
static void observeOwnEvent(CGEventRef event) { PulseDictationShortcutEvent([NSEvent eventWithCGEvent:event]); }
static void shortcutAction(bool down,bool chord,bool interrupted,double time) { shortcutActions++; }
static void begin(void) {
    stableIdentity=wrongIdentity=wrongParent=caretBounds=false;
    acceptFocus=true; targetGone=false; foregroundPID=1000; secondPID=1000; focusedWindow=fakeWindow;
    missingSystemFocus=missingText=missingSelection=missingEditability=false; emptyCaretOffset=0; characterCountOverride=-1;
    selectedElement=fakeElement; exposesPlaceholder=false; phantomPlaceholderCaret=false; emptyParagraph=false;
    focused=supportsRange=acceptTyping=acceptSelection=true;
    editor=@"Before SELECT after"; caret=NSMakeRange(7,6);
    expect(pulse_dictation_delivery_begin()==1,"capture selected input only at final delivery");
}
static void insert(const char *text) {
    int result=0;
    for(int i=0;i<100 && result==0;i++) result=pulse_dictation_final_step(text);
    if(result!=1)fprintf(stderr,"insert %s result %d value %s expected %s range %lu,%lu expected %lu,%lu\n",text,result,editor.UTF8String,expectedValue.UTF8String,caret.location,caret.length,expectedSelection.location,expectedSelection.length);
    expect(result==1,"final insertion is verified by read-back");
}
int main(void) {
    @autoreleasepool {
        fakeElement=AXUIElementCreateApplication(getpid());
        fakeSystem=AXUIElementCreateApplication(getpid()+2);
        fakeWindow=AXUIElementCreateApplication(getpid()+9);
        fakeParent=AXUIElementCreateApplication(getpid()+11);
        secondElement=AXUIElementCreateApplication(getpid()+12);
        begin(); shortcutCallback=shortcutAction; observePosted=observeOwnEvent;
        unsigned before=keyboardEvents;
        expect([editor isEqualToString:@"Before SELECT after"] && !before,"capturing input never types");
        insert("Final words 🌍 café 👨‍👩‍👧‍👦 中文.");
        expect([editor isEqualToString:@"Before Final words 🌍 café 👨‍👩‍👧‍👦 中文. after"],"replace only current selection and preserve Unicode and surroundings");
        expect(!shortcutActions && !deliveryInterrupted,"own insertion events never activate shortcuts");
        before=keyboardEvents; insert("Final words 🌍 café 👨‍👩‍👧‍👦 中文.");
        expect(keyboardEvents==before,"verified delivery never inserts twice");
        // A field selected at completion receives the result, regardless of
        // which field was selected while the microphone was recording.
        begin(); pulse_dictation_clear_target();
        selectedElement=secondElement; secondPID=foregroundPID=2000;
        secondEditor=@"Other REPLACE end"; secondCaret=NSMakeRange(6,7);
        expect(pulse_dictation_delivery_begin()==1,"capture the field selected at completion");
        insert("finished transcript");
        expect([secondEditor isEqualToString:@"Other finished transcript end"] && [editor isEqualToString:@"Before SELECT after"],"only final selected field changes");
        begin(); focused=false;
        before=keyboardEvents;
        expect(pulse_dictation_final_step("words")==-1 && keyboardEvents==before,"focus loss during delivery falls back without typing");
        begin(); selectedElement=secondElement;
        expect(pulse_dictation_final_step("words")==-1,"never retarget during an insertion");
        begin(); focusedWindow=secondElement;
        expect(pulse_dictation_final_step("words")==-1,"stale focus in another window is rejected");
        begin(); caret=NSMakeRange(0,0);
        expect(pulse_dictation_final_step("words")==-1,"caret changes never overwrite an unintended range");
        begin(); editor=@"User edited";
        expect(pulse_dictation_final_step("words")==-1,"user edits are preserved");
        begin(); acceptTyping=false;
        expect(pulse_dictation_final_step("hello")==0,"posting input is not success");
        before=keyboardEvents; editor=@"Before he after"; caret=NSMakeRange(9,0);
        expect(pulse_dictation_final_step("hello")==0 && keyboardEvents==before,"partial readback waits without replay");
        editor=pendingValue; caret=pendingSelection; acceptTyping=true;
        insert("hello");
        expect(keyboardEvents==before,"acknowledged input was posted only once");
        begin(); acceptTyping=false;
        expect(pulse_dictation_final_step("ignored")==0,"start ignored insertion"); pendingSince-=1;
        expect(pulse_dictation_final_step("ignored")==-1,"ignored insertion falls back after bounded wait");
        begin(); missingText=true;
        expect(pulse_dictation_final_step("words")==0,"temporarily missing text waits");
        missingText=false; insert("words");
        begin(); exposesPlaceholder=phantomPlaceholderCaret=true; editor=@""; caret=NSMakeRange(0,0);
        expect(pulse_dictation_delivery_begin()==1,"web placeholder is readable");
        insert("A complete sentence with a placeholder.");
        expect([editor isEqualToString:@"A complete sentence with a placeholder."],"placeholder disappears without losing delivery");
        begin(); editor=@""; caret=NSMakeRange(0,0); emptyCaretOffset=1;
        expect(pulse_dictation_delivery_begin()==1,"synthetic empty caret is normalized");
        insert("Empty editor."); expect([editor isEqualToString:@"Empty editor."],"empty contenteditable receives final text");
        begin(); missingSystemFocus=true;
        expect(pulse_dictation_delivery_begin()==1,"app focus fallback supports embedded editors"); insert("embedded editor");
        begin(); supportsRange=false;
        expect(pulse_dictation_delivery_begin()==1,"insertion needs no AX range setter"); insert("native input");
        expect(clipboardWrites==0 && !focusRequests && !raiseRequests,"input delivery preserves clipboard and never steals focus");
        focused=false;
        expect(pulse_dictation_delivery_begin()==0,"no selected input uses clipboard");
        expect(pulse_dictation_copy("clipboard result") && clipboardWrites==2,"explicit fallback copies the full transcript once");
        pulse_dictation_clear_target();
        CFRelease(fakeElement); CFRelease(fakeSystem); CFRelease(fakeWindow); CFRelease(fakeParent); CFRelease(secondElement);
        puts("PASS: final-only insertion, final selected field, Unicode, placeholders, delayed readbacks, focus and edit guards, clipboard preservation");
    }
}
