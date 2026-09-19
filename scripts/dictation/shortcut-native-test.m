// Standalone Mac regression harness. No microphone, OS key injection, or API calls.
// Include the real adapter with only OS trust/key-state queries substituted.
#import <AppKit/AppKit.h>
#import <AVFoundation/AVFoundation.h>
#import <ApplicationServices/ApplicationServices.h>
static bool trusted = true;
static bool reportedKeyDown = false;
static bool TestKeyState(CGEventSourceStateID source, CGKeyCode key) {
    (void)source; (void)key; return reportedKeyDown;
}
static bool TestTrusted(void) { return trusted; }
#define CGEventSourceKeyState TestKeyState
#define AXIsProcessTrusted TestTrusted
#include "../../src-tauri/src/dictation/native.m"
#undef CGEventSourceKeyState
#undef AXIsProcessTrusted
#include <stdio.h>
#include <stdlib.h>
static unsigned presses, releases, interruptions;
static double lastTimestamp;
static void observe(bool down, bool chord, bool interrupted, double timestamp) {
    lastTimestamp = timestamp;
    if (interrupted) interruptions++;
    else if (!down && !chord) releases++;
    else if (down && !chord) presses++;
}
static void expect(bool condition, const char *message) {
    if (!condition) { fprintf(stderr,"FAIL: %s\n",message); exit(1); }
}
static NSEvent *flags(unsigned short key, NSEventModifierFlags modifiers) {
    return [NSEvent keyEventWithType:NSEventTypeFlagsChanged location:NSZeroPoint modifierFlags:modifiers timestamp:124.25 windowNumber:0 context:nil characters:@"" charactersIgnoringModifiers:@"" isARepeat:NO keyCode:key];
}
int main(void) {
    @autoreleasepool {
        shortcutCallback = observe;
        PulseDictationShortcutEvent(flags(0x3D, NSEventModifierFlagOption | NX_DEVICERALTKEYMASK));
        expect(shortcutRightDown && presses == 1,"Right Option press must start the hold");
        expect(lastTimestamp == 124.25,"native event timestamp must reach the tap/hold detector");
        // The event says held, but an independent key-state query reports false.
        // No Right Option release event has occurred. Repeated timer ticks must
        // not stop the microphone or produce an empty-transcription completion.
        for (int tick=0;tick<10;tick++) PulseDictationShortcutWatchdog();
        expect(shortcutRightDown && releases == 0,"watchdog ended transcription while Right Option was still held");
        PulseDictationShortcutEvent(flags(0x39,0)); // Unrelated Caps Lock event.
        expect(shortcutRightDown && releases == 0,"unrelated modifier must not release Right Option");
        PulseDictationShortcutEvent(flags(0x3D,0));
        expect(!shortcutRightDown && releases == 1,"actual Right Option release must finish the hold");
        // Hands-free recording also needs cancellation when the key is up.
        trusted = false;
        PulseDictationShortcutWatchdog();
        expect(interruptions == 1,"revoked Accessibility must cancel recording");
        puts("PASS: held key survives watchdog ticks; real release finishes; revoked access cancels");
    }
}
