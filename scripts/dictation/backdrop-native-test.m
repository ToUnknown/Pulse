// Actual native materials in an unshown window; no capture, typing or clipboard.
#include "../../src-tauri/src/dictation/native.m"
#include <assert.h>
int main(void) {
    @autoreleasepool {
        [NSApplication sharedApplication];
        NSWindow *window = [[NSWindow alloc] initWithContentRect:NSMakeRect(0,0,800,600)
            styleMask:NSWindowStyleMaskBorderless backing:NSBackingStoreBuffered defer:NO];
        void *pointer = (__bridge void *)window;
        pulse_dictation_transcript_blur(pointer,200,300,320,66,1,false);
        pulse_dictation_glass(pointer,328,376,64,28,1,false);
        NSView *host = PulseDictationMaterialHost(window);
        assert(host.subviews.count == 4);
        NSView *pillHalo = host.subviews[0], *textHalo = host.subviews[1];
        // Both halos stay below the transcript backing and optical glass.
        assert(pillHalo.layer.mask && textHalo.layer.mask);
        assert(NSEqualRects(pillHalo.frame,NSMakeRect(304,172,112,76)));
        assert(NSEqualRects(textHalo.frame,NSMakeRect(176,210,368,114)));
        assert(fabs(textHalo.alphaValue - .8) < .001);
        pulse_dictation_transcript_blur(pointer,200,240,320,126,.5,true);
        assert(NSEqualRects(textHalo.frame,NSMakeRect(176,210,368,174)));
        assert(NSEqualRects(textHalo.layer.mask.frame,textHalo.bounds));
        assert(fabs(textHalo.alphaValue - .4) < .001);
        pulse_dictation_glass(pointer,346,376,28,28,.5,true);
        assert(NSEqualRects(pillHalo.frame,NSMakeRect(322,172,76,76)));
        pulse_dictation_transcript_blur(pointer,200,240,320,126,0,true);
        pulse_dictation_glass(pointer,346,376,28,28,0,true);
        assert(textHalo.hidden && pillHalo.hidden);
        assert(!window.visible);
        CGImageRef mask = PulseBackdropMask();
        CFDataRef bytes = CGDataProviderCopyData(CGImageGetDataProvider(mask));
        const UInt8 *pixels = CFDataGetBytePtr(bytes);
        size_t stride = CGImageGetBytesPerRow(mask);
        assert(pixels[39*stride+39*4+3] == 255);
        assert(pixels[3] == 0); // No square corner at the outer edge.
        assert(pixels[39*stride+12*4+3] > 0 && pixels[39*stride+12*4+3] < 255);
        CFRelease(bytes);
        puts("PASS: 24pt feather, layering, growth, contraction, matching fade and hidden-window safety");
    }
}
