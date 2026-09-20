// Actual audio lifecycle bridge with a fake, slow engine. Never opens a mic.
#import <AppKit/AppKit.h>
#import <AVFoundation/AVFoundation.h>
#import <ApplicationServices/ApplicationServices.h>
#include <assert.h>
#include <unistd.h>

@interface TestCaptureDevice : NSObject
+ (AVAuthorizationStatus)authorizationStatusForMediaType:(AVMediaType)type;
+ (void)requestAccessForMediaType:(AVMediaType)type completionHandler:(void (^)(BOOL))handler;
@end
@implementation TestCaptureDevice
+ (AVAuthorizationStatus)authorizationStatusForMediaType:(AVMediaType)type { return AVAuthorizationStatusAuthorized; }
+ (void)requestAccessForMediaType:(AVMediaType)type completionHandler:(void (^)(BOOL))handler { handler(YES); }
@end
@interface TestAudioEngine : NSObject
@property(readonly) AVAudioInputNode *inputNode;
- (void)prepare;
- (void)stop;
- (BOOL)startAndReturnError:(NSError **)error;
@end
@implementation TestAudioEngine
- (instancetype)init { assert(!NSThread.isMainThread); usleep(100000); return [super init]; }
- (AVAudioInputNode *)inputNode { return nil; } // Fail safely after simulated setup.
- (void)prepare {}
- (void)stop {}
- (BOOL)startAndReturnError:(NSError **)error { return NO; }
@end
#define AVCaptureDevice TestCaptureDevice
#define AVAudioEngine TestAudioEngine
#include "../../src-tauri/src/dictation/native.m"

static void ready(void *context, bool started) {
    assert(!NSThread.isMainThread);
    assert(!started);
    (*(unsigned *)context)++;
}
static void audio(uint64_t id, const uint8_t *bytes, size_t size, float level) { abort(); }
int main(void) {
    @autoreleasepool {
        unsigned completions = 0;
        dispatch_semaphore_t gate = dispatch_semaphore_create(0);
        dispatch_async(PulseDictationAudioQueue(), ^{ dispatch_semaphore_wait(gate, DISPATCH_TIME_FOREVER); });
        // A blocked lifecycle queue must never block key-down/UI processing.
        pulse_dictation_start_async(1, audio, ready, &completions);
        assert(completions == 0);
        dispatch_semaphore_signal(gate);
        pulse_dictation_stop();
        assert(completions == 1); // Immediate release cannot overtake startup.
        pulse_dictation_start_async(2, audio, ready, &completions);
        pulse_dictation_stop();
        assert(completions == 2); // Repeated starts return ownership exactly once.
        puts("PASS: nonblocking key-down, off-main capture startup, ordered stop, exactly-once completion");
    }
}
