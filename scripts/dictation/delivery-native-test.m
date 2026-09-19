// Actual native live writer, with AX, keyboard posting and pasteboard mocked.
// No real input or clipboard is changed. Run on macOS.
#import <AppKit/AppKit.h>
#import <AVFoundation/AVFoundation.h>
#import <ApplicationServices/ApplicationServices.h>
static AXUIElementRef fakeElement, selectedElement, fakeSystem;
static bool missingSystemFocus;
static NSUInteger emptyCaretOffset;
static long characterCountOverride=-1;
static void (*observePosted)(CGEventRef);
static unsigned shortcutActions;
static NSString *editor;
static NSMutableArray<NSString *> *editFrames;
static bool exposesPlaceholder, phantomPlaceholderCaret, emptyParagraph;
static NSRange caret;
static bool focused=true, supportsRange=true, acceptTyping=true, acceptSelection=true;
static unsigned keyboardEvents, clipboardWrites;
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
static AXUIElementRef createSystem(void) { return (AXUIElementRef)CFRetain(fakeSystem); }
static bool trusted(void) { return true; }
static AXError getPid(AXUIElementRef element,pid_t *pid) { *pid=NSWorkspace.sharedWorkspace.frontmostApplication.processIdentifier; return kAXErrorSuccess; }
static AXError copyAttribute(AXUIElementRef element,CFStringRef name,CFTypeRef *out) {
    *out=NULL;
    if (CFEqual(name,kAXFocusedUIElementAttribute) && focused && !(missingSystemFocus && CFEqual(element,fakeSystem))) *out=CFRetain(selectedElement);
    else if (CFEqual(name,kAXRoleAttribute)) *out=CFRetain(kAXTextAreaRole);
    else if (CFEqual(name,CFSTR("AXEditable"))) *out=CFRetain(kCFBooleanTrue);
    else if (CFEqual(name,kAXValueAttribute)) *out=CFBridgingRetain(!editor.length && emptyParagraph ? @"\n" : exposesPlaceholder && !editor.length ? @"\nType here" : editor);
    else if (CFEqual(name,kAXNumberOfCharactersAttribute)) { long count=characterCountOverride>=0 ? characterCountOverride : editor.length; *out=CFNumberCreate(NULL,kCFNumberLongType,&count); }
    else if (CFEqual(name,kAXDescriptionAttribute) && exposesPlaceholder) *out=CFRetain(CFSTR("Type here"));
    else if (CFEqual(name,kAXSelectedTextRangeAttribute)) { CFRange r=CFRangeMake(!editor.length && emptyCaretOffset ? emptyCaretOffset : phantomPlaceholderCaret && !editor.length ? 10 : caret.location,caret.length); *out=AXValueCreate(kAXValueCFRangeType,&r); }
    return *out?kAXErrorSuccess:kAXErrorNoValue;
}
static AXError settable(AXUIElementRef e,CFStringRef name,Boolean *out) { *out=CFEqual(name,kAXSelectedTextRangeAttribute)?supportsRange:true; return kAXErrorSuccess; }
static AXError writeAttribute(AXUIElementRef e,CFStringRef name,CFTypeRef value) {
    // A direct selected-text/value write must never be used for insertion.
    if (!CFEqual(name,kAXSelectedTextRangeAttribute)) abort();
    CFRange r;
    if (acceptSelection && AXValueGetValue(value,kAXValueCFRangeType,&r)) caret=NSMakeRange(r.location,r.length);
    return kAXErrorSuccess;
}
static void post(CGEventTapLocation tap,CGEventRef event) {
    if (tap!=kCGHIDEventTap || CGEventGetFlags(event)!=0 || !CGEventGetIntegerValueField(event,kCGEventSourceUserData)) abort();
    keyboardEvents++;
    if (observePosted) observePosted(event);
    if (!acceptTyping || CGEventGetType(event)!=kCGEventKeyDown) return;
    UniChar buffer[128]; UniCharCount length=0;
    CGEventKeyboardGetUnicodeString(event,128,&length,buffer);
    if (CGEventGetIntegerValueField(event,kCGKeyboardEventKeycode)==0x33) length=0;
    NSString *insert=length?[[NSString alloc] initWithCharacters:buffer length:length]:@"";
    editor=[editor stringByReplacingCharactersInRange:caret withString:insert];
    caret=NSMakeRange(caret.location+insert.length,0);
    [editFrames addObject:editor];
}
#define AXUIElementCreateSystemWide createSystem
#define AXIsProcessTrusted trusted
#define AXUIElementGetPid getPid
#define AXUIElementCopyAttributeValue copyAttribute
#define AXUIElementIsAttributeSettable settable
#define AXUIElementSetAttributeValue writeAttribute
#define CGEventPost post
#define NSPasteboard TestPasteboard
#include "../../src-tauri/src/dictation/native.m"
#include <stdio.h>
static void expect(bool ok,const char *message) { if (!ok) { fprintf(stderr,"FAIL: %s\n",message); exit(1); } }
static void observeOwnEvent(CGEventRef event) { PulseDictationShortcutEvent([NSEvent eventWithCGEvent:event]); }
static void shortcutAction(bool down,bool chord,bool interrupted,double time) { shortcutActions++; }
static void begin(void) {
    missingSystemFocus=false; emptyCaretOffset=0; characterCountOverride=-1;
    selectedElement=fakeElement; exposesPlaceholder=false; phantomPlaceholderCaret=false; emptyParagraph=false;
    focused=supportsRange=acceptTyping=acceptSelection=true;
    editor=@"Before SELECT after"; caret=NSMakeRange(7,6);
    expect(pulse_dictation_live_begin()==1,"readable input supports live insertion");
}
static void syncText(const char *text) {
    int result=0;
    for(int i=0;i<100 && result==0;i++) result=pulse_dictation_live_step(text);
    expect(result==1,"writer must read back every completed update");
}
int main(void) {
    @autoreleasepool {
        fakeElement=AXUIElementCreateApplication(getpid());
        fakeSystem=AXUIElementCreateApplication(getpid()+2);
        begin();
        shortcutCallback=shortcutAction; observePosted=observeOwnEvent;
        syncText("hello wonderful world");
        expect(shortcutActions==0 && !liveInterrupted,"own Unicode events must not cancel held dictation");
        expect([editor isEqualToString:@"Before hello wonderful world after"],"replace selection, preserve surrounding text");
        syncText("hello corrected 🌍 café 👨‍👩‍👧‍👦 中文");
        expect([editor isEqualToString:@"Before hello corrected 🌍 café 👨‍👩‍👧‍👦 中文 after"],"correct only owned text and preserve Unicode");
        syncText("this sentence is already written in the field");
        editFrames=[NSMutableArray new];
        syncText("This sentence is already written in the field.");
        for (NSString *frame in editFrames) expect([frame containsString:@" sentence is already written in the field"],"final capitalization and punctuation must never erase and replay unchanged text");
        expect([editor isEqualToString:@"Before This sentence is already written in the field. after"],"final corrections preserve surrounding content");
        expect(NSEqualRanges(caret,NSMakeRange(7+[@"This sentence is already written in the field." length],0)),"caret returns to the end of dictation after corrections");
        unsigned unchangedEvents=keyboardEvents;
        syncText("This sentence is already written in the field.");
        expect(keyboardEvents==unchangedEvents,"identical final transcript never retypes text");
        editFrames=nil;
        syncText("the cat is resting beside the window");
        editFrames=[NSMutableArray new];
        syncText("The dog is resting beside the window.");
        for (NSString *frame in editFrames) expect([frame containsString:@" is resting beside the window"],"separate corrections preserve unchanged trailing words");
        editFrames=nil;
        syncText("🌍 é 👨‍👩‍👧‍👦 suffix");
        syncText("🌎 é 👨‍👩‍👦 suffix!");
        expect([editor isEqualToString:@"Before 🌎 é 👨‍👩‍👦 suffix! after"],"corrections never split composed Unicode characters");
        syncText(""); syncText("new");
        expect([editor isEqualToString:@"Before new after"],"empty correction must not restore original replacement range");
        unsigned posted=keyboardEvents;
        caret=NSMakeRange(0,0);
        expect(pulse_dictation_live_step("new words")==-1 && keyboardEvents==posted,"cursor movement stops insertion");
        begin(); syncText("dictated"); editor=[editor stringByAppendingString:@" user edit"];
        expect(pulse_dictation_live_step("correction")==-1,"user edits stop replacement");
        begin();
        AXUIElementRef otherField=AXUIElementCreateApplication(getpid()+1);
        selectedElement=otherField;
        unsigned beforeFocus=keyboardEvents;
        expect(pulse_dictation_live_step("hello")==2 && keyboardEvents==beforeFocus,"focus changes pause the pinned input");
        selectedElement=fakeElement; CFRelease(otherField); syncText("hello");
        expect([editor isEqualToString:@"Before hello after"],"returning to original input resumes without retargeting");
        begin(); acceptTyping=false;
        expect(pulse_dictation_live_step("ignored")==0,"posting is not success");
        pendingSince-=1;
        expect(pulse_dictation_live_step("ignored")==-1,"unacknowledged typing stops without duplicate retries");
        begin(); syncText("old"); acceptSelection=false;
        expect(pulse_dictation_live_step("new")==0,"wait for asynchronous selection acknowledgement");
        selectionSince-=1;
        expect(pulse_dictation_live_step("new")==-1,"ignored range selection never overwrites unrelated text");
        begin(); syncText("old"); acceptSelection=false;
        unsigned beforeSelection=keyboardEvents;
        expect(pulse_dictation_live_step("new")==0 && keyboardEvents==beforeSelection,"no text posted before selection acknowledgement");
        caret=requestedSelection; acceptSelection=true;
        syncText("new");
        expect([editor isEqualToString:@"Before new after"],"deferred selection completes safely");
        begin(); supportsRange=false;
        expect(pulse_dictation_live_begin()==2,"unsupported input uses preview");
        begin(); exposesPlaceholder=true; editor=@""; caret=NSMakeRange(0,0);
        expect(pulse_dictation_live_begin()==1,"placeholder editor supports live insertion");
        syncText("hello placeholder editor");
        expect([editor isEqualToString:@"hello placeholder editor"] && livePlaceholderConfirmed,"placeholder disappearance is not a user edit");
        syncText("corrected 🌍"); emptyParagraph=true; syncText(""); syncText("again");
        syncText("\nType here");
        expect([editor isEqualToString:@"\nType here"],"dictated text matching placeholder must remain real text");
        syncText("again");
        expect([editor isEqualToString:@"again"],"empty corrections and placeholder reappearance are supported");
        begin(); exposesPlaceholder=phantomPlaceholderCaret=true; editor=@""; caret=NSMakeRange(0,0);
        expect(pulse_dictation_live_begin()==1,"phantom placeholder caret remains readable");
        syncText("rich editor"); syncText(""); syncText("again");
        expect([editor isEqualToString:@"again"],"placeholder caret offsets do not break rich editors");
        begin(); exposesPlaceholder=true; editor=@"\nType here"; caret=NSMakeRange(0,0);
        expect(pulse_dictation_live_begin()==1,"label-like real text is a candidate only");
        syncText("prefix ");
        expect([editor isEqualToString:@"prefix \nType here"] && !livePlaceholderConfirmed,"never discard real text matching an input label");
        begin(); missingSystemFocus=true;
        expect(pulse_dictation_live_begin()==1,"application focus fallback supports embedded editors");
        syncText("app focused");
        expect([editor isEqualToString:@"Before app focused after"],"fallback retains verified insertion");
        AXUIElementRef fallbackOther=AXUIElementCreateApplication(getpid()+3);
        selectedElement=fallbackOther;
        unsigned fallbackEvents=keyboardEvents;
        expect(pulse_dictation_live_step("app focused more")==2 && keyboardEvents==fallbackEvents,"application fallback never retargets another focused input");
        selectedElement=fakeElement; CFRelease(fallbackOther);
        syncText("app focused more");
        begin(); editor=@""; caret=NSMakeRange(0,0); emptyCaretOffset=1;
        expect(pulse_dictation_live_begin()==1,"empty editor's synthetic position one is normalized");
        syncText("empty editor"); syncText("Empty editor."); syncText(""); syncText("again");
        expect([editor isEqualToString:@"again"],"synthetic empty caret works through insertion and corrections");
        begin(); editor=@""; caret=NSMakeRange(0,0); emptyCaretOffset=2;
        expect(pulse_dictation_live_begin()==2,"other out-of-range positions remain unsupported");
        emptyCaretOffset=1; characterCountOverride=1;
        expect(pulse_dictation_live_begin()==2,"inconsistent character counts cannot authorize empty-caret normalization");
        // Exercise the actual edit planner on deterministic, varied Unicode
        // sequences, including insertions longer than its alignment window.
        NSArray<NSString *> *tokens=@[@"a",@"b",@" ",@".",@"🌍",@"é",@"👨‍👩‍👧‍👦",@"中文"];
        uint32_t seed=42;
        for (unsigned trial=0;trial<500;trial++) {
            NSMutableString *from=[NSMutableString new], *to=[NSMutableString new];
            for (unsigned k=0;k<80;k++) {
                seed=seed*1664525u+1013904223u;
                if (seed&1) [from appendString:tokens[(seed>>16)%tokens.count]];
                seed=seed*1664525u+1013904223u;
                if (seed&4) [to appendString:tokens[(seed>>16)%tokens.count]];
            }
            NSString *value=from;
            for (unsigned step=0;step<250 && ![value isEqualToString:to];step++) {
                NSRange change; NSString *fragment;
                nextLiveEdit(value,to,&change,&fragment);
                expect(change.length || fragment.length,"each correction makes progress");
                expect(!change.length || NSEqualRanges(change,[value rangeOfComposedCharacterSequencesForRange:change]),"replacement stays on composed-character boundaries");
                value=[value stringByReplacingCharactersInRange:change withString:fragment];
            }
            expect([value isEqualToString:to],"bounded corrections converge to the authoritative transcript");
        }
        expect(clipboardWrites==0,"ALL input paths preserve clipboard");
        focused=false;
        expect(pulse_dictation_live_begin()==0,"no selected input uses bottom preview");
        expect(pulse_dictation_copy("clipboard result") && clipboardWrites==2,"clipboard only written by explicit no-input path");
        pulse_dictation_clear_target(); CFRelease(fakeElement); CFRelease(fakeSystem);
        puts("PASS: application focus fallback, synthetic empty caret, pinned input, final corrections without replay, Unicode, read-back failures, clipboard preservation");
    }
}
