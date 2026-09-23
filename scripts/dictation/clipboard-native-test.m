#import <AppKit/AppKit.h>
#import "../../src-tauri/src/dictation/native.m"

static void check(BOOL condition, NSString *message) {
    if (!condition) [NSException raise:@"PulseClipboardTest" format:@"%@",message];
}

int main(void) {
    @autoreleasepool {
        NSPasteboard *board=[NSPasteboard pasteboardWithUniqueName];
        NSPasteboardType binary=@"app.pulse.test.binary";
        NSData *bytes=[NSData dataWithBytes:(uint8_t[]){0,1,2,255} length:4];
        NSPasteboardItem *item=[NSPasteboardItem new];
        [item setString:@"original" forType:NSPasteboardTypeString];
        [item setData:bytes forType:binary];
        [board clearContents];
        check([board writeObjects:@[item]],@"set original formats");

        NSArray<NSPasteboardItem *> *snapshot=clipboardSnapshot(board);
        check(snapshot.count==1,@"snapshot item count");
        [board clearContents];
        [board setString:@"transcript" forType:NSPasteboardTypeString];
        restoreClipboard(board,snapshot,board.changeCount);
        check([[board stringForType:NSPasteboardTypeString] isEqualToString:@"original"],@"restore text");
        check([[board.pasteboardItems.firstObject dataForType:binary] isEqualToData:bytes],@"restore custom format");

        [board clearContents];
        [board setString:@"transcript" forType:NSPasteboardTypeString];
        NSInteger temporary=board.changeCount;
        [board clearContents];
        [board setString:@"new user copy" forType:NSPasteboardTypeString];
        restoreClipboard(board,snapshot,temporary);
        check([[board stringForType:NSPasteboardTypeString] isEqualToString:@"new user copy"],@"preserve newer copy");

        [board clearContents];
        NSArray<NSPasteboardItem *> *empty=clipboardSnapshot(board);
        [board setString:@"transcript" forType:NSPasteboardTypeString];
        restoreClipboard(board,empty,board.changeCount);
        check(board.pasteboardItems.count==0,@"restore empty clipboard");
        [board releaseGlobally];
        NSLog(@"PASS: clipboard formats, guarded restoration, and empty clipboard");
    }
    return 0;
}
