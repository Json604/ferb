#import "AudioCaptureTestSupport.h"

static void FailIf(NSString *requested, NSString *operation) {
    if ([requested isEqualToString:operation]) {
        @throw [NSException exceptionWithName:@"com.apple.coreaudio.avfaudio"
                                      reason:@"fixture-private-reason" userInfo:nil];
    }
}

// AVAudioInputNode has no public initializer. This test double implements only
// the Objective-C selectors used by the boundary, without opening hardware.
@interface FERBTestInput : NSObject
@property(nonatomic, copy) NSString *failingOperation;
@property(nonatomic) double installedSampleRate;
@property(nonatomic) NSInteger outputFormatReads;
@property(nonatomic) NSInteger removedTaps;
@end
@implementation FERBTestInput
- (AVAudioFormat *)inputFormatForBus:(AVAudioNodeBus)bus {
    FailIf(self.failingOperation, @"format");
    return [[AVAudioFormat alloc] initStandardFormatWithSampleRate:16000 channels:1];
}
- (AVAudioFormat *)outputFormatForBus:(AVAudioNodeBus)bus {
    self.outputFormatReads++;
    return [[AVAudioFormat alloc] initStandardFormatWithSampleRate:44100 channels:1];
}
- (void)installTapOnBus:(AVAudioNodeBus)bus bufferSize:(AVAudioFrameCount)size
                format:(AVAudioFormat *)format block:(AVAudioNodeTapBlock)block {
    self.installedSampleRate = format.sampleRate;
    FailIf(self.failingOperation, @"install");
}
- (void)removeTapOnBus:(AVAudioNodeBus)bus {
    self.removedTaps++;
    FailIf(self.failingOperation, @"remove");
}
@end

@implementation FERBTestAudioEngine {
    FERBTestInput *_testInput;
}
- (instancetype)init {
    if ((self = [super init])) { _testInput = [FERBTestInput new]; _failingOperation = @""; }
    return self;
}
- (AVAudioInputNode *)inputNode {
    FailIf(self.failingOperation, @"input");
    _testInput.failingOperation = self.failingOperation;
    return (AVAudioInputNode *)_testInput;
}
- (double)installedSampleRate { return _testInput.installedSampleRate; }
- (NSInteger)outputFormatReads { return _testInput.outputFormatReads; }
- (NSInteger)removedTaps { return _testInput.removedTaps; }
- (void)prepare { FailIf(self.failingOperation, @"prepare"); }
- (BOOL)startAndReturnError:(NSError **)error {
    FailIf(self.failingOperation, @"start");
    if ([self.failingOperation isEqualToString:@"start-error"]) {
        if (error) *error = [NSError errorWithDomain:@"FixtureStartError" code:1 userInfo:nil];
        return NO;
    }
    return YES;
}
- (void)stop { FailIf(self.failingOperation, @"stop"); }
- (void)reset { FailIf(self.failingOperation, @"reset"); }
@end
