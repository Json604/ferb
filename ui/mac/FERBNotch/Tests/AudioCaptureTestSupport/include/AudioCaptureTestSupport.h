#import <AVFAudio/AVFAudio.h>

NS_ASSUME_NONNULL_BEGIN
// No microphone, permission prompt or real audio session is touched by this fixture.
@interface FERBTestAudioEngine : AVAudioEngine
@property(nonatomic, copy) NSString *failingOperation;
@property(nonatomic, readonly) double installedSampleRate;
@property(nonatomic, readonly) NSInteger outputFormatReads;
@property(nonatomic, readonly) NSInteger removedTaps;
@end
NS_ASSUME_NONNULL_END
